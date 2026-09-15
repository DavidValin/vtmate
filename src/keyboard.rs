// ------------------------------------------------------------------
//  Keyboard handling
// ------------------------------------------------------------------

use crate::conversation::Command;
use crate::state::{GLOBAL_STATE, decrease_voice_speed, increase_voice_speed};
use crate::text_field::{insert_char_at, remove_char_at};
use crossbeam_channel::Sender;
use crossterm::{
  event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
  terminal,
};

use crate::util::terminate;
use crossterm::event::KeyEvent;
use std::sync::{
  Arc,
  atomic::{AtomicBool, AtomicU64, Ordering},
};
use std::thread;
use std::time::{Duration, Instant};

// API
// ------------------------------------------------------------------

pub struct ReadFileMode {
  pub current_phrase: Arc<std::sync::atomic::AtomicUsize>,
  pub tts_paused: Arc<AtomicBool>,
  pub should_exit: Arc<AtomicBool>,
  pub display_update_tx: Sender<()>,
  /// Display line each phrase belongs to. Speaking is split at punctuation but
  /// the reader moves a line at a time, which is what is highlighted on screen:
  /// stepping through the phrases inside a line would leave the arrows looking
  /// like they did nothing.
  pub line_of: Vec<usize>,
  /// First phrase of each display line.
  pub line_starts: Vec<usize>,
}

impl ReadFileMode {
  /// First phrase of the line above the one `idx` is on.
  fn previous_line_start(&self, idx: usize) -> Option<usize> {
    let line = *self.line_of.get(idx)?;
    self.line_starts.get(line.checked_sub(1)?).copied()
  }

  /// First phrase of the line below the one `idx` is on.
  fn next_line_start(&self, idx: usize) -> Option<usize> {
    let line = *self.line_of.get(idx)?;
    self.line_starts.get(line + 1).copied()
  }
}
pub fn keyboard_thread(
  tx_ui: Sender<String>,
  recording_paused: Arc<AtomicBool>,
  stop_play_tx: Sender<()>,
  interrupt_counter: Arc<AtomicU64>,
  // Optional parameters for read-file mode
  read_file_mode: Option<ReadFileMode>,
  tx_cmd: Sender<Command>,
) {
  // Raw mode lets us capture single key presses (space to pause/resume).
  let ctx = KeyCtx {
    tx_ui: &tx_ui,
    recording_paused: &recording_paused,
    stop_play_tx: &stop_play_tx,
    interrupt_counter: &interrupt_counter,
    tx_cmd: &tx_cmd,
  };
  let mut st = KeyLocalState::default();
  loop {
    // Check read-file mode exit flag
    if let Some(ref rfm) = read_file_mode {
      if rfm.should_exit.load(Ordering::SeqCst) {
        break;
      }
    }

    if event::poll(Duration::from_millis(50)).unwrap_or(false) {
      if let Ok(Event::Key(k)) = event::read() {
        // Handle read-file mode separately
        if let Some(ref rfm) = read_file_mode {
          if k.kind != KeyEventKind::Press {
            continue;
          }

          // Ctrl+C exits
          if k.modifiers.contains(KeyModifiers::CONTROL) {
            if let KeyCode::Char('c') | KeyCode::Char('C') = k.code {
              rfm.should_exit.store(true, Ordering::SeqCst);
              break;
            }
          }

          match k.code {
            KeyCode::Up => {
              // One line back, in a single atomic step. The reader thread
              // advances this same counter when a phrase ends, so reading it
              // and writing it back separately loses one of the two moves.
              let moved = rfm
                .current_phrase
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |c| {
                  rfm.previous_line_start(c)
                })
                .is_ok();
              if moved {
                let _ = stop_play_tx.try_send(());
                interrupt_counter.fetch_add(1, Ordering::SeqCst);
                rfm.tts_paused.store(false, Ordering::SeqCst);
                let _ = rfm.display_update_tx.send(());
              }
            }
            KeyCode::Down => {
              // One line forward, atomically, for the same reason.
              let moved = rfm
                .current_phrase
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |c| {
                  rfm.next_line_start(c)
                })
                .is_ok();
              if moved {
                let _ = stop_play_tx.try_send(());
                interrupt_counter.fetch_add(1, Ordering::SeqCst);
                rfm.tts_paused.store(false, Ordering::SeqCst);
                let _ = rfm.display_update_tx.send(());
              }
            }
            KeyCode::Char(' ') => {
              let paused = rfm.tts_paused.load(Ordering::SeqCst);
              if paused {
                // Resume TTS playback - move index back one element if possible
                let moved = rfm
                  .current_phrase
                  .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |c| {
                    if c > 0 { Some(c - 1) } else { None }
                  })
                  .is_ok();
                if moved {
                  // Immediately abort any ongoing TTS/LLM by incrementing interrupt counter
                  interrupt_counter.fetch_add(1, Ordering::SeqCst);
                  thread::sleep(Duration::from_millis(10));
                  // Stop playback first
                  let _ = stop_play_tx.try_send(());
                  let _ = rfm.display_update_tx.send(());
                }
                rfm.tts_paused.store(false, Ordering::SeqCst);
              } else {
                // Stop TTS playback
                rfm.tts_paused.store(true, Ordering::SeqCst);
                let _ = stop_play_tx.try_send(());
                interrupt_counter.fetch_add(1, Ordering::SeqCst);
              }
            }
            _ => {}
          }
          continue; // Skip the rest of the normal keyboard handling
        }

        // Normal mode handling
        if let KeyOutcome::Quit = handle_key(&k, &ctx, &mut st) {
          thread::sleep(Duration::from_millis(20));
          terminate(0);
        }
      }
    }

    // If space was pressed but no new space event for a short period, consider
    // it released (only when PTT). Only in normal mode (not read-file mode).
    if read_file_mode.is_none() {
      st.tick(&ctx);
    }
  }

  // Always restore terminal state.
  let _ = terminal::disable_raw_mode();
}

