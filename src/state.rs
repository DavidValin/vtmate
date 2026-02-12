// ------------------------------------------------------------------
//  Application state
// ------------------------------------------------------------------

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

// API
// ------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct UiState {
  pub thinking: Arc<AtomicBool>,
  pub playing: Arc<AtomicBool>,
  pub speaking: Arc<AtomicBool>, // voice activity flag
  pub peak: Arc<Mutex<f32>>,     // current audio peak
}

#[derive(Debug)]
pub struct PlaybackState {
  pub paused: Arc<AtomicBool>,
  pub playback_active: Arc<AtomicBool>,
  pub gate_until_ms: Arc<AtomicU64>,
  pub volume: Arc<Mutex<f32>>,
}

pub static GLOBAL_STATE: OnceLock<Arc<AppState>> = OnceLock::new();

#[derive(Debug)]
pub struct AppState {
  pub voice: Arc<Mutex<String>>,
  pub ui: UiState,
  pub speed: AtomicU32,
  pub conversation_history: std::sync::Arc<std::sync::Mutex<String>>,
  pub playback: PlaybackState,
  pub status_line: Arc<Mutex<String>>,
  pub print_lock: Arc<Mutex<()>>,
  pub interrupt_counter: Arc<AtomicU64>,
  pub recording_paused: Arc<AtomicBool>,
}

impl AppState {
  pub fn new_with_voice(voice: String) -> Self {
    Self {
      ui: UiState {
        thinking: Arc::new(AtomicBool::new(false)),
        playing: Arc::new(AtomicBool::new(false)),
        speaking: Arc::new(AtomicBool::new(false)),
        peak: Arc::new(Mutex::new(0.0)),
      },
      speed: AtomicU32::new(12),
      conversation_history: std::sync::Arc::new(std::sync::Mutex::new(String::new())),
      playback: PlaybackState {
        paused: Arc::new(AtomicBool::new(false)),
        playback_active: Arc::new(AtomicBool::new(false)),
        gate_until_ms: Arc::new(AtomicU64::new(0)),
        volume: Arc::new(Mutex::new(1.0_f32)),
      },
      status_line: Arc::new(Mutex::new(String::new())),
      print_lock: Arc::new(Mutex::new(())),
      interrupt_counter: Arc::new(AtomicU64::new(0)),
      recording_paused: Arc::new(AtomicBool::new(false)),
      voice: Arc::new(Mutex::new(voice)),
    }
  }
}

pub fn get_speed() -> f32 {
  let state = GLOBAL_STATE.get().expect("AppState not initialized");
  state.speed.load(Ordering::Relaxed) as f32 / 10.0
}

pub fn get_voice() -> String {
  let state = GLOBAL_STATE.get().expect("AppState not initialized");
  state.voice.lock().unwrap().clone()
}

/// Increase speed by 0.1, capped at 8.0.
pub fn increase_voice_speed() {
  let state = GLOBAL_STATE.get().expect("AppState not initialized");
  let mut cur = state.speed.load(Ordering::Relaxed);
  if cur < 80 {
    cur += 1;
    state.speed.store(cur, Ordering::Relaxed);
  }
}

/// Decrease speed by 0.1, floored at 0.5.
pub fn decrease_voice_speed() {
  let state = GLOBAL_STATE.get().expect("AppState not initialized");
  let mut cur = state.speed.load(Ordering::Relaxed);
  if cur > 5 {
    cur -= 1;
    state.speed.store(cur, Ordering::Relaxed);
  }
}
