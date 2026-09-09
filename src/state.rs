// ------------------------------------------------------------------
//  Application state
// ------------------------------------------------------------------

use std::path::PathBuf;
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
  /// True while the agent's voice is actually coming out of the speaker.
  ///
  /// `playback_active` cannot answer that question: audio arrives in chunks
  /// and the queue runs dry between them, so it drops to false many times
  /// during a single phrase. Interrupting by voice in one of those moments
  /// used to be ignored. This one only drops once nothing has been played for
  /// a short while, and nothing waits on it, so phrase timing is unaffected.
  pub speaking: Arc<AtomicBool>,
  /// Frames of silence emitted since the last real sample, feeding `speaking`.
  pub silent_frames: Arc<AtomicU64>,
  pub gate_until_ms: Arc<AtomicU64>,
  pub volume: Arc<Mutex<f32>>,
}

pub static GLOBAL_STATE: OnceLock<Arc<AppState>> = OnceLock::new();

/// What the conversation thread should do with a captured utterance.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum UtteranceKind {
  /// Transcribe and send to the LLM (normal conversation turn).
  #[default]
  Llm,
  /// Transcribe and paste the text at the cursor (daemon dictation).
  Paste,
  /// Transcribe nothing; the utterance is thrown away (daemon reset).
  Discard,
}

/// Snapshotted by the record thread at the moment an utterance is flushed, so
/// the kind and the attached text always travel with the audio they belong to.
#[derive(Clone, Debug, Default)]
pub struct UtteranceMeta {
  pub kind: UtteranceKind,
  /// Selected desktop text to append to the transcript (daemon LLM turns).
  pub attachment: Option<String>,
}