/// Everything the key handler needs from the running engine. The terminal
/// keyboard thread and the daemon (keys forwarded by an attached client) build
/// one of these and share `handle_key`.
pub struct KeyCtx<'a> {
  pub tx_ui: &'a Sender<String>,
  pub recording_paused: &'a Arc<AtomicBool>,
  pub stop_play_tx: &'a Sender<()>,
  pub interrupt_counter: &'a Arc<AtomicU64>,
  pub tx_cmd: &'a Sender<Command>,
}

/// Per-input-source state: double-ESC timing and the space push-to-talk hold.
#[derive(Default)]
pub struct KeyLocalState {
  last_esc: Option<Instant>,
  pub space_pressed: bool,
  last_space_time: Option<Instant>,
}

pub enum KeyOutcome {
  Continue,
  /// Ctrl+C was pressed.
  Quit,
}

impl KeyLocalState {
  /// Push-to-talk release fallback for terminals that do not report key
  /// release events: no space event for 500 ms means the key is up.
  pub fn tick(&mut self, ctx: &KeyCtx) {
    let state = GLOBAL_STATE.get().unwrap();
    if state.ptt.load(Ordering::Relaxed) && self.space_pressed {
      if let Some(t) = self.last_space_time {
        if Instant::now().duration_since(t) > Duration::from_millis(500) {
          ctx.recording_paused.store(true, Ordering::Relaxed);
          self.space_pressed = false;
          self.last_space_time = None;
        }
      }
    }
  }
}

