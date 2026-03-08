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
  pub agent_speaking: Arc<AtomicBool>, // voice activity flag
  pub peak: Arc<Mutex<f32>>,           // current audio peak
  pub spinner_index: usize,
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
  pub conversation_paused: Arc<AtomicBool>,
  pub voice: Arc<Mutex<String>>,
  pub ui: UiState,
  pub speed: AtomicU32,
  pub conversation_history: crate::conversation::ConversationHistory,
  pub agent_name: Arc<Mutex<String>>,
  pub agents: Arc<Vec<crate::config::AgentSettings>>,
  pub tts: Arc<Mutex<String>>,
  pub language: Arc<Mutex<String>>,
  pub provider: Arc<Mutex<String>>,
  pub baseurl: Arc<Mutex<String>>,
  pub model: Arc<Mutex<String>>,
  pub system_prompt: Arc<Mutex<String>>,
  pub playback: PlaybackState,
  pub status_line: Arc<Mutex<String>>,
  pub interrupt_counter: Arc<AtomicU64>,
  pub recording_paused: Arc<AtomicBool>,
  pub processing_response: Arc<AtomicBool>,
  pub ptt: Arc<AtomicBool>,
}

impl AppState {
  pub fn new() -> Self {
    Self {
      conversation_paused: Arc::new(AtomicBool::new(false)),
      voice: Arc::new(Mutex::new(String::new())),
      tts: Arc::new(Mutex::new(String::new())),
      language: Arc::new(Mutex::new(String::new())),
      provider: Arc::new(Mutex::new(String::new())),
      baseurl: Arc::new(Mutex::new(String::new())),
      model: Arc::new(Mutex::new(String::new())),
      system_prompt: Arc::new(Mutex::new(String::new())),

      ui: UiState {
        thinking: Arc::new(AtomicBool::new(false)),
        playing: Arc::new(AtomicBool::new(false)),
        agent_speaking: Arc::new(AtomicBool::new(false)), // tts synthesizing
        peak: Arc::new(Mutex::new(0.0)),
        spinner_index: 0,
      },
      speed: AtomicU32::new(12),
      conversation_history: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
      agent_name: Arc::new(Mutex::new(String::new())),
      agents: Arc::new(Vec::new()),
      playback: PlaybackState {
        paused: Arc::new(AtomicBool::new(false)),
        playback_active: Arc::new(AtomicBool::new(false)),
        gate_until_ms: Arc::new(AtomicU64::new(0)),
        volume: Arc::new(Mutex::new(1.0_f32)),
      },
      status_line: Arc::new(Mutex::new(String::new())),

      interrupt_counter: Arc::new(AtomicU64::new(0)),
      recording_paused: Arc::new(AtomicBool::new(false)),
      processing_response: Arc::new(AtomicBool::new(false)),
      ptt: Arc::new(AtomicBool::new(false)),
    }
  }

  /// Create a new AppState and initialize the voice field with the provided value.
  pub fn with_agent(
    settings: crate::config::AgentSettings,
    agents: Vec<crate::config::AgentSettings>,
  ) -> Self {
    let mut state = Self::new();
    // SAFETY: we have exclusive access to the Mutex here.
    *state.voice.lock().unwrap() = settings.voice.clone();
    *state.agent_name.lock().unwrap() = settings.name.clone();
    *state.tts.lock().unwrap() = settings.tts.clone();
    *state.language.lock().unwrap() = settings.language.clone();
    *state.provider.lock().unwrap() = settings.provider.clone();
    *state.baseurl.lock().unwrap() = settings.baseurl.clone();
    *state.model.lock().unwrap() = settings.model.clone();
    *state.system_prompt.lock().unwrap() = settings.system_prompt.clone();

    state.ptt.store(settings.ptt == "true", Ordering::Relaxed);
    state.agents = Arc::new(agents);
    state
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