#[derive(Debug)]
pub struct AppState {
  pub conversation_paused: Arc<AtomicBool>,
  pub voice: Arc<Mutex<String>>,
  pub ui: UiState,
  pub speed: AtomicU32,
  pub conversation_history: crate::conversation::ConversationHistory,
  pub agent_name: Arc<Mutex<String>>,
  /// Every agent of the settings file, in file order. Replaced (not only
  /// read) at run time: the settings popup writes the file and puts the
  /// agents it loaded back here.
  pub agents: Arc<Mutex<Vec<crate::config::AgentSettings>>>,
  pub tts: Arc<Mutex<String>>,
  pub language: Arc<Mutex<String>>,
  pub provider: Arc<Mutex<String>>,
  pub baseurl: Arc<Mutex<String>>,
  pub model: Arc<Mutex<String>>,
  pub api_key: Arc<Mutex<String>>,
  pub system_prompt: Arc<Mutex<String>>,
  pub playback: PlaybackState,
  pub status_line: Arc<Mutex<String>>,
  pub interrupt_counter: Arc<AtomicU64>,
  pub recording_paused: Arc<AtomicBool>,
  pub processing_response: Arc<AtomicBool>,
  pub ptt: Arc<AtomicBool>,
  /// Voice detection peak, in thousandths (0.125 is stored as 125). An atomic
  /// because the audio callback reads it on every buffer, and the settings
  /// popup writes it while that callback runs.
  pub sound_threshold_peak: Arc<AtomicU32>,
  /// Silence that ends an utterance in LIVE mode. Read by the same callback.
  pub end_silence_ms: Arc<AtomicU64>,
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
  /// Settings file in use; `selected_agent` is written back here on agent switch.
  pub settings_path: Arc<Mutex<PathBuf>>,
  /// Meta attached to the next utterance the record thread flushes.
  pub utterance_meta: Arc<Mutex<UtteranceMeta>>,
  /// Whisper model loaded (the daemon reports it as "ready").
  pub stt_ready: Arc<AtomicBool>,
  /// Daemon "read selection aloud" job running.
  pub tts_read_active: Arc<AtomicBool>,
  /// Running as (or attached to) the background daemon.
  pub daemon_mode: AtomicBool,
  /// The Ctrl+S settings popup: closed, or the agent list / form being edited.
  pub settings_ui: Arc<Mutex<crate::settings_ui::SettingsUi>>,
  /// `--ptt` as it was given on the command line. It overrides what the file
  /// says, at startup and every time the settings popup reloads it.
  pub ptt_override: Mutex<Option<bool>>,
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
      api_key: Arc::new(Mutex::new(String::new())),
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
      agents: Arc::new(Mutex::new(Vec::new())),
      playback: PlaybackState {
        speaking: Arc::new(AtomicBool::new(false)),
        silent_frames: Arc::new(AtomicU64::new(0)),
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
      sound_threshold_peak: Arc::new(AtomicU32::new(0)),
      end_silence_ms: Arc::new(AtomicU64::new(0)),
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
      settings_path: Arc::new(Mutex::new(PathBuf::new())),
      utterance_meta: Arc::new(Mutex::new(UtteranceMeta::default())),
      stt_ready: Arc::new(AtomicBool::new(false)),
      tts_read_active: Arc::new(AtomicBool::new(false)),
      daemon_mode: AtomicBool::new(false),
      settings_ui: Arc::new(Mutex::new(crate::settings_ui::SettingsUi::default())),
      ptt_override: Mutex::new(None),
    }
  }

  pub fn with_agent(
    settings: crate::config::AgentSettings,
    agents: Vec<crate::config::AgentSettings>,
    quiet: bool,
    settings_path: PathBuf,
  ) -> Self {
    let mut state = Self::new();
    state.ui.quiet = quiet;
    state.apply_agent(&settings);
    state.agents = Arc::new(Mutex::new(agents));
    *state.settings_path.lock().unwrap() = settings_path;
    state
  }

  /// Make `agent` the active one: copy its settings into the live state and
  /// set the PTT gate accordingly. Used at startup and on LEFT/RIGHT switches.
  pub fn apply_agent(&self, agent: &crate::config::AgentSettings) {
    *self.voice.lock().unwrap() = agent.voice.clone();
    *self.agent_name.lock().unwrap() = agent.name.clone();
    *self.tts.lock().unwrap() = agent.tts.clone();
    *self.language.lock().unwrap() = agent.language.clone();
    *self.provider.lock().unwrap() = agent.provider.clone();
    *self.baseurl.lock().unwrap() = agent.baseurl.clone();
    *self.model.lock().unwrap() = agent.model.clone();
    *self.api_key.lock().unwrap() = agent.api_key.clone();
    *self.system_prompt.lock().unwrap() = agent.system_prompt.clone();
    self.ptt.store(agent.ptt, Ordering::Relaxed);
    self.sound_threshold_peak.store(
      (agent.sound_threshold_peak * 1000.0).round().max(0.0) as u32,
      Ordering::Relaxed,
    );
    self
      .end_silence_ms
      .store(agent.end_silence_ms, Ordering::Relaxed);
    *self.whisper_model_path.lock().unwrap() = agent.whisper_model_path.clone();
    self
      .speed
      .store((agent.voice_speed * 10.0) as u32, Ordering::Relaxed);
    // PTT agents start with the mic gated; live agents record straight away.
    self.recording_paused.store(agent.ptt, Ordering::Relaxed);
  }

  /// Voice detection peak the recorder compares buffers against.
  pub fn vad_threshold(&self) -> f32 {
    self.sound_threshold_peak.load(Ordering::Relaxed) as f32 / 1000.0
  }

  /// A copy of the agent list, for callers that only read it.
  pub fn agents(&self) -> Vec<crate::config::AgentSettings> {
    self.agents.lock().unwrap().clone()
  }

  /// Name of the agent at `pos` relative to the active one (wrapping).
  pub fn neighbour_agent(&self, offset: isize) -> Option<crate::config::AgentSettings> {
    let agents = self.agents.lock().unwrap();
    if agents.is_empty() {
      return None;
    }
    let current_name = self.agent_name.lock().unwrap().clone();
    let pos = agents
      .iter()
      .position(|a| a.name == current_name)
      .unwrap_or(0) as isize;
    let len = agents.len() as isize;
    let idx = ((pos + offset) % len + len) % len;
    Some(agents[idx as usize].clone())
  }

  pub fn reset_conversation(&self) {
    self.conversation_history.lock().unwrap().clear();
    *self.save_path.lock().unwrap() = None;
    *self.start_date.lock().unwrap() = String::new();
    crate::html_export::reset();
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