/// Handle one key event of the conversation mode (space PTT, ESC, undo,
/// speed, agent switch, debate modal). Mutates `GLOBAL_STATE` directly.
pub fn handle_key(k: &KeyEvent, ctx: &KeyCtx, st: &mut KeyLocalState) -> KeyOutcome {
  // Normal mode handling below
  let state = GLOBAL_STATE.get().expect("AppState not initialized");

  // Ctrl+C: the caller decides (terminal exits, daemon ignores it)
  if k.modifiers.contains(KeyModifiers::CONTROL) {
    if let KeyCode::Char('c') | KeyCode::Char('C') = k.code {
      return KeyOutcome::Quit;
    }
  }

  // The settings popup takes the whole keyboard while it is open, so that
  // typing a prompt cannot trigger the conversation shortcuts.
  if crate::settings_ui::is_open(state) {
    for message in crate::settings_ui::handle_key(state, k) {
      let _ = ctx.tx_ui.send(message);
    }
    return KeyOutcome::Continue;
  }

  if k.modifiers.contains(KeyModifiers::CONTROL) {
    // Ctrl+S opens the settings
    if let KeyCode::Char('s') | KeyCode::Char('S') = k.code {
      if k.kind == KeyEventKind::Press
        && !state.debate_modal_visible.load(Ordering::SeqCst)
        && !state.save_modal_visible.load(Ordering::SeqCst)
      {
        for message in crate::settings_ui::open(state) {
          let _ = ctx.tx_ui.send(message);
        }
      }
      return KeyOutcome::Continue;
    }
    // Ctrl+D toggles debate mode or shows modal
    if let KeyCode::Char('d') | KeyCode::Char('D') = k.code {
      let debate_enabled = state.debate_enabled.load(Ordering::SeqCst);
      let modal_visible = state.debate_modal_visible.load(Ordering::SeqCst);

      if !modal_visible && !state.save_modal_visible.load(Ordering::SeqCst) {
        if !debate_enabled {
          // Entering debate mode - show agent selection modal
          let agent_count = state.agents.lock().unwrap().len();
          if agent_count >= 2 {
            // Show modal for agent selection
            state.debate_modal_visible.store(true, Ordering::SeqCst);
            *state.debate_modal_selected_agent1.lock().unwrap() = 0;
            *state.debate_modal_selected_agent2.lock().unwrap() =
              if agent_count > 1 { 1 } else { 0 };
            *state.debate_modal_focus.lock().unwrap() = 0;
            *state.debate_modal_subject.lock().unwrap() = String::new();
            *state.debate_modal_caret.lock().unwrap() = 0;
            // Prefilled from the live value, not reset: --max-turns already
            // carries over between debates started with Control+D (see
            // README), so reopening the modal should show the limit that is
            // actually still in effect rather than blank it out.
            let current_max_turns = state.max_turns.load(Ordering::SeqCst);
            *state.debate_modal_max_turns.lock().unwrap() = if current_max_turns == 0 {
              String::new()
            } else {
              current_max_turns.to_string()
            };
            *state.debate_modal_max_turns_caret.lock().unwrap() =
              state.debate_modal_max_turns.lock().unwrap().chars().count();
            let _ = ctx.tx_ui.send("modal_show|".to_string());
          } else {
            // Not enough agents
            let _ = ctx.tx_ui.send(
              "line|\n\x1b[31m✗ Need at least 2 agents for debate mode\x1b[0m\n".to_string(),
            );
          }
        } else {
          // Exiting debate mode
          state.debate_enabled.store(false, Ordering::SeqCst);
          state.reset_conversation();
          state.debate_agents.lock().unwrap().clear();
          state.debate_turn.store(0, Ordering::SeqCst);
          *state.debate_subject.lock().unwrap() = String::new();
          state.debate_paused.store(false, Ordering::SeqCst);
          // Back to the selected agent alone; an engine it shares with a
          // debate agent stays loaded.
          crate::tts::apply_residency(state);
          // Interrupt any ongoing TTS playback
          ctx.interrupt_counter.fetch_add(1, Ordering::SeqCst);
          state
            .playback
            .playback_active
            .store(false, Ordering::Relaxed);
          let _ = ctx.tx_ui.send("line|\n\x1b[33m⇄ Debate mode DISABLED\x1b[0m\n".to_string());
        }
      }
    }
    // Ctrl+E shows the save popup (start a new recording, or stop the
    // running one). The settings popup already took over the whole keyboard
    // above if it is open, so only the debate modal needs checking here.
    if let KeyCode::Char('e') | KeyCode::Char('E') = k.code {
      if k.kind == KeyEventKind::Press
        && !state.debate_modal_visible.load(Ordering::SeqCst)
        && !state.save_modal_visible.load(Ordering::SeqCst)
      {
        open_save_modal(state);
        let _ = ctx.tx_ui.send("save_modal_show|".to_string());
      }
      return KeyOutcome::Continue;
    }
  }

  // Undo key handling ('u' to undo last response)
  if k.code == KeyCode::Char('u')
    && !state.debate_modal_visible.load(Ordering::SeqCst)
    && !state.save_modal_visible.load(Ordering::SeqCst)
    && k.kind == KeyEventKind::Press
  {
    // If a response is currently being processed, cancel undo
    if state.processing_response.load(Ordering::Relaxed) {
      return KeyOutcome::Continue;
    }
    // Interrupt TTS
    ctx.interrupt_counter.fetch_add(1, Ordering::SeqCst);
    thread::sleep(Duration::from_millis(10));
    // Ensure we also stop any ongoing playback first
    let _ = ctx.stop_play_tx.try_send(());
    // If a response is in progress, interrupt it first
    if state.processing_response.load(Ordering::Relaxed) {
      if !state.undo_pending.load(Ordering::SeqCst) {
        state.undo_pending.store(true, Ordering::SeqCst);
        // If debate is active, pause it
        if state.debate_enabled.load(Ordering::SeqCst) {
          if !state.debate_paused.load(Ordering::SeqCst) {
            state.debate_paused.store(true, Ordering::SeqCst);
            // send UI
            let _ = ctx.tx_ui.send(
              "line|\n\x1b[32m▮▮ Debate paused, speak again to continue \x1b[0m\n"
                .to_string(),
            );
          }
        }
      }
    }
    // Send undo command
    let _ = ctx.tx_cmd.send(Command::Undo);
    return KeyOutcome::Continue;
  }

  // 't' toggles the live TODO popup (shows the most recently touched TODO
  // list, refreshed from disk while open).
  if k.code == KeyCode::Char('t')
    && !state.debate_modal_visible.load(Ordering::SeqCst)
    && !state.save_modal_visible.load(Ordering::SeqCst)
    && k.kind == KeyEventKind::Press
  {
    let visible = state.todo_popup_visible.load(Ordering::SeqCst);
    if visible {
      state.todo_popup_visible.store(false, Ordering::SeqCst);
      let _ = ctx.tx_ui.send("todo_hide|".to_string());
    } else {
      state.todo_popup_visible.store(true, Ordering::SeqCst);
      let _ = ctx.tx_ui.send("todo_show|".to_string());
    }
    return KeyOutcome::Continue;
  }

  // Handle modal keyboard navigation
  let modal_visible = state.debate_modal_visible.load(Ordering::SeqCst);
  if modal_visible {
    // A terminal reporting the Kitty keyboard protocol sends a Release event
    // too; without this, every key here would also fire again on release.
    if k.kind != KeyEventKind::Press {
      return KeyOutcome::Continue;
    }
    match k.code {
      KeyCode::Esc => {
        // Close modal without starting debate
        state.debate_modal_visible.store(false, Ordering::SeqCst);
        let _ = ctx.tx_ui.send("modal_hide|".to_string());
      }
      KeyCode::Enter
        if *state.debate_modal_focus.lock().unwrap() == 3
          && !k.modifiers.contains(KeyModifiers::CONTROL) =>
      {
        // The subject field is a multi-line textarea: Enter writes a
        // newline into it, the same convention settings_ui's system-prompt
        // field uses. Ctrl+Enter confirms from here instead (see below).
        let mut caret = state.debate_modal_caret.lock().unwrap();
        let mut text = state.debate_modal_subject.lock().unwrap();
        insert_char_at(&mut text, *caret, '\n');
        *caret += 1;
        let _ = ctx.tx_ui.send("modal_update|".to_string());
      }
      KeyCode::Enter => {
        // Confirm selection and start debate
        let agents = state.agents();
        let agent1_idx = *state.debate_modal_selected_agent1.lock().unwrap();
        let agent2_idx = *state.debate_modal_selected_agent2.lock().unwrap();
        let max_turns_text = state.debate_modal_max_turns.lock().unwrap().clone();
        let max_turns = parse_modal_max_turns(&max_turns_text);

        // Same agent picked twice is allowed - it just debates itself.
        if max_turns_text.is_empty() {
          // blank = no limit, always valid
          start_debate(state, ctx.tx_ui, &agents, agent1_idx, agent2_idx, 0);
        } else if let Some(max_turns) = max_turns {
          start_debate(state, ctx.tx_ui, &agents, agent1_idx, agent2_idx, max_turns);
        } else {
          let _ = ctx.tx_ui.send(
            "line|\n\x1b[31m✗ Max turns must be blank (no limit) or 2-1000000000000\x1b[0m\n"
              .to_string(),
          );
        }
      }
      KeyCode::Up | KeyCode::Down | KeyCode::Tab => {
        // Inside the subject textarea, ↑/↓ walk its lines first (same as
        // settings_ui's system-prompt field) and only cycle focus once they
        // fall off the field's first/last line; Tab always cycles focus.
        let focus_now = *state.debate_modal_focus.lock().unwrap();
        let moved_within_text = focus_now == 3
          && k.code != KeyCode::Tab
          && move_subject_caret_line(state, k.code == KeyCode::Down);
        if !moved_within_text {
          // Switch focus between agent1, agent2, max turns, the subject
          // field, and the Start Debate button - the same convention the
          // agent settings form uses (Tab/↑/↓ move between fields, ←/→
          // change the current field's value / move its caret).
          let mut focus = state.debate_modal_focus.lock().unwrap();
          if k.code == KeyCode::Up {
            *focus = if *focus == 0 { 4 } else { *focus - 1 };
          } else {
            *focus = (*focus + 1) % 5;
          }
        }
        let _ = ctx.tx_ui.send("modal_update|".to_string());
      }
      KeyCode::Left => {
        let focus = *state.debate_modal_focus.lock().unwrap();
        let agent_count = state.agents.lock().unwrap().len();

        if focus == 0 {
          let mut agent1_idx = state.debate_modal_selected_agent1.lock().unwrap();
          *agent1_idx = if *agent1_idx == 0 {
            agent_count - 1
          } else {
            *agent1_idx - 1
          };
          let _ = ctx.tx_ui.send("modal_update|".to_string());
        } else if focus == 1 {
          let mut agent2_idx = state.debate_modal_selected_agent2.lock().unwrap();
          *agent2_idx = if *agent2_idx == 0 {
            agent_count - 1
          } else {
            *agent2_idx - 1
          };
          let _ = ctx.tx_ui.send("modal_update|".to_string());
        } else if focus == 2 {
          let mut caret = state.debate_modal_max_turns_caret.lock().unwrap();
          *caret = caret.saturating_sub(1);
          let _ = ctx.tx_ui.send("modal_update|".to_string());
        } else if focus == 3 {
          let mut caret = state.debate_modal_caret.lock().unwrap();
          *caret = caret.saturating_sub(1);
          let _ = ctx.tx_ui.send("modal_update|".to_string());
        }
        // focus == 4 (Start Debate button): nothing to move.
      }
      KeyCode::Right => {
        let focus = *state.debate_modal_focus.lock().unwrap();
        let agent_count = state.agents.lock().unwrap().len();

        if focus == 0 {
          let mut agent1_idx = state.debate_modal_selected_agent1.lock().unwrap();
          *agent1_idx = (*agent1_idx + 1) % agent_count;
          let _ = ctx.tx_ui.send("modal_update|".to_string());
        } else if focus == 1 {
          let mut agent2_idx = state.debate_modal_selected_agent2.lock().unwrap();
          *agent2_idx = (*agent2_idx + 1) % agent_count;
          let _ = ctx.tx_ui.send("modal_update|".to_string());
        } else if focus == 2 {
          let len = state.debate_modal_max_turns.lock().unwrap().chars().count();
          let mut caret = state.debate_modal_max_turns_caret.lock().unwrap();
          *caret = (*caret + 1).min(len);
          let _ = ctx.tx_ui.send("modal_update|".to_string());
        } else if focus == 3 {
          let len = state.debate_modal_subject.lock().unwrap().chars().count();
          let mut caret = state.debate_modal_caret.lock().unwrap();
          *caret = (*caret + 1).min(len);
          let _ = ctx.tx_ui.send("modal_update|".to_string());
        }
        // focus == 4 (Start Debate button): nothing to move.
      }
      KeyCode::Home if *state.debate_modal_focus.lock().unwrap() == 2 => {
        *state.debate_modal_max_turns_caret.lock().unwrap() = 0;
        let _ = ctx.tx_ui.send("modal_update|".to_string());
      }
      KeyCode::End if *state.debate_modal_focus.lock().unwrap() == 2 => {
        let len = state.debate_modal_max_turns.lock().unwrap().chars().count();
        *state.debate_modal_max_turns_caret.lock().unwrap() = len;
        let _ = ctx.tx_ui.send("modal_update|".to_string());
      }
      KeyCode::Home if *state.debate_modal_focus.lock().unwrap() == 3 => {
        let text = state.debate_modal_subject.lock().unwrap().clone();
        let caret_now = *state.debate_modal_caret.lock().unwrap();
        let (start, _) = current_line_bounds(&text, caret_now);
        *state.debate_modal_caret.lock().unwrap() = start;
        let _ = ctx.tx_ui.send("modal_update|".to_string());
      }
      KeyCode::End if *state.debate_modal_focus.lock().unwrap() == 3 => {
        let text = state.debate_modal_subject.lock().unwrap().clone();
        let caret_now = *state.debate_modal_caret.lock().unwrap();
        let (_, end) = current_line_bounds(&text, caret_now);
        *state.debate_modal_caret.lock().unwrap() = end;
        let _ = ctx.tx_ui.send("modal_update|".to_string());
      }
      KeyCode::Backspace if *state.debate_modal_focus.lock().unwrap() == 2 => {
        let mut caret = state.debate_modal_max_turns_caret.lock().unwrap();
        if *caret > 0 {
          let idx = *caret - 1;
          let mut text = state.debate_modal_max_turns.lock().unwrap();
          remove_char_at(&mut text, idx);
          *caret = idx;
          let _ = ctx.tx_ui.send("modal_update|".to_string());
        }
      }
      KeyCode::Backspace if *state.debate_modal_focus.lock().unwrap() == 3 => {
        let mut caret = state.debate_modal_caret.lock().unwrap();
        if *caret > 0 {
          let idx = *caret - 1;
          let mut text = state.debate_modal_subject.lock().unwrap();
          remove_char_at(&mut text, idx);
          *caret = idx;
          let _ = ctx.tx_ui.send("modal_update|".to_string());
        }
      }
      KeyCode::Delete if *state.debate_modal_focus.lock().unwrap() == 2 => {
        let caret = *state.debate_modal_max_turns_caret.lock().unwrap();
        let mut text = state.debate_modal_max_turns.lock().unwrap();
        if caret < text.chars().count() {
          remove_char_at(&mut text, caret);
          let _ = ctx.tx_ui.send("modal_update|".to_string());
        }
      }
      KeyCode::Delete if *state.debate_modal_focus.lock().unwrap() == 3 => {
        let caret = *state.debate_modal_caret.lock().unwrap();
        let mut text = state.debate_modal_subject.lock().unwrap();
        if caret < text.chars().count() {
          remove_char_at(&mut text, caret);
          let _ = ctx.tx_ui.send("modal_update|".to_string());
        }
      }
      KeyCode::Char(c)
        if *state.debate_modal_focus.lock().unwrap() == 2
          && c.is_ascii_digit()
          && !k.modifiers.contains(KeyModifiers::CONTROL) =>
      {
        // Capped to the field's own valid range's digit count (13, for
        // 1000000000000) - Enter still re-validates the actual value.
        let mut text = state.debate_modal_max_turns.lock().unwrap();
        if text.chars().count() < 13 {
          let mut caret = state.debate_modal_max_turns_caret.lock().unwrap();
          insert_char_at(&mut text, *caret, c);
          *caret += 1;
          let _ = ctx.tx_ui.send("modal_update|".to_string());
        }
      }
      KeyCode::Char(c)
        if *state.debate_modal_focus.lock().unwrap() == 3
          && !k.modifiers.contains(KeyModifiers::CONTROL) =>
      {
        let mut caret = state.debate_modal_caret.lock().unwrap();
        let mut text = state.debate_modal_subject.lock().unwrap();
        insert_char_at(&mut text, *caret, c);
        *caret += 1;
        let _ = ctx.tx_ui.send("modal_update|".to_string());
      }
      _ => {}
    }
    return KeyOutcome::Continue; // Don't process other keys when modal is visible
  }

  // Handle the save popup's keyboard navigation
  if state.save_modal_visible.load(Ordering::SeqCst) {
    if k.kind != KeyEventKind::Press {
      return KeyOutcome::Continue;
    }
    save_modal_key(state, ctx, k.code);
    return KeyOutcome::Continue;
  }

  match k.code {
    KeyCode::Char(' ') => {
      if state.ptt.load(Ordering::Relaxed) {
        crate::log::log("debug", &format!("SPACE event kind={:?}", k.kind));
        match k.kind {
          KeyEventKind::Press => {
            st.last_space_time = Some(Instant::now());
            ctx.recording_paused.store(false, Ordering::Relaxed);
            st.space_pressed = true;
          }
          KeyEventKind::Repeat => {
            st.last_space_time = Some(Instant::now());
            ctx.recording_paused.store(false, Ordering::Relaxed);
          }
          KeyEventKind::Release => {
            // Terminals that report key-release (e.g. Kitty protocol) give us
            // an explicit signal here, so pause immediately instead of waiting
            // on the idle-timeout fallback below.
            ctx.recording_paused.store(true, Ordering::Relaxed);
            st.space_pressed = false;
            st.last_space_time = None;
          }
        }
        crate::log::log(
          "debug",
          &format!(
            "recording_paused={}",
            ctx.recording_paused.load(Ordering::Relaxed)
          ),
        );
      } else {
        // Toggle pause on space press (no repeat handling)
        if k.kind == KeyEventKind::Press {
          let paused = ctx.recording_paused.load(Ordering::Relaxed);
          ctx.recording_paused.store(!paused, Ordering::Relaxed);
        }
      }
    }
    KeyCode::Esc => {
      let state = GLOBAL_STATE.get().expect("AppState not initialized");
      // Interrupt LLM/TTS
      ctx.interrupt_counter.fetch_add(1, Ordering::SeqCst);
      thread::sleep(Duration::from_millis(10));
      // Ensure we also stop any ongoing playback first
      let _ = ctx.stop_play_tx.try_send(());
      thread::sleep(Duration::from_millis(10));
      state.processing_response.store(false, Ordering::Relaxed);
      if state.debate_enabled.load(Ordering::SeqCst) {
        // only send the message once when we transition from running to paused
        if !state.debate_paused.load(Ordering::SeqCst) {
          state.debate_paused.store(true, Ordering::SeqCst);
          let _ = ctx.tx_ui.send(
            "line|\n\x1b[32m▮▮ Debate paused, speak again to continue \x1b[0m\n".to_string(),
          );
        }
      }
      let now = Instant::now();
      if let Some(prev) = st.last_esc {
        // double ESC stops playback and resets conversation
        if now.duration_since(prev) <= Duration::from_millis(1000) {
          st.last_esc = None;
          state.reset_conversation();
          if state.daemon_mode.load(Ordering::Relaxed) {
            // A history reset starts a new session: `-s`/`--save-html`
            // exported the one that just got cleared, and must be asked for
            // again (rather than silently resuming) to export whatever
            // comes next. This is the reset an attached client's own Esc-Esc
            // triggers over IPC; the global-hotkey reset has the same rule
            // in daemon::controller::Controller::reset_conversation.
            state.save_enabled.store(false, Ordering::Relaxed);
            state.save_html_enabled.store(false, Ordering::Relaxed);
            crate::daemon::desktop::notify("vtmate", "Conversation restarted!");
          }
          let _ = ctx.tx_ui.send("line|".to_string());
          let _ = ctx.tx_ui.send(
            "line|\n\x1b[32m↻ Session restarted (history reset) \x1b[0m\n".to_string(),
          );
        } else {
          st.last_esc = Some(now);
        }
      } else {
        st.last_esc = Some(now);
      }
    }

    // increase voice speed
    KeyCode::Up => {
      increase_voice_speed();
    }

    // decrease voice speed
    KeyCode::Down => {
      decrease_voice_speed();
    }

    // switch to previous / next agent
    KeyCode::Left | KeyCode::Right => {
      if !state.debate_enabled.load(Ordering::SeqCst) {
        let offset = if k.code == KeyCode::Left { -1 } else { 1 };
        if let Some(new_agent) = state.neighbour_agent(offset) {
          switch_agent(state, &new_agent, ctx.tx_ui);
        }
      }
    }
    _ => {
      // Any other key while space was pressed indicates release
      if st.space_pressed {
        ctx.recording_paused.store(true, Ordering::Relaxed);
        st.space_pressed = false;
      }
    }
  }
  KeyOutcome::Continue
}

