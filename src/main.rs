use clap::Parser;
use crate::util::get_user_home_path;
use cpal::traits::DeviceTrait;
use crossbeam_channel::{bounded, unbounded};
use std::process;
use std::sync::{Arc, OnceLock, atomic::Ordering};
use std::thread::{self, Builder};
use std::time::Instant;
use std::time::Duration;
use crossterm::{terminal::{self, Clear, ClearType, ScrollUp}};

mod assets;
mod audio;
mod config;
mod conversation;
mod keyboard;
mod llm;
mod log;
mod playback;
mod record;
mod state;
mod stt;
mod tts;
mod ui;
mod util;

static START_INSTANT: OnceLock<Instant> = OnceLock::new();

fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {

  let args = crate::config::Args::parse();
  crate::log::set_verbose(args.verbose || false);
  let _ = START_INSTANT.get_or_init(Instant::now);

  // make sure piper phonemes are unpacked
  assets::ensure_piper_espeak_env();
  // make sure the user has the whisper + tts models unpacked
  assets::ensure_assets_env();

  // ---------------------------------------------------
  // setup thread communication channels
  // ---------------------------------------------------
  // broadcast stop signal to all threads
  let (stop_all_tx, stop_all_rx) = unbounded::<()>();
  // channel for utterance audio chunks
  let (tx_utt, rx_utt) = bounded::<audio::AudioChunk>(1);
  // channel for tts phrases
  let (tx_tts, rx_tts) = bounded::<(String, u64)>(1);
  // channel for playback audio chunks
  let (tx_play, rx_play) = bounded::<audio::AudioChunk>(1);
  // channel for ui messages
  let (tx_ui, rx_ui) = bounded::<String>(1);
  log::set_tx_ui_sender(tx_ui.clone());

  if !util::terminal_supported() {
    log::log(
      "error",
      "Terminal does not support colors or emojis. Please use a different terminal. continuing...",
    );
    // do not exit; allow the program to continue for debugging
  }

  // ---------------------------------------------------
  // handle --list-voices
  // ---------------------------------------------------
  if args.list_voices {
    tts::print_voices();
    process::exit(0);
  }
  let _ = terminal::enable_raw_mode();
  env_logger::init();
  whisper_rs::install_logging_hooks();

  // ---------------------------------------------------
  // Load Settings
  // ---------------------------------------------------
  // force creation of default config file if unexisting
  let _ = config::ensure_settings_file();
  let settings_path = get_user_home_path().ok_or("Unable to determine home directory")?
    .join(".ai-mate").join("settings");

  // load and file settings, merge cli args and validate
  let agents = match config::load_settings(&settings_path, &args) {
      Ok(v) => v,
      Err(e) => {
          log::log("error", &format!("Failed to load settings: {}", e));
          process::exit(1);
      }
  };


  let settings = agents
    .iter()
    .find(|a| a.name == args.agent)
    .cloned()
    .unwrap_or_else(|| agents[0].clone());

  // Initialize AppState with the selected voice
   let state: Arc<state::AppState> = Arc::new(state::AppState::with_agent(settings.clone(), agents.clone()));

  state::GLOBAL_STATE.set(state.clone()).unwrap();
  let ui = state.ui.clone();
  let status_line = state.status_line.clone();

  // Spawn UI thread
  let ui_handle = ui::spawn_ui_thread(
      ui.clone(),
      status_line.clone(),
      rx_ui,
  );
  thread::sleep(Duration::from_millis(30));


  // Clones for threads
  let tx_ui_for_keyboard = tx_ui.clone();
  let stop_all_rx_for_keyboard = stop_all_rx.clone();
  let stop_all_rx_for_playback = stop_all_rx.clone();
  let (stop_play_tx, stop_play_rx) = unbounded::<()>(); // stop playback signal

  // Resolve Whisper model path and log it
  let whisper_path = config::resolved_whisper_model_path(&args);
  crate::log::log("info", &format!("Whisper model path: {}", whisper_path));

  let host = cpal::default_host();
  let (in_dev, _in_stream) = audio::pick_input_stream(&host).unwrap_or_else(|msg| {
    log::log("error", &format!("{}", msg));
    process::exit(1)
  });
  let (out_dev, _out_stream) = audio::pick_output_stream(&host).unwrap_or_else(|msg| {
    log::log("error", &format!("{}", msg));
    process::exit(1)
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

  log::log("info", &format!("TTS: {}", settings.tts));
  if args.tts.as_deref() == Some("kokoro") || settings.tts == "kokoro" { tts::start_kokoro_engine()?; }
  log::log("info", &format!("Language: {}", settings.language));
  log::log("info", &format!("TTS voice: {}", settings.voice));
  log::log("info", &format!("LLM provider: {}", settings.provider));
  if settings.provider == "ollama" {
    log::log("info", &format!("ollama base url: {}", settings.baseurl));
  } else {
    log::log(
      "info",
      &format!("llama-server url: {}", settings.baseurl),
    );
  }
  log::log(
    "info",
    &format!(
      "sound_threshold_peak={:.3}  end_silence_ms={}  hangover_ms={}",
      settings.sound_threshold_peak, settings.end_silence_ms, config::HANGOVER_MS_DEFAULT
    ),
  );
  
  let recording_paused = state.recording_paused.clone();
  let recording_paused_for_record = recording_paused.clone();
  if state.ptt.load(Ordering::Relaxed) {
    recording_paused.store(true, Ordering::Relaxed);
  }
  let interrupt_counter = state.interrupt_counter.clone();
  let paused = state.playback.paused.clone();
  let playback_active = state.playback.playback_active.clone();
  let gate_until_ms = state.playback.gate_until_ms.clone();
  let volume = state.playback.volume.clone();
  let conversation_history = state.conversation_history.clone();
  let volume_play = volume.clone();
  let volume_rec = volume.clone();

  // ---------------------------------------------------
  // Thread: TTS
  // ---------------------------------------------------

  let stop_play_tx_for_tts = stop_play_tx.clone();
  let tts_handle = thread::spawn({
  let voice_state = state.voice.clone();
  let out_sample_rate = out_sample_rate.clone();
  let tx_play = tx_play.clone();
  let stop_all_rx = stop_all_rx.clone();
  let interrupt_counter = interrupt_counter.clone();

    move || {
      tts::tts_thread(
        voice_state,
        out_sample_rate,
        tx_play,
        stop_all_rx,
        interrupt_counter,
        rx_tts,
        stop_play_tx_for_tts,
      )
      .unwrap();
    }
  });

  // ---------------------------------------------------
  // Thread: Playback
  // ---------------------------------------------------
  let rx_play_for_playback = rx_play.clone();
  let playback_active_for_play = playback_active.clone();
  let gate_until_ms_for_play = gate_until_ms.clone();
  let paused_for_play = paused.clone();


  // Initialize AppState with the selected voice


  let ui_for_play = ui.clone();
  let volume_play_for_play = volume_play.clone();
  let play_handle = thread::spawn({
    move || {
      playback::playback_thread(
        &START_INSTANT,
        out_dev.clone(),
        out_cfg_supported.clone(),
        out_cfg.clone(),
        rx_play_for_playback,
        stop_play_rx,
        stop_all_rx_for_playback.clone(),
        playback_active_for_play.clone(),
        gate_until_ms_for_play.clone(),
        paused_for_play.clone(),
        out_channels,
        ui_for_play.clone(),
        volume_play_for_play.clone(),
      )
    }
  });

  // ---------------------------------------------------
  // Thread: record
  // ---------------------------------------------------
  let tx_utt_for_rec = tx_utt.clone();
  let playback_active_for_rec = playback_active.clone();
  let gate_until_ms_for_rec = gate_until_ms.clone();
  let interrupt_counter_for_rec = interrupt_counter.clone();
  let stop_all_rx_for_record = stop_all_rx.clone();
  let ui_peak_for_rec = ui.peak.clone();
  let ui_for_rec = ui.clone();
  let volume_rec_for_rec = volume_rec.clone();
  let recording_paused_for_record_for_rec = recording_paused_for_record.clone();
  let tx_ui_for_record = tx_ui.clone();
  let rec_handle = Builder::new()
    .name("record_thread".to_string())
    .stack_size(4 * 1024 * 1024)
    .spawn({
      move || {
        record::record_thread(
          &START_INSTANT,
          in_dev.clone(),
          in_cfg_supported,
          in_cfg,
          tx_utt_for_rec.clone(),
          tx_ui_for_record,
          settings.sound_threshold_peak,
          settings.end_silence_ms,
          playback_active_for_rec.clone(),
          gate_until_ms_for_rec.clone(),
          interrupt_counter_for_rec.clone(),
          stop_all_rx_for_record.clone(),
          ui_peak_for_rec.clone(),
          ui_for_rec.clone(),
          volume_rec_for_rec.clone(),
          recording_paused_for_record_for_rec.clone(),
        )
      }
    })?;

  // ---------------------------------------------------
  // Thread: conversation
  // ---------------------------------------------------
  let rx_utt_for_conv = rx_utt.clone();
  let stop_all_rx_for_conv = stop_all_rx.clone();
  let stop_all_tx_for_conv = stop_all_tx.clone();
  let interrupt_counter_for_conv = interrupt_counter.clone();
  let whisper_path_for_conv = whisper_path.clone();
  let settings_for_conv = settings.clone();
  let ui_for_conv = ui.clone();
  let conversation_history_for_conv = conversation_history.clone();
  let tx_tts_for_conv = tx_tts.clone();
  let conv_handle = thread::spawn({
    move || {
      conversation::conversation_thread(
        rx_utt_for_conv,
        stop_all_rx_for_conv.clone(),
        stop_all_tx_for_conv.clone(),
        interrupt_counter_for_conv.clone(),
        whisper_path_for_conv.clone(),
        settings_for_conv.clone(),
        ui_for_conv.clone(),
        conversation_history_for_conv.clone(),
        tx_ui.clone(),
        tx_tts_for_conv.clone(),
      )
    }
  });

  // ---------------------------------------------------
  // Thread: keyboard
  // ---------------------------------------------------
  let recording_paused_for_key = recording_paused.clone();
  let stop_all_tx_for_key = stop_all_tx.clone();
  let stop_play_tx_for_key = stop_play_tx.clone();
  let key_handle = thread::spawn({
    move || {
      keyboard::keyboard_thread(
        tx_ui_for_keyboard.clone(),
        stop_all_tx_for_key.clone(),
        stop_all_rx_for_keyboard.clone(),
        recording_paused_for_key.clone(),
        stop_play_tx_for_key.clone(),
        interrupt_counter.clone()
      )
    }
  });

  // If running in interactive terminal, block until keyboard thread exits.
  if util::terminal_supported() {
    let _ = key_handle.join();
  }
  let _ = stop_all_tx.try_send(());

  drop(stop_play_tx);
  drop(tx_tts);

  // Wait for all threads to finish
  let _ = rec_handle.join().unwrap();
  let _ = play_handle.join().unwrap();
  let _ = conv_handle.join().unwrap();
  let _ = ui_handle.join().unwrap();
  let _ = tts_handle.join().unwrap();

  Ok(())
}
