// ------------------------------------------------------------------
//  Engine: the audio / STT / LLM / TTS thread set shared by the
//  terminal conversation mode and the background daemon.
// ------------------------------------------------------------------

use crate::START_INSTANT;
use crate::audio;
use crate::config;
use crate::conversation::{self, Command, DaemonAction};
use crate::log;
use crate::playback;
use crate::record;
use crate::state::AppState;
use crate::tts;
use crate::util;
use cpal::traits::DeviceTrait;
use crossbeam_channel::{Receiver, Sender, bounded, unbounded};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread::{self, Builder as ThreadBuilder, JoinHandle};

// API
// ------------------------------------------------------------------

/// Every channel the threads talk over. Created once at startup so the log
/// sink can be attached to `tx_ui` before anything else runs.
pub struct Channels {
  /// utterance -> conversation
  pub tx_utt: Sender<audio::Utterance>,
  pub rx_utt: Receiver<audio::Utterance>,
  /// (text, expected interrupt epoch, voice) -> tts
  pub tx_tts: Sender<(String, u64, String)>,
  pub rx_tts: Receiver<(String, u64, String)>,
  /// tts -> whoever waits for a phrase to be synthesized (rendezvous)
  pub tts_done_tx: Sender<()>,
  pub tts_done_rx: Receiver<()>,
  /// synthesized audio -> playback
  pub tx_play: Sender<audio::AudioChunk>,
  pub rx_play: Receiver<audio::AudioChunk>,
  /// "type|payload" lines -> ui
  pub tx_ui: Sender<String>,
  pub rx_ui: Receiver<String>,
  /// stop playback signal
  pub stop_play_tx: Sender<()>,
  pub stop_play_rx: Receiver<()>,
  /// keyboard -> conversation commands (undo)
  pub tx_cmd_conv: Sender<Command>,
  pub rx_cmd_conv: Receiver<Command>,
}

impl Channels {
  pub fn new() -> Self {
    let (tx_utt, rx_utt) = bounded::<audio::Utterance>(1);
    let (tx_tts, rx_tts) = unbounded::<(String, u64, String)>();
    let (tts_done_tx, tts_done_rx) = bounded::<()>(0);
    let (tx_play, rx_play) = bounded::<audio::AudioChunk>(1);
    let (tx_ui, rx_ui) = bounded::<String>(1);
    let (stop_play_tx, stop_play_rx) = unbounded::<()>();
    let (tx_cmd_conv, rx_cmd_conv) = unbounded::<Command>();
    Self {
      tx_utt,
      rx_utt,
      tx_tts,
      rx_tts,
      tts_done_tx,
      tts_done_rx,
      tx_play,
      rx_play,
      tx_ui,
      rx_ui,
      stop_play_tx,
      stop_play_rx,
      tx_cmd_conv,
      rx_cmd_conv,
    }
  }
}

pub struct EngineOptions {
  pub quiet: bool,
  pub save: bool,
  pub initial_prompt: Option<String>,
  /// Where the conversation thread sends daemon work (paste requests).
  pub tx_action: Option<Sender<DaemonAction>>,
}

/// Handles to a running engine: the senders the key handler / daemon
/// controller need, plus the thread handles.
pub struct Engine {
  pub tx_ui: Sender<String>,
  pub tx_utt: Sender<audio::Utterance>,
  pub tx_tts: Sender<(String, u64, String)>,
  pub tts_done_rx: Receiver<()>,
  pub stop_play_tx: Sender<()>,
  pub tx_cmd_conv: Sender<Command>,
  pub interrupt_counter: Arc<AtomicU64>,
  pub handles: Vec<JoinHandle<()>>,
}

