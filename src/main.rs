use clap::Parser;
use cpal::traits::DeviceTrait;
use crossbeam_channel::{bounded, unbounded};
use std::process;
use std::sync::{Arc, OnceLock};
use std::thread::{self, Builder};
use std::time::Instant;

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
  env_logger::init();
  whisper_rs::install_logging_hooks();

  if !util::terminal_supported() {
    log::log(
      "error",
      "Terminal does not support colors or emojis. Please use a different terminal. exiting...",
    );
    process::exit(1);
  }
  assets::ensure_piper_espeak_env();
  assets::ensure_assets_env();

  crossterm::execute!(
    std::io::stdout(),
    crossterm::terminal::Clear(crossterm::terminal::ClearType::All)
  )
  .unwrap();
  println!(
    r#"
   █████╗ ██╗      ███╗   ███╗ █████╗ ████████╗███████╗
  ██╔══██╗██║      ████╗ ████║██╔══██╗╚══██╔══╝██╔════╝
  ███████║██║█████╗██╔████╔██║███████║   ██║   █████╗  
  ██╔══██║██║╚════╝██║╚██╔╝██║██╔══██║   ██║   ██╔══╝  
  ██║  ██║██║      ██║ ╚═╝ ██║██║  ██║   ██║   ███████╗
  ╚═╝  ╚═╝╚═╝      ╚═╝     ╚═╝╚═╝  ╚═╝   ╚═╝   ╚══════╝"#
  );

  println!(
    "    \x1b[90mv{}\x1b[0m\n\n\n\n\n",
    env!("CARGO_PKG_VERSION")
  );

  let _ = START_INSTANT.get_or_init(Instant::now);
  let args = crate::config::Args::parse();

  if args.list_voices {
    tts::print_voices();
    process::exit(0);
  }

  // silence external whisper logs
  // unsafe {
  //   whisper_rs::set_log_callback(Some(noop_whisper_log), std::ptr::null_mut());
  // }
  // show external whisper.cpp logs

  crate::log::set_verbose(args.verbose);

  // Resolve Whisper model path and log it
  let whisper_path = args.resolved_whisper_model_path();
  crate::log::log("info", &format!("Whisper model path: {}", whisper_path));

  let vad_thresh: f32 = args.sound_threshold_peak;
  let end_silence_ms: u64 = args.end_silence_ms;

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

  // broadcast stop signal to all threads
  let (stop_all_tx, stop_all_rx) = bounded::<()>(1);
  // channel for recording audio chunks
  let (tx_rec, _rx_rec) = unbounded::<audio::AudioChunk>();
  // channel for playback audio chunks
  let (tx_play, rx_play) = unbounded::<audio::AudioChunk>();
  // channel for utterance audio chunks
  let (tx_utt, rx_utt) = unbounded::<audio::AudioChunk>();

  // Clones for threads
  let rx_play_for_playback = rx_play.clone();
  let stop_all_rx_for_playback = stop_all_rx.clone();
  let stop_all_rx_for_record = stop_all_rx.clone();
  let stop_all_rx_for_keyboard = stop_all_rx.clone();
  let (stop_play_tx, stop_play_rx) = bounded::<()>(2); // stop playback signal

  let available_langs = tts::get_all_available_languages();
  if !available_langs.contains(&args.language.as_str()) {
    log::log(
      "error",
      &format!(
        "Unsupported language '{}'. Next languages are supported: {}",
        args.language,
        available_langs.join(", ")
      ),
    );
    process::exit(1);
  }

  let voice_selected = if let Some(v) = &args.voice {
    v.clone()
  } else {
    if args.tts == "opentts" {
      tts::DEFAULT_OPENTTS_VOICES_PER_LANGUAGE
        .iter()
        .find(|(lang, _)| *lang == args.language.as_str())
        .map(|(_, voice)| (*voice).to_string())
        .unwrap()
    } else {
      tts::DEFAULTKOKORO_VOICES_PER_LANGUAGE
        .iter()
        .find(|(lang, _)| *lang == args.language.as_str())
        .map(|(_, voice)| (*voice).to_string())
        .unwrap()
    }
  };

  let valid_voices: Vec<&str> = tts::get_voices_for(&args.tts, &args.language);
  if valid_voices.is_empty() {
    log::log(
      "error",
      &format!(
        "No available voices for TTS '{}' and language '{}'.",
        args.tts, args.language
      ),
    );
    process::exit(1);
  }
  if !valid_voices.contains(&voice_selected.as_str()) {
    log::log(
      "error",
      &format!(
        "Invalid voice '{}' for TTS '{}' and language '{}'. Available voices: {}",
        voice_selected,
        args.tts,
        args.language,
        valid_voices.join(", ")
      ),
    );
    process::exit(1);
  }
  log::log("info", &format!("TTS system: {}", args.tts));
  if args.tts == "kokoro" {
    tts::start_kokoro_engine()?;
  }
  log::log("info", &format!("Language: {}", args.language));
  log::log("info", &format!("TTS voice: {}", voice_selected));
  log::log("info", &format!("LLM engine: ollama"));
  log::log("info", &format!("ollama base url: {}", args.ollama_url));

  // initialize state after voice_selected
  let state = Arc::new(state::AppState::new_with_voice(voice_selected.clone()));
  let recording_paused = state.recording_paused.clone();
  let recording_paused_for_record = recording_paused.clone();
  state::GLOBAL_STATE.set(state.clone()).unwrap();

  let interrupt_counter = state.interrupt_counter.clone();
  let paused = state.playback.paused.clone();
  let playback_active = state.playback.playback_active.clone();
  let gate_until_ms = state.playback.gate_until_ms.clone();

  let ui = state.ui.clone();
  let volume = state.playback.volume.clone();
  let conversation_history = state.conversation_history.clone();
  let volume_play = volume.clone();
  let volume_rec = volume.clone();
  let status_line = state.status_line.clone();
  let print_lock = state.print_lock.clone();

  // ---- Thread: UI Thread ----
  let ui_handle = ui::spawn_ui_thread(
    ui.clone(),
    stop_all_rx.clone(),
    status_line.clone(),
    print_lock.clone(),
    ui.peak.clone(),
  );

  // ---- Thread: Playback (persistent) ----
  let play_handle = thread::spawn({
    let playback_active = playback_active.clone();
    let gate_until_ms = gate_until_ms.clone();
    let stop_all_rx = stop_all_rx_for_playback.clone();
    let paused = paused.clone();
    let out_dev = out_dev.clone();
    let out_cfg_supported_thread = out_cfg_supported.clone();
    let out_cfg_thread = out_cfg.clone();
    let ui = ui.clone();
    move || {
      playback::playback_thread(
        &START_INSTANT,
        out_dev,
        out_cfg_supported_thread,
        out_cfg_thread,
        rx_play_for_playback,
        stop_play_rx,
        stop_all_rx.clone(),
        playback_active,
        gate_until_ms,
        paused,
        out_channels,
        ui,
        volume_play.clone(),
      )
    }
  });

  // ---- Thread: record ----
  let rec_handle = Builder::new()
    .name("record_thread".to_string())
    .stack_size(4 * 1024 * 1024)
    .spawn({
      let start_instant = &START_INSTANT;
      let device = in_dev.clone();
      let supported = in_cfg_supported;
      let config = in_cfg;
      let tx = tx_rec.clone();
      let tx_utt = tx_utt.clone();
      let vad_thresh = vad_thresh;
      let end_silence_ms = end_silence_ms;
      let playback_active = playback_active.clone();
      let gate_until_ms = gate_until_ms.clone();
      let stop_play_tx = stop_play_tx.clone();
      let interrupt_counter = interrupt_counter.clone();
      let stop_all_rx = stop_all_rx_for_record.clone();
      let peak = ui.peak.clone();
      let ui = ui.clone();
      move || {
        record::record_thread(
          start_instant,
          device,
          supported,
          config,
          tx,
          tx_utt,
          vad_thresh,
          end_silence_ms,
          playback_active,
          gate_until_ms,
          stop_play_tx,
          interrupt_counter,
          stop_all_rx,
          peak,
          ui,
          volume_rec.clone(),
          recording_paused_for_record.clone(),
        )
      }
    })?;

  // ---- Thread: conversation ----
  // clone state for conversation thread to avoid move
  let state_conv = state.clone();
  let conv_handle = thread::spawn({
    let out_sample_rate = out_sample_rate;
    let interrupt_counter = interrupt_counter.clone();
    let args = args.clone();
    let ui = ui.clone();
    let status_line = status_line.clone();
    let print_lock = print_lock.clone();
    let stop_all_tx_conv = stop_all_tx.clone();
    let conversation_history = conversation_history.clone();
    let whisper_path = whisper_path.clone();
    move || {
      conversation::conversation_thread(
        state_conv.voice.clone(),
        rx_utt,
        tx_play.clone(),
        stop_all_rx.clone(),
        stop_all_tx_conv,
        out_sample_rate,
        interrupt_counter,
        whisper_path,
        args,
        ui,
        status_line,
        print_lock,
        conversation_history,
      )
    }
  });

  // ---- Thread: keyboard ----
  // clone state for keyboard thread
  let state_key = state.clone();
  let key_handle = thread::spawn({
    let stop_all_tx = stop_all_tx.clone();
    let stop_all_rx = stop_all_rx_for_keyboard.clone();
    let paused = paused.clone();
    let playback_active = playback_active.clone();
    move || {
      keyboard::keyboard_thread(
        stop_all_tx,
        stop_all_rx,
        paused,
        playback_active,
        recording_paused.clone(),
        state_key.voice.clone(),
        args.tts.clone(),
        args.language.clone(),
      )
    }
  });

  // Print config knobs
  let hangover_ms = util::env_u64("HANGOVER_MS", config::HANGOVER_MS_DEFAULT);
  log::log(
    "info",
    &format!(
      "sound_threshold_peak={:.3}  end_silence_ms={}  hangover_ms={}",
      vad_thresh, end_silence_ms, hangover_ms
    ),
  );

  // Block until keyboard thread exits (Enter/Esc), then propagate stop.
  let _ = key_handle.join();
  let _ = stop_all_tx.try_send(());

  drop(tx_rec);
  drop(stop_play_tx);

  // Wait for all threads to finish
  let _ = rec_handle.join().unwrap();
  let _ = play_handle.join().unwrap();
  let _ = conv_handle.join().unwrap();
  let _ = ui_handle.join().unwrap();

  Ok(())
}
