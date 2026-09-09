// ------------------------------------------------------------------
//  Keyboard handling
// ------------------------------------------------------------------

use crate::conversation::Command;
use crate::state::{GLOBAL_STATE, decrease_voice_speed, increase_voice_speed};
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
  pub phrases_len: usize,
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
              // One phrase back, in a single atomic step. The reader thread
              // advances this same counter when a phrase ends, so reading it
              // and writing it back separately loses one of the two moves.
              let moved = rfm
                .current_phrase
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |c| {
                  if c > 0 { Some(c - 1) } else { None }
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
              // One phrase forward, atomically, for the same reason.
              let last = rfm.phrases_len.saturating_sub(1);
              let moved = rfm
                .current_phrase
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |c| {
                  if c < last { Some(c + 1) } else { None }
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
      if k.kind == KeyEventKind::Press && !state.debate_modal_visible.load(Ordering::SeqCst) {
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

      if !modal_visible {
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
            let _ = ctx.tx_ui.send("modal_show|".to_string());
          } else {
            // Not enough agents
            let _ = ctx.tx_ui.send(
              "line|\n\x1b[31m❌ Need at least 2 agents for debate mode\x1b[0m\n".to_string(),
            );
          }
        } else {
          // Exiting debate mode
          state.debate_enabled.store(false, Ordering::SeqCst);
          state.reset_conversation();
          state.debate_agents.lock().unwrap().clear();
          state.debate_turn.store(0, Ordering::SeqCst);
          *state.debate_subject.lock().unwrap() = String::new();
          // Interrupt any ongoing TTS playback
          ctx.interrupt_counter.fetch_add(1, Ordering::SeqCst);
          state
            .playback
            .playback_active
            .store(false, Ordering::Relaxed);
          let _ = ctx.tx_ui.send("line|\n\x1b[33m🎭 Debate mode DISABLED\x1b[0m\n".to_string());
        }
      }
    }
  }

  // Undo key handling ('u' to undo last response)
  if k.code == KeyCode::Char('u')
    && !state.debate_modal_visible.load(Ordering::SeqCst)
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
              "line|\n\x1b[32m🚩 Debate paused, speak again to continue \x1b[0m\n"
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

  // Handle modal keyboard navigation
  let modal_visible = state.debate_modal_visible.load(Ordering::SeqCst);
  if modal_visible {
    match k.code {
      KeyCode::Esc => {
        // Close modal without starting debate
        state.debate_modal_visible.store(false, Ordering::SeqCst);
        let _ = ctx.tx_ui.send("modal_hide|".to_string());
      }
      KeyCode::Enter => {
        // Confirm selection and start debate
        let agents = state.agents();
        let agent1_idx = *state.debate_modal_selected_agent1.lock().unwrap();
        let agent2_idx = *state.debate_modal_selected_agent2.lock().unwrap();

        if agent1_idx == agent2_idx {
          let _ = ctx.tx_ui.send(
            "line|\n\x1b[31m❌ Please select two different agents\x1b[0m\n".to_string(),
          );
        } else {
          let debate_agents = vec![agents[agent1_idx].clone(), agents[agent2_idx].clone()];
          *state.debate_agents.lock().unwrap() = debate_agents;
          state.debate_turn.store(0, Ordering::SeqCst);
          *state.debate_subject.lock().unwrap() =
            "Let's debate. What should we discuss?".to_string();
          state.debate_enabled.store(true, Ordering::SeqCst);
          state.reset_conversation();
          state.debate_modal_visible.store(false, Ordering::SeqCst);

          let _ = ctx.tx_ui.send("modal_hide|".to_string());
          let _ = ctx.tx_ui.send(format!(
            "line|\n\x1b[33m🎭 Debate mode ENABLED between '{}' and '{}'\x1b[0m",
            agents[agent1_idx].name, agents[agent2_idx].name
          ));
          let _ = ctx.tx_ui.send("line|\n\x1b[33m💬 Speak to set the debate topic or change the subject at any time\x1b[0m\n".to_string());
        }
      }
      KeyCode::Up => {
        let focus = *state.debate_modal_focus.lock().unwrap();
        let agent_count = state.agents.lock().unwrap().len();

        if focus == 0 {
          // Agent 1 selection - move up
          let mut agent1_idx = state.debate_modal_selected_agent1.lock().unwrap();
          *agent1_idx = if *agent1_idx == 0 {
            agent_count - 1
          } else {
            *agent1_idx - 1
          };
          let _ = ctx.tx_ui.send("modal_update|".to_string());
        } else if focus == 1 {
          // Agent 2 selection - move up
          let mut agent2_idx = state.debate_modal_selected_agent2.lock().unwrap();
          *agent2_idx = if *agent2_idx == 0 {
            agent_count - 1
          } else {
            *agent2_idx - 1
          };
          let _ = ctx.tx_ui.send("modal_update|".to_string());
        }
      }
      KeyCode::Down => {
        let focus = *state.debate_modal_focus.lock().unwrap();
        let agent_count = state.agents.lock().unwrap().len();

        if focus == 0 {
          // Agent 1 selection - move down
          let mut agent1_idx = state.debate_modal_selected_agent1.lock().unwrap();
          *agent1_idx = (*agent1_idx + 1) % agent_count;
          let _ = ctx.tx_ui.send("modal_update|".to_string());
        } else if focus == 1 {
          // Agent 2 selection - move down
          let mut agent2_idx = state.debate_modal_selected_agent2.lock().unwrap();
          *agent2_idx = (*agent2_idx + 1) % agent_count;
          let _ = ctx.tx_ui.send("modal_update|".to_string());
        }
      }
      KeyCode::Left | KeyCode::Right | KeyCode::Tab => {
        // Switch focus between agent1, agent2, and confirm button
        let mut focus = state.debate_modal_focus.lock().unwrap();
        if k.code == KeyCode::Left {
          *focus = if *focus == 0 { 2 } else { *focus - 1 };
        } else {
          *focus = (*focus + 1) % 3;
        }
        let _ = ctx.tx_ui.send("modal_update|".to_string());
      }
      _ => {}
    }
    return KeyOutcome::Continue; // Don't process other keys when modal is visible
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
            "line|\n\x1b[32m🚩 Debate paused, speak again to continue \x1b[0m\n".to_string(),
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
            crate::daemon::desktop::notify("vtmate", "Conversation restarted!");
          }
          let _ = ctx.tx_ui.send("line|".to_string());
          let _ = ctx.tx_ui.send(
            "line|\n\x1b[32m✨ Session restarted (history reset) \x1b[0m\n".to_string(),
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
    "line|\n\x1b[32m🤖 Agent switched to '\x1b[37m{}\x1b[0m\x1b[32m' language: \x1b[37m{}\x1b[0m",
    new_agent.name, new_agent.language
  ));
}