/// Make `new_agent` the active agent: apply its settings, reset the
/// conversation, remember the choice in the settings file and tell the UI.
pub fn switch_agent(
  state: &crate::state::AppState,
  new_agent: &crate::config::AgentSettings,
  tx_ui: &Sender<String>,
) {
  state.apply_agent(new_agent);
  // The new agent may use a different engine: load what it needs, free the
  // rest.
  crate::tts::apply_residency(state);
  // Reset conversation history when changing agents
  state.reset_conversation();
  let settings_path = state.settings_path.lock().unwrap().clone();
  if settings_path.as_os_str().is_empty() {
    // no settings file in use (e.g. attached client mirror)
  } else if let Err(e) = crate::config::persist_selected_agent(&settings_path, &new_agent.name) {
    crate::log::log(
      "warning",
      &format!(
        "Could not save selected_agent to {}: {}",
        settings_path.display(),
        e
      ),
    );
  }
  let _ = tx_ui.send(format!(
    "line|\n\x1b[32m◆ Agent switched to '\x1b[37m{}\x1b[0m\x1b[32m' language: \x1b[37m{}\x1b[0m",
    new_agent.name, new_agent.language
  ));
}

/// (start, end) char index of the line containing `caret` in `text` - end
/// exclusive of that line's own trailing '\n', if any. Used to make Home/End
/// jump to the start/end of the current line rather than the whole field,
/// now that the debate modal's subject field can span several lines.
fn current_line_bounds(text: &str, caret: usize) -> (usize, usize) {
  let chars: Vec<char> = text.chars().collect();
  let caret = caret.min(chars.len());
  let start = chars[..caret]
    .iter()
    .rposition(|c| *c == '\n')
    .map_or(0, |i| i + 1);
  let end = chars[caret..]
    .iter()
    .position(|c| *c == '\n')
    .map_or(chars.len(), |rel| caret + rel);
  (start, end)
}