/// Pick the audio devices and spawn the tts, playback, record and
/// conversation threads. `GLOBAL_STATE` must already be set to `state`.
/// The caller keeps (or has already handed out) `rx_ui`; everything else in
/// `ch` is consumed here.
pub fn start(
  state: &Arc<AppState>,
  settings: &config::AgentSettings,
  ch: Channels,
  opts: EngineOptions,
) -> Result<Engine, Box<dyn std::error::Error + Send + Sync>> {
  // Resolve Whisper model path and log it
  let whisper_path = config::resolved_whisper_model_path(&settings.whisper_model_path);
  log::log("info", &format!("Whisper model path: {}", whisper_path));

  let host = cpal::default_host();
  let (in_dev, _in_stream) = audio::pick_input_stream(&host).unwrap_or_else(|msg| {
    log::log("error", &format!("{}", msg));
    util::terminate(1)
  });
  let (out_dev, _out_stream) = audio::pick_output_stream(&host).unwrap_or_else(|msg| {
    log::log("error", &format!("{}", msg));
    util::terminate(1)
  });
  log::log(
    "info",
    &format!(
      "Input device:  {}",
      in_dev.name().unwrap_or("<unknown>".into())
    ),
  );
  log::log(
    "info",
    &format!(
      "Output device: {}",
      out_dev.name().unwrap_or("<unknown>".into())
    ),
  );

  let out_cfg_supported = out_dev.default_output_config()?;
  let out_cfg: cpal::StreamConfig = out_cfg_supported.clone().into();
  let out_sample_rate = out_cfg.sample_rate.0;
  let out_channels = out_cfg.channels;

  let in_cfg_supported = config::pick_input_config(&in_dev, out_sample_rate)?;
  let in_cfg: cpal::StreamConfig = in_cfg_supported.clone().into();

  log::log(
    "info",
    &format!(
      "Picked Input:  {} ch @ {} Hz ({:?})",
      in_cfg.channels,
      in_cfg.sample_rate.0,
      in_cfg_supported.sample_format()
    ),
  );
  log::log(
    "info",
    &format!(
      "Picked Output: {} ch @ {} Hz ({:?})",
      out_cfg.channels,
      out_cfg.sample_rate.0,
      out_cfg_supported.sample_format()
    ),
  );
  log::log(
    "info",
    &format!("Playback stream SR (truth): {}", out_sample_rate),
  );

  log::log("info", &format!("Agent: {}", settings.name));
  log::log("info", &format!("TTS: {}", settings.tts));
  log::log("info", &format!("Language: {}", settings.language));
  log::log("info", &format!("TTS voice: {}", settings.voice));
  log::log("info", &format!("LLM provider: {}", settings.provider));
  if settings.baseurl.trim().is_empty() {
    log::log("info", "LLM base url: (provider default)");
  } else {
    log::log("info", &format!("LLM base url: {}", settings.baseurl));
  }
  log::log(
    "info",
    &format!(
      "sound_threshold_peak={:.3}  end_silence_ms={}  hangover_ms={}",
      settings.sound_threshold_peak,
      settings.end_silence_ms,
      config::HANGOVER_MS_DEFAULT
    ),
  );

  let ui = state.ui.clone();
  let recording_paused = state.recording_paused.clone();
  if state.ptt.load(Ordering::Relaxed) {
    recording_paused.store(true, Ordering::Relaxed);
  }
  let interrupt_counter = state.interrupt_counter.clone();
  let paused = state.playback.paused.clone();
  let playback_active = state.playback.playback_active.clone();
  let gate_until_ms = state.playback.gate_until_ms.clone();
  let conversation_history = state.conversation_history.clone();
  let volume = state.playback.volume.clone();

  let Channels {
    tx_utt,
    rx_utt,
    tx_tts,
    rx_tts,
    tts_done_tx,
    tts_done_rx,
    tx_play,
    rx_play,
    tx_ui,
    rx_ui: _rx_ui,
    stop_play_tx,
    stop_play_rx,
    tx_cmd_conv,
    rx_cmd_conv,
  } = ch;

  let mut handles: Vec<JoinHandle<()>> = Vec::new();

  // ---------------------------------------------------
  // Thread: TTS
  // ---------------------------------------------------
  handles.push(thread::spawn({
    let tx_play = tx_play.clone();
    let interrupt_counter = interrupt_counter.clone();
    let stop_play_tx = stop_play_tx.clone();
    move || {
      if let Err(e) = tts::tts_thread(
        out_sample_rate,
        tx_play,
        interrupt_counter,
        rx_tts,
        stop_play_tx,
        tts_done_tx,
      ) {
        log::log("error", &format!("tts thread ended: {}", e));
      }
    }
  }));

  // ---------------------------------------------------
  // Thread: Playback
  // ---------------------------------------------------
  handles.push(thread::spawn({
    let playback_active = playback_active.clone();
    let gate_until_ms = gate_until_ms.clone();
    let paused = paused.clone();
    let ui = ui.clone();
    let volume = volume.clone();
    move || {
      if let Err(e) = playback::playback_thread(
        &START_INSTANT,
        out_dev,
        out_cfg_supported,
        out_cfg,
        rx_play,
        stop_play_rx,
        playback_active,
        gate_until_ms,
        paused,
        out_channels,
        ui,
        volume,
      ) {
        log::log("error", &format!("playback thread ended: {}", e));
      }
    }
  }));

  // ---------------------------------------------------
  // Thread: record (not in quiet mode: no microphone there)
  // ---------------------------------------------------
  if !opts.quiet {
    let tx_utt = tx_utt.clone();
    let tx_ui = tx_ui.clone();
    let playback_active = playback_active.clone();
    let gate_until_ms = gate_until_ms.clone();
    let interrupt_counter = interrupt_counter.clone();
    let peak = ui.peak.clone();
    let ui = ui.clone();
    let volume = volume.clone();
    let recording_paused = recording_paused.clone();
    let utterance_meta = state.utterance_meta.clone();
    let vad_thresh = settings.sound_threshold_peak;
    let end_silence_ms = settings.end_silence_ms;
    handles.push(
      ThreadBuilder::new()
        .name("record_thread".to_string())
        .stack_size(4 * 1024 * 1024)
        .spawn(move || {
          if let Err(e) = record::record_thread(
            &START_INSTANT,
            in_dev,
            in_cfg_supported,
            in_cfg,
            tx_utt,
            tx_ui,
            vad_thresh,
            end_silence_ms,
            playback_active,
            gate_until_ms,
            interrupt_counter,
            peak,
            ui,
            volume,
            recording_paused,
            utterance_meta,
          ) {
            log::log("error", &format!("record thread ended: {}", e));
          }
        })?,
    );
  }

  // ---------------------------------------------------
  // Thread: conversation
  // ---------------------------------------------------
  handles.push(thread::spawn({
    let interrupt_counter = interrupt_counter.clone();
    let settings = settings.clone();
    let ui = ui.clone();
    let tx_tts = tx_tts.clone();
    let tx_ui = tx_ui.clone();
    let tts_done_rx = tts_done_rx.clone();
    let stop_play_tx = stop_play_tx.clone();
    let initial_prompt = opts.initial_prompt.clone();
    let tx_action = opts.tx_action.clone();
    let quiet = opts.quiet;
    let save = opts.save;
    move || {
      if let Err(e) = conversation::conversation_thread(
        rx_utt,
        interrupt_counter,
        whisper_path,
        settings,
        ui,
        conversation_history,
        tx_ui,
        tx_tts,
        tts_done_rx,
        stop_play_tx,
        rx_cmd_conv,
        initial_prompt,
        quiet,
        save,
        tx_action,
      ) {
        log::log("error", &format!("conversation thread ended: {}", e));
      }
    }
  }));

  Ok(Engine {
    tx_ui,
    tx_utt,
    tx_tts,
    tts_done_rx,
    stop_play_tx,
    tx_cmd_conv,
    interrupt_counter,
    handles,
  })
}
