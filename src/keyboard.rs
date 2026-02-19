// ------------------------------------------------------------------
//  Keyboard handling
// ------------------------------------------------------------------

use crate::state::{GLOBAL_STATE, decrease_voice_speed, increase_voice_speed};
use crate::tts;
use crossbeam_channel::{Receiver, Sender};
use crossterm::{
  event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
  terminal,
};
use std::sync::{
  Arc, Mutex,
  atomic::{AtomicBool, AtomicU64, Ordering},
};
use std::time::{Duration, Instant};

// API
// ------------------------------------------------------------------

pub fn keyboard_thread(
  stop_all_tx: Sender<()>,
  stop_all_rx: Receiver<()>,
  _paused: Arc<AtomicBool>,
  recording_paused: Arc<AtomicBool>,
  voice_state: Arc<Mutex<String>>,
  tts: String,
  language: String,
  stop_play_tx: Sender<()>,
  interrupt_counter: Arc<AtomicU64>,
  ptt: bool,
) {
  // Raw mode lets us capture single key presses (space to pause/resume).
  let _ = terminal::enable_raw_mode();
  let mut last_esc: Option<Instant> = None;

  // Track if space was pressed and when last space event occurred
  let mut space_pressed = false;
  let mut last_space_time: Option<Instant> = None;
  loop {
    if stop_all_rx.try_recv().is_ok() {
      break;
    }

    // Poll so we can also respond to stop_all.
    if event::poll(Duration::from_millis(50)).unwrap_or(false) {
      if let Ok(Event::Key(k)) = event::read() {
        // Ctrl+C should exit immediately
        if k.modifiers.contains(KeyModifiers::CONTROL) {
          if let KeyCode::Char('c') | KeyCode::Char('C') = k.code {
            let _ = stop_all_tx.try_send(());
            break;
          }
        }

        match k.code {
          KeyCode::Char(' ') => {
            if ptt {
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

          // swap to previous voice
          KeyCode::Left => {
            let voices = tts::get_voices_for(&tts, &language);
            let mut current = voice_state.lock().unwrap();
            if !voices.is_empty() {
              let pos = voices.iter().position(|v| *v == *current).unwrap_or(0);
              let new_idx = if pos == 0 { voices.len() - 1 } else { pos - 1 };
              *current = voices[new_idx].to_string();
            }
          }

          // swap to next voice
          KeyCode::Right => {
            let voices = tts::get_voices_for(&tts, &language);
            let mut current = voice_state.lock().unwrap();
            if !voices.is_empty() {
              let pos = voices.iter().position(|v| *v == *current).unwrap_or(0);
              let new_idx = (pos + 1) % voices.len();
              *current = voices[new_idx].to_string();
            }
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
    if ptt && space_pressed {
      if let Some(t) = last_space_time {
        if Instant::now().duration_since(t) > Duration::from_millis(500) {
          recording_paused.store(true, Ordering::Relaxed);
          space_pressed = false;
          last_space_time = None;
        }
      }
    }
  }

  // Always restore terminal state.
  let _ = terminal::disable_raw_mode();
}