/// Move the debate modal's subject caret to the equivalent column on the
/// line above/below. `false` at the field's first (going up) or last (going
/// down) line, so the caller can fall back to cycling focus instead.
fn move_subject_caret_line(state: &crate::state::AppState, down: bool) -> bool {
  let text = state.debate_modal_subject.lock().unwrap().clone();
  let mut caret = state.debate_modal_caret.lock().unwrap();
  match crate::text_field::move_caret_vertical(&text, *caret, down) {
    Some(pos) => {
      *caret = pos;
      true
    }
    None => false,
  }
}

/// Validate the debate modal's max-turns text: all digits and within
/// `[2, 1_000_000_000_000]`. An empty string ("no limit") is handled
/// separately by the caller, not by this function.
fn parse_modal_max_turns(text: &str) -> Option<u64> {
  let n: u64 = text.parse().ok()?;
  crate::state::DEBATE_MAX_TURNS_RANGE.contains(&n).then_some(n)
}

/// Commit the debate modal: start a debate between `agents[agent1_idx]` and
/// `agents[agent2_idx]`, capped at `max_turns` replies (0 = no limit), with
/// whatever initial subject was typed.
fn start_debate(
  state: &crate::state::AppState,
  tx_ui: &Sender<String>,
  agents: &[crate::config::AgentSettings],
  agent1_idx: usize,
  agent2_idx: usize,
  max_turns: u64,
) {
  // The typed initial message, same role as `--debate`'s trailing <subject>;
  // left blank, the debate waits for a spoken topic instead (see
  // conversation::conversation_thread's turn-0 handling).
  let subject = state.debate_modal_subject.lock().unwrap().trim().to_string();
  let debate_agents = vec![agents[agent1_idx].clone(), agents[agent2_idx].clone()];
  *state.debate_agents.lock().unwrap() = debate_agents;
  state.debate_turn.store(0, Ordering::SeqCst);
  *state.debate_subject.lock().unwrap() = subject.clone();
  state.max_turns.store(max_turns, Ordering::SeqCst);
  // A popup-started debate switches back to conversation mode when
  // --max-turns is reached, instead of exiting the process - see
  // conversation::conversation_thread. This overrides a `--debate` CLI start
  // from earlier in the same run.
  state.debate_started_via_cli.store(false, Ordering::SeqCst);
  // Reset *before* flipping the flag below: the conversation thread's own
  // loop reacts to `debate_enabled` going true by submitting the subject
  // above as turn 0 - if that flip happened first, it could race this reset
  // and have the turn it just submitted wiped out from under it a moment
  // later instead.
  state.reset_conversation();
  // A pause left over from a previous debate (Esc sets this without clearing
  // it on exit) would otherwise make this fresh start silently drop its
  // typed subject at conversation_thread's pause check and sit waiting for a
  // spoken utterance instead.
  state.debate_paused.store(false, Ordering::SeqCst);
  state.debate_pending_submit.store(true, Ordering::SeqCst);
  state.debate_enabled.store(true, Ordering::SeqCst);
  // Both debate agents speak from here on, so both engines are wanted. Reads
  // `debate_enabled`/`debate_agents`, so must run after both are set.
  crate::tts::apply_residency(state);
  state.debate_modal_visible.store(false, Ordering::SeqCst);

  let _ = tx_ui.send("modal_hide|".to_string());
  let _ = tx_ui.send(format!(
    "line|\n\x1b[33m⇄ Debate mode ENABLED between '{}' and '{}'\x1b[0m",
    agents[agent1_idx].name, agents[agent2_idx].name
  ));
  if subject.is_empty() {
    let _ = tx_ui.send("line|\n\x1b[33m» Speak to set the debate topic or change the subject at any time\x1b[0m\n".to_string());
  } else {
    let _ = tx_ui.send(format!(
      "line|\n\x1b[33m» Debate topic: \x1b[37m{}\x1b[0m\n",
      subject
    ));
  }
  if max_turns > 0 {
    let _ = tx_ui.send(format!(
      "line|\x1b[33m» Debate will end automatically after {} turns\x1b[0m\n",
      max_turns
    ));
  }
}

