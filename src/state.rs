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
  pub quiet: bool,
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
  pub sound_threshold_peak: Arc<Mutex<f32>>,
  pub end_silence_ms: Arc<Mutex<u64>>,
  pub whisper_model_path: Arc<Mutex<String>>,
  pub debate_enabled: Arc<AtomicBool>,
  pub debate_subject: Arc<Mutex<String>>,
  pub debate_agents: Arc<Mutex<Vec<crate::config::AgentSettings>>>,
  pub debate_turn: Arc<AtomicU64>,
  pub debate_paused: Arc<AtomicBool>,
  pub debate_modal_visible: Arc<AtomicBool>,
  pub debate_modal_selected_agent1: Arc<Mutex<usize>>,
  pub debate_modal_selected_agent2: Arc<Mutex<usize>>,
  pub debate_modal_focus: Arc<Mutex<u8>>, // 0 = agent1, 1 = agent2, 2 = confirm
  pub save_path: Arc<Mutex<Option<std::path::PathBuf>>>,
  pub start_date: Arc<Mutex<String>>,
  pub undo_pending: Arc<AtomicBool>,
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
        quiet: false,
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
      sound_threshold_peak: Arc::new(Mutex::new(0.0)),
      end_silence_ms: Arc::new(Mutex::new(0)),
      whisper_model_path: Arc::new(Mutex::new(String::new())),
      debate_enabled: Arc::new(AtomicBool::new(false)),
      debate_subject: Arc::new(Mutex::new(String::new())),
      debate_agents: Arc::new(Mutex::new(Vec::new())),
      debate_turn: Arc::new(AtomicU64::new(0)),
      debate_paused: Arc::new(AtomicBool::new(false)),
      debate_modal_visible: Arc::new(AtomicBool::new(false)),
      debate_modal_selected_agent1: Arc::new(Mutex::new(0)),
      debate_modal_selected_agent2: Arc::new(Mutex::new(1)),
      debate_modal_focus: Arc::new(Mutex::new(0)),
      save_path: Arc::new(Mutex::new(None)),
      start_date: Arc::new(Mutex::new(String::new())),
      undo_pending: Arc::new(AtomicBool::new(false)),
    }
  }

  pub fn with_agent(
    settings: crate::config::AgentSettings,
    agents: Vec<crate::config::AgentSettings>,
    quiet: bool,
  ) -> Self {
    let mut state = Self::new();
    state.ui.quiet = quiet;
    *state.voice.lock().unwrap() = settings.voice.clone();
    *state.agent_name.lock().unwrap() = settings.name.clone();
    *state.tts.lock().unwrap() = settings.tts.clone();
    *state.language.lock().unwrap() = settings.language.clone();
    *state.provider.lock().unwrap() = settings.provider.clone();
    *state.baseurl.lock().unwrap() = settings.baseurl.clone();
    *state.model.lock().unwrap() = settings.model.clone();
    *state.system_prompt.lock().unwrap() = settings.system_prompt.clone();
    state.ptt.store(settings.ptt, Ordering::Relaxed);
    *state.sound_threshold_peak.lock().unwrap() = settings.sound_threshold_peak;
    *state.end_silence_ms.lock().unwrap() = settings.end_silence_ms;
    *state.whisper_model_path.lock().unwrap() = settings.whisper_model_path.clone();
    state
      .speed
      .store((settings.voice_speed * 10.0) as u32, Ordering::Relaxed);
    state.agents = Arc::new(agents);
    state
  }

  pub fn reset_conversation(&self) {
    self.conversation_history.lock().unwrap().clear();
    *self.save_path.lock().unwrap() = None;
    *self.start_date.lock().unwrap() = String::new();
  }
}

pub fn get_speed() -> f32 {
  let state = GLOBAL_STATE.get().expect("AppState not initialized");
  state.speed.load(Ordering::Relaxed) as f32 / 10.0
}

pub fn increase_voice_speed() {
  let state = GLOBAL_STATE.get().expect("AppState not initialized");
  let mut cur = state.speed.load(Ordering::Relaxed);
  if cur < 80 {
    cur += 1;
    state.speed.store(cur, Ordering::Relaxed);
  }
}

pub fn decrease_voice_speed() {
  let state = GLOBAL_STATE.get().expect("AppState not initialized");
  let mut cur = state.speed.load(Ordering::Relaxed);
  if cur > 5 {
    cur -= 1;
    state.speed.store(cur, Ordering::Relaxed);
  }
}
