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
) {
  // Raw mode lets us capture single key presses (space to pause/resume).
  let _ = terminal::enable_raw_mode();
  let mut last_esc: Option<Instant> = None;

  loop {
    if stop_all_rx.try_recv().is_ok() {
      break;
    }

    // Poll so we can also respond to stop_all.
    if event::poll(Duration::from_millis(50)).unwrap_or(false) {
      if let Ok(Event::Key(k)) = event::read() {
        // Only act on key presses (avoid repeats on some terminals)
        if k.kind != KeyEventKind::Press {
          continue;
        }

        // Ctrl+C should exit immediately (raw mode disables default SIGINT handling on many terminals).
        if k.modifiers.contains(KeyModifiers::CONTROL) {
          if let KeyCode::Char('c') | KeyCode::Char('C') = k.code {
            let _ = stop_all_tx.try_send(());
            break;
          }
        }

        match k.code {
          KeyCode::Char(' ') => {
            // Toggle recording pause only
            let new_val = !recording_paused.load(Ordering::Relaxed);
            recording_paused.store(new_val, Ordering::Relaxed);
          }

          KeyCode::Esc => {
            // stop playing
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
          _ => {}
        }

        //
      }
    }
  }

  // Always restore terminal state.
  let _ = terminal::disable_raw_mode();
}