/// Open the Ctrl+E save popup: refreshes the running session's folder name
/// (relevant if one is already running, in which case the popup opens
/// straight on the "stop recording" screen) and, only when nothing is
/// running yet, resets the checkbox choices to their defaults.
fn open_save_modal(state: &crate::state::AppState) {
  state.save_modal_visible.store(true, Ordering::SeqCst);
  *state.save_modal_folder.lock().unwrap() = crate::conversation::active_save_folder(state);
  let currently_saving = state.save_enabled.load(Ordering::Relaxed)
    || state.save_html_enabled.load(Ordering::Relaxed);
  if !currently_saving {
    state.save_modal_check_txt.store(true, Ordering::SeqCst);
    state.save_modal_check_html.store(false, Ordering::SeqCst);
    *state.save_modal_focus.lock().unwrap() = 0;
  }
}

/// Handle one key while the Ctrl+E popup is open.
fn save_modal_key(state: &crate::state::AppState, ctx: &KeyCtx, code: KeyCode) {
  let currently_saving =
    state.save_enabled.load(Ordering::Relaxed) || state.save_html_enabled.load(Ordering::Relaxed);

  if currently_saving {
    // "Stop recording" screen: only Enter and Esc do anything - Enter stops
    // the running save (the popup then falls back to the checkbox screen so
    // a new one can be started right away), Esc just closes the popup.
    match code {
      KeyCode::Enter => {
        crate::conversation::stop_save(state);
        *state.save_modal_folder.lock().unwrap() = None;
        state.save_modal_check_txt.store(true, Ordering::SeqCst);
        state.save_modal_check_html.store(false, Ordering::SeqCst);
        *state.save_modal_focus.lock().unwrap() = 0;
        let _ = ctx.tx_ui.send("save_modal_update|".to_string());
        let _ = ctx
          .tx_ui
          .send("line|\n\x1b[35m■ Recording stopped\x1b[0m\n".to_string());
      }
      KeyCode::Esc => {
        state.save_modal_visible.store(false, Ordering::SeqCst);
        let _ = ctx.tx_ui.send("save_modal_hide|".to_string());
      }
      _ => {}
    }
    return;
  }

  match code {
    KeyCode::Esc => {
      state.save_modal_visible.store(false, Ordering::SeqCst);
      let _ = ctx.tx_ui.send("save_modal_hide|".to_string());
    }
    KeyCode::Up | KeyCode::Down | KeyCode::Tab | KeyCode::BackTab => {
      // Cycles the two checkboxes and the Start Recording button.
      let mut focus = state.save_modal_focus.lock().unwrap();
      let backward = matches!(code, KeyCode::Up | KeyCode::BackTab);
      *focus = if backward {
        if *focus == 0 { 2 } else { *focus - 1 }
      } else {
        (*focus + 1) % 3
      };
      let _ = ctx.tx_ui.send("save_modal_update|".to_string());
    }
    KeyCode::Char(' ') => {
      let focus = *state.save_modal_focus.lock().unwrap();
      let flag = match focus {
        0 => &state.save_modal_check_txt,
        1 => &state.save_modal_check_html,
        _ => return, // focus == 2 (Start Recording button): nothing to toggle
      };
      let cur = flag.load(Ordering::SeqCst);
      flag.store(!cur, Ordering::SeqCst);
      let _ = ctx.tx_ui.send("save_modal_update|".to_string());
    }
    KeyCode::Enter => {
      let want_txt = state.save_modal_check_txt.load(Ordering::SeqCst);
      let want_html = state.save_modal_check_html.load(Ordering::SeqCst);
      if !want_txt && !want_html {
        // Nothing selected: the popup's own inline warning already explains
        // this, so there is nothing more to do here.
        return;
      }
      if want_txt {
        state.save_enabled.store(true, Ordering::Relaxed);
      }
      if want_html {
        state.save_html_enabled.store(true, Ordering::Relaxed);
      }
      state.save_modal_visible.store(false, Ordering::SeqCst);
      let _ = ctx.tx_ui.send("save_modal_hide|".to_string());
      let _ = ctx.tx_ui.send(
        "line|\n\x1b[35m● Saving to ~/.vtmate/conversations\x1b[0m\n".to_string(),
      );
    }
    _ => {}
  }
}
