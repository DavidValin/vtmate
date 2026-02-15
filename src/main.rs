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
  // channel for utterance audio chunks
  let (tx_utt, rx_utt) = unbounded::<audio::AudioChunk>();
  // channel for tts phrases
  let (tx_tts, rx_tts) = bounded::<(String, u64)>(1);
  // channel for playback audio chunks
  let (tx_play, rx_play) = unbounded::<audio::AudioChunk>();

  // Clones for threads
  let rx_play_for_playback = rx_play.clone();
  let stop_all_rx_for_playback = stop_all_rx.clone();
  let stop_all_rx_for_record = stop_all_rx.clone();
  let stop_all_rx_for_keyboard = stop_all_rx.clone();
  let (stop_play_tx, stop_play_rx) = unbounded::<()>(); // stop playback signal

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
  log::log("info", &format!("LLM engine: {}", args.llm));
  if args.llm == "ollama" {
    log::log("info", &format!("ollama base url: {}", args.ollama_url));
  } else {
    log::log("info", &format!("llama-server url: {}", args.llama_server_url));
  }
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

  // ---- Thread: UI Thread ----
  let (tx_ui, rx_ui) = unbounded::<String>();
  let ui_handle = ui::spawn_ui_thread(
    ui.clone(),
    stop_all_rx.clone(),
    status_line.clone(),
    ui.peak.clone(),
    rx_ui,
  );

  // ---- Thread: TTS -----
  let stop_play_tx_for_tts = stop_play_tx.clone();
  let tts_handle = thread::spawn({
    let voice_state = state.voice.clone();
    let out_sample_rate = out_sample_rate.clone();
    let tx_play = tx_play.clone();
    let stop_all_rx = stop_all_rx.clone();
    let interrupt_counter = interrupt_counter.clone();
    let args = args.clone();
    move || {
      tts::tts_thread(
        voice_state,
        out_sample_rate,
        tx_play,
        stop_all_rx,
        interrupt_counter,
        args,
        rx_tts,
        stop_play_tx_for_tts,
      ).unwrap();
    }
  });

  // ---- Thread: Playback (persistent) ----
  let playback_active_for_play = playback_active.clone();
  let gate_until_ms_for_play = gate_until_ms.clone();
  let paused_for_play = paused.clone();
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

  // ---- Thread: record ----
  let tx_utt_for_rec = tx_utt.clone();
  let playback_active_for_rec = playback_active.clone();
  let gate_until_ms_for_rec = gate_until_ms.clone();
  let stop_play_tx_for_rec = stop_play_tx.clone();
  let interrupt_counter_for_rec = interrupt_counter.clone();
  let stop_all_rx_for_record_for_rec = stop_all_rx_for_record.clone();
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
          vad_thresh,
          end_silence_ms,
          playback_active_for_rec.clone(),
          gate_until_ms_for_rec.clone(),
          stop_play_tx_for_rec.clone(),
          interrupt_counter_for_rec.clone(),
          stop_all_rx_for_record_for_rec.clone(),
          ui_peak_for_rec.clone(),
          ui_for_rec.clone(),
          volume_rec_for_rec.clone(),
          recording_paused_for_record_for_rec.clone(),
        )
      }
    })?;

  // ---- Thread: conversation ----
  let rx_utt_for_conv = rx_utt.clone();
  let stop_all_rx_for_conv = stop_all_rx.clone();
  let stop_all_tx_for_conv = stop_all_tx.clone();
  let interrupt_counter_for_conv = interrupt_counter.clone();
  let whisper_path_for_conv = whisper_path.clone();
  let args_for_conv = args.clone();
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
        args_for_conv.clone(),
        ui_for_conv.clone(),
        conversation_history_for_conv.clone(),
        tx_ui.clone(),
        tx_tts_for_conv.clone(),
      )
    }
  });

  // ---- Thread: keyboard ----
  let state_for_key = state.clone();
  let paused_for_key = paused.clone();
  let recording_paused_for_key = recording_paused.clone();
  let voice_for_key = state_for_key.voice.clone();
  let args_tts_for_key = args.tts.clone();
  let args_language_for_key = args.language.clone();
  let stop_all_tx_for_key = stop_all_tx.clone();
  let stop_play_tx_for_key = stop_play_tx.clone();
  let key_handle = thread::spawn({
    move || {
      keyboard::keyboard_thread(
        stop_all_tx_for_key.clone(),
        stop_all_rx_for_keyboard.clone(),
        paused_for_key.clone(),
        recording_paused_for_key.clone(),
        voice_for_key.clone(),
        args_tts_for_key.clone(),
        args_language_for_key.clone(),
        stop_play_tx_for_key.clone(),
        interrupt_counter.clone(),
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
