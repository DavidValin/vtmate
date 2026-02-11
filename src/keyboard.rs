// ------------------------------------------------------------------
//  Keyboard handling
// ------------------------------------------------------------------

use crate::state::{decrease_voice_speed, increase_voice_speed};
use crossbeam_channel::{Receiver, Sender};
use crossterm::{
  event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
  terminal,
};
use std::sync::{
  Arc,
  atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

// API
// ------------------------------------------------------------------

pub fn keyboard_thread(
  stop_all_tx: Sender<()>,
  stop_all_rx: Receiver<()>,
  paused: Arc<AtomicBool>,
  playback_active: Arc<AtomicBool>,
  recording_paused: Arc<AtomicBool>,
) {
  // Raw mode lets us capture single key presses (space to pause/resume).
  let _ = terminal::enable_raw_mode();

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
            if playback_active.load(Ordering::Relaxed) {
              let new_val = !paused.load(Ordering::Relaxed);
              paused.store(new_val, Ordering::Relaxed);
            }
          }
          KeyCode::Esc => {
            let _ = stop_all_tx.try_send(());
            break;
          }
          KeyCode::Up => {
            increase_voice_speed();
          }
          KeyCode::Down => {
            decrease_voice_speed();
          }
          _ => {}
        }
        if (k.code == KeyCode::Char('p') || k.code == KeyCode::Char('P'))
          && k.modifiers.contains(KeyModifiers::CONTROL)
        {
          let new_val = !recording_paused.load(Ordering::Relaxed);
          recording_paused.store(new_val, Ordering::Relaxed);
        }
      }
    }
  }

  // Always restore terminal state.
  let _ = terminal::disable_raw_mode();
}
