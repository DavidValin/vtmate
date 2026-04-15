// ------------------------------------------------------------------
//  Keyboard handling
// ------------------------------------------------------------------

use crate::state::{decrease_voice_speed, increase_voice_speed, GLOBAL_STATE};
use crossbeam_channel::{Receiver, Sender};
use crossterm::{
  event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
  terminal,
};
use std::sync::{
  atomic::{AtomicBool, AtomicU64, Ordering},
  Arc,
};
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
  stop_all_tx: Sender<()>,
  stop_all_rx: Receiver<()>,
  recording_paused: Arc<AtomicBool>,
  stop_play_tx: Sender<()>,
  interrupt_counter: Arc<AtomicU64>,
  // Optional parameters for read-file mode
  read_file_mode: Option<ReadFileMode>,
) {
  // Raw mode lets us capture single key presses (space to pause/resume).
  let mut last_esc: Option<Instant> = None;

  // Track if space was pressed and when last space event occurred
  let mut space_pressed = false;
  let mut last_space_time: Option<Instant> = None;
  loop {
    // Check read-file mode exit flag
    if let Some(ref rfm) = read_file_mode {
      if rfm.should_exit.load(Ordering::SeqCst) {
        break;
      }
    }

    if stop_all_rx.try_recv().is_ok() {
      break;
    }

    // Poll so we can also respond to stop_all.
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
            KeyCode::Char('u') | KeyCode::Char('U') => {
              // 'u': previous phrase
              let curr = rfm.current_phrase.load(Ordering::SeqCst);
              if curr > 0 {
                // Stop current playback
                let _ = stop_play_tx.try_send(());
                interrupt_counter.fetch_add(1, Ordering::SeqCst);
                // Move to previous phrase
                rfm.current_phrase.store(curr - 1, Ordering::SeqCst);
                rfm.tts_paused.store(false, Ordering::SeqCst);
                // Trigger display update
                let _ = rfm.display_update_tx.send(());
              }
            }
            KeyCode::Char('d') | KeyCode::Char('D') => {
              // 'd': next phrase
              let curr = rfm.current_phrase.load(Ordering::SeqCst);
              if curr < rfm.phrases_len - 1 {
                // Stop current playback
                let _ = stop_play_tx.try_send(());
                interrupt_counter.fetch_add(1, Ordering::SeqCst);
                // Move to next phrase
                rfm.current_phrase.store(curr + 1, Ordering::SeqCst);
                rfm.tts_paused.store(false, Ordering::SeqCst);
                // Trigger display update
                let _ = rfm.display_update_tx.send(());
              }
            }
            KeyCode::Char('s') | KeyCode::Char('S') => {
              // Stop TTS playback
              rfm.tts_paused.store(true, Ordering::SeqCst);
              let _ = stop_play_tx.try_send(());
              interrupt_counter.fetch_add(1, Ordering::SeqCst);
            }
            KeyCode::Char('p') | KeyCode::Char('P') => {
              // Continue TTS playback
              rfm.tts_paused.store(false, Ordering::SeqCst);
            }
            _ => {}
          }
          continue; // Skip the rest of the normal keyboard handling
        }

        // Normal mode handling below
        let state = GLOBAL_STATE.get().unwrap();

        // Ctrl+C should exit immediately
        if k.modifiers.contains(KeyModifiers::CONTROL) {
          if let KeyCode::Char('c') | KeyCode::Char('C') = k.code {
            let _ = stop_all_tx.try_send(());
            let _ = tx_ui.try_send("stop_ui|".to_string());
            break;
          }
          // Ctrl+D toggles debate mode or shows modal
          if let KeyCode::Char('d') | KeyCode::Char('D') = k.code {
            let debate_enabled = state.debate_enabled.load(Ordering::SeqCst);
            let modal_visible = state.debate_modal_visible.load(Ordering::SeqCst);

            if !modal_visible {
              if !debate_enabled {
                // Entering debate mode - show agent selection modal
                let agents = state.agents.as_ref();
                if agents.len() >= 2 {
                  // Show modal for agent selection
                  state.debate_modal_visible.store(true, Ordering::SeqCst);
                  *state.debate_modal_selected_agent1.lock().unwrap() = 0;
                  *state.debate_modal_selected_agent2.lock().unwrap() =
                    if agents.len() > 1 { 1 } else { 0 };
                  *state.debate_modal_focus.lock().unwrap() = 0;
                  let _ = tx_ui.send("modal_show|".to_string());
                } else {
                  // Not enough agents
                  let _ = tx_ui.send(
                    "line|\n\x1b[31m❌ Need at least 2 agents for debate mode\x1b[0m\n".to_string(),
                  );
                }
              } else {
                // Exiting debate mode
                state.debate_enabled.store(false, Ordering::SeqCst);
                state.debate_agents.lock().unwrap().clear();
                state.debate_turn.store(0, Ordering::SeqCst);
                *state.debate_subject.lock().unwrap() = String::new();
                // Interrupt any ongoing TTS playback
                interrupt_counter.fetch_add(1, Ordering::SeqCst);
                state
                  .playback
                  .playback_active
                  .store(false, Ordering::Relaxed);
                let _ = tx_ui.send("line|\n\x1b[33m🎭 Debate mode DISABLED\x1b[0m\n".to_string());
              }
            }
          }
        }

        // Handle modal keyboard navigation
        let modal_visible = state.debate_modal_visible.load(Ordering::SeqCst);
        if modal_visible {
          match k.code {
            KeyCode::Esc => {
              // Close modal without starting debate
              state.debate_modal_visible.store(false, Ordering::SeqCst);
              let _ = tx_ui.send("modal_hide|".to_string());
            }
            KeyCode::Enter => {
              // Confirm selection and start debate
              let agents = state.agents.as_ref();
              let agent1_idx = *state.debate_modal_selected_agent1.lock().unwrap();
              let agent2_idx = *state.debate_modal_selected_agent2.lock().unwrap();

              if agent1_idx == agent2_idx {
                let _ = tx_ui.send(
                  "line|\n\x1b[31m❌ Please select two different agents\x1b[0m\n".to_string(),
                );
              } else {
                let debate_agents = vec![agents[agent1_idx].clone(), agents[agent2_idx].clone()];
                *state.debate_agents.lock().unwrap() = debate_agents;
                state.debate_turn.store(0, Ordering::SeqCst);
                *state.debate_subject.lock().unwrap() =
                  "Let's debate. What should we discuss?".to_string();
                state.debate_enabled.store(true, Ordering::SeqCst);
                state.debate_modal_visible.store(false, Ordering::SeqCst);

                let _ = tx_ui.send("modal_hide|".to_string());
                let _ = tx_ui.send(format!(
                  "line|\n\x1b[33m🎭 Debate mode ENABLED between '{}' and '{}'\x1b[0m",
                  agents[agent1_idx].name, agents[agent2_idx].name
                ));
                let _ = tx_ui.send("line|\n\x1b[33m💬 Speak to set the debate topic or change the subject at any time\x1b[0m\n".to_string());
              }
            }
            KeyCode::Up => {
              let focus = *state.debate_modal_focus.lock().unwrap();
              let agents = state.agents.as_ref();

              if focus == 0 {
                // Agent 1 selection - move up
                let mut agent1_idx = state.debate_modal_selected_agent1.lock().unwrap();
                *agent1_idx = if *agent1_idx == 0 {
                  agents.len() - 1
                } else {
                  *agent1_idx - 1
                };
                let _ = tx_ui.send("modal_update|".to_string());
              } else if focus == 1 {
                // Agent 2 selection - move up
                let mut agent2_idx = state.debate_modal_selected_agent2.lock().unwrap();
                *agent2_idx = if *agent2_idx == 0 {
                  agents.len() - 1
                } else {
                  *agent2_idx - 1
                };
                let _ = tx_ui.send("modal_update|".to_string());
              }
            }
            KeyCode::Down => {
              let focus = *state.debate_modal_focus.lock().unwrap();
              let agents = state.agents.as_ref();

              if focus == 0 {
                // Agent 1 selection - move down
                let mut agent1_idx = state.debate_modal_selected_agent1.lock().unwrap();
                *agent1_idx = (*agent1_idx + 1) % agents.len();
                let _ = tx_ui.send("modal_update|".to_string());
              } else if focus == 1 {
                // Agent 2 selection - move down
                let mut agent2_idx = state.debate_modal_selected_agent2.lock().unwrap();
                *agent2_idx = (*agent2_idx + 1) % agents.len();
                let _ = tx_ui.send("modal_update|".to_string());
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
              let _ = tx_ui.send("modal_update|".to_string());
            }
            _ => {}
          }
          continue; // Don't process other keys when modal is visible
        }

        match k.code {
          KeyCode::Char(' ') => {
            if state.ptt.load(Ordering::Relaxed) {
              crate::log::log("debug", &format!("SPACE event kind={:?}", k.kind));
              last_space_time = Some(Instant::now());
              match k.kind {
                KeyEventKind::Press => {
                  recording_paused.store(false, Ordering::Relaxed);
                  space_pressed = true;
                }
                KeyEventKind::Repeat => {
                  recording_paused.store(false, Ordering::Relaxed);
                }
                _ => {}
              }
              crate::log::log(
                "debug",
                &format!(
                  "recording_paused={}",
                  recording_paused.load(Ordering::Relaxed)
                ),
              );
            } else {
              // Toggle pause on space press (no repeat handling)
              if k.kind == KeyEventKind::Press {
                let paused = recording_paused.load(Ordering::Relaxed);
                recording_paused.store(!paused, Ordering::Relaxed);
              }
            }
          }
          KeyCode::Esc => {
            let _ = stop_play_tx.try_send(());
            let now = Instant::now();
            if let Some(prev) = last_esc {
              if now.duration_since(prev) <= Duration::from_millis(1000) {
                // double ESC stops playback and interrupts conversation
                interrupt_counter.fetch_add(1, Ordering::SeqCst);
                // flag that we are waiting for next LLM response
                GLOBAL_STATE
                  .get()
                  .unwrap()
                  .processing_response
                  .store(true, Ordering::Relaxed);
                last_esc = None;
              } else {
                last_esc = Some(now);
              }
            } else {
              last_esc = Some(now);
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

          // switch to previous agent
          KeyCode::Left => {
            let agents = state.agents.as_ref();
            let current_name = state.agent_name.lock().unwrap().clone();
            let pos = agents
              .iter()
              .position(|a| a.name == current_name)
              .unwrap_or(0);
            let new_idx = if pos == 0 { agents.len() - 1 } else { pos - 1 };
            let new_agent = &agents[new_idx];
            *state.voice.lock().unwrap() = new_agent.voice.clone();
            *state.agent_name.lock().unwrap() = new_agent.name.clone();
            *state.tts.lock().unwrap() = new_agent.tts.clone();
            *state.language.lock().unwrap() = new_agent.language.clone();
            *state.provider.lock().unwrap() = new_agent.provider.clone();
            *state.baseurl.lock().unwrap() = new_agent.baseurl.clone();
            *state.model.lock().unwrap() = new_agent.model.clone();
            *state.system_prompt.lock().unwrap() = new_agent.system_prompt.clone();
            state.ptt.store(new_agent.ptt, Ordering::Relaxed);
            if state.ptt.load(Ordering::Relaxed) {
              recording_paused.store(true, Ordering::Relaxed);
            } else {
              recording_paused.store(false, Ordering::Relaxed);
            }
            // Reset conversation history when changing agents
            state.conversation_history.lock().unwrap().clear();
            let _ = tx_ui.send(format!(
              "line|\n\x1b[32m🤖 Agent switched to '\x1b[37m{}\x1b[0m\x1b[32m' language: \x1b[37m{}\x1b[0m",
              new_agent.name,
              new_agent.language
            ));
          }

          // switch to next agent
          KeyCode::Right => {
            let state = GLOBAL_STATE.get().unwrap();
            let agents = state.agents.as_ref();
            let current_name = state.agent_name.lock().unwrap().clone();
            let pos = agents
              .iter()
              .position(|a| a.name == current_name)
              .unwrap_or(0);
            let new_idx = (pos + 1) % agents.len();
            let new_agent = &agents[new_idx];
            *state.voice.lock().unwrap() = new_agent.voice.clone();
            *state.agent_name.lock().unwrap() = new_agent.name.clone();
            *state.tts.lock().unwrap() = new_agent.tts.clone();
            *state.language.lock().unwrap() = new_agent.language.clone();
            *state.provider.lock().unwrap() = new_agent.provider.clone();
            *state.baseurl.lock().unwrap() = new_agent.baseurl.clone();
            *state.model.lock().unwrap() = new_agent.model.clone();
            *state.system_prompt.lock().unwrap() = new_agent.system_prompt.clone();
            state.ptt.store(new_agent.ptt, Ordering::Relaxed);
            if state.ptt.load(Ordering::Relaxed) {
              recording_paused.store(true, Ordering::Relaxed);
            } else {
              recording_paused.store(false, Ordering::Relaxed);
            }
            // Reset conversation history when changing agents
            state.conversation_history.lock().unwrap().clear();
            let _ = tx_ui.send(format!(
              "line|\n\x1b[32m🤖 Agent switched to '\x1b[37m{}\x1b[0m\x1b[32m' language: \x1b[37m{}\x1b[0m",
              new_agent.name,
              new_agent.language
            ));
          }
          _ => {
            // Any other key while space was pressed indicates release
            if space_pressed {
              recording_paused.store(true, Ordering::Relaxed);
              space_pressed = false;
            }
          }
        }
      }
    }

    // If space was pressed but no new space event for a short period, consider it released (only when PTT)
    // Only check this in normal mode (not read-file mode)
    if read_file_mode.is_none() {
      let state = GLOBAL_STATE.get().unwrap();
      if state.ptt.load(Ordering::Relaxed) && space_pressed {
        if let Some(t) = last_space_time {
          if Instant::now().duration_since(t) > Duration::from_millis(500) {
            recording_paused.store(true, Ordering::Relaxed);
            space_pressed = false;
            last_space_time = None;
          }
        }
      }
    }
  }

  // Always restore terminal state.
  let _ = terminal::disable_raw_mode();
}
