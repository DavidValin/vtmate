use crate::util::get_user_home_path;
use clap::Parser;
use cpal::traits::DeviceTrait;
use crossbeam_channel::{bounded, unbounded};
use crossterm::terminal::{self};
use std::process;
use std::sync::{Arc, OnceLock, atomic::Ordering};
use std::thread::{self, Builder as ThreadBuilder};
use std::time::Duration;
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
  let (tx_tts, rx_tts) = unbounded::<(String, u64, String)>();
  let (tts_done_tx, tts_done_rx) = crossbeam_channel::bounded(0);

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

  // ---------------------------------------------------
  // handle --read-file
  // ---------------------------------------------------
  if let Some(ref filename) = args.read_file {
    // Load settings first to get agent configuration
    let _ = config::ensure_settings_file();
    let settings_path = get_user_home_path()
      .ok_or("Unable to determine home directory")?
      .join(".ai-mate")
      .join("settings");
    
    let agents = match config::load_settings(&settings_path, &args) {
      Ok(v) => v,
      Err(e) => {
        eprintln!("❌ Failed to load settings: {}", e);
        process::exit(1);
      }
    };
    
    // Select agent: use --agent if specified, otherwise pick first
    let settings = match &args.agent {
      Some(agent_name) => match agents.iter().find(|a| a.name == *agent_name).cloned() {
        Some(a) => a,
        None => {
          eprintln!(
            "❌ Agent '{}' not found. Available agents: {}",
            agent_name,
            agents
              .iter()
              .map(|a| a.name.as_str())
              .collect::<Vec<&str>>()
              .join(", ")
          );
          process::exit(1);
        }
      },
      None => {
        // Pick the first agent if none specified
        agents.first().unwrap().clone()
      }
    };

    // Read the file - try UTF-8 first, then try Latin-1 encoding
    let content = match std::fs::read_to_string(filename) {
      Ok(c) => c,
      Err(_) => {
        // If UTF-8 reading fails, try reading as bytes and detect encoding
        match std::fs::read(filename) {
          Ok(bytes) => {
            // Try to detect encoding - common encodings for Spanish text
            use encoding_rs::*;
            
            // Try UTF-8 first
            if let Ok(s) = std::str::from_utf8(&bytes) {
              s.to_string()
            } else {
              // Try Latin-1 (ISO-8859-1) - common for Spanish
              let (decoded, encoding, had_errors) = WINDOWS_1252.decode(&bytes);
              if !had_errors {
                eprintln!("⚠️  File encoded as Windows-1252/Latin-1, converting to UTF-8");
                decoded.to_string()
              } else {
                // Fall back to lossy UTF-8 conversion
                eprintln!("⚠️  File encoding unknown, using lossy UTF-8 conversion");
                String::from_utf8_lossy(&bytes).to_string()
              }
            }
          }
          Err(e) => {
            eprintln!("❌ Failed to read file '{}': {}", filename, e);
            process::exit(1);
          }
        }
      }
    };

    // Start kokoro engine if needed
    if settings.tts == "kokoro" {
      tts::start_kokoro_engine()?;
    }

    // Initialize global state for TTS thread
    let app_state = Arc::new(state::AppState::with_agent(
      settings.clone(),
      agents.clone(),
    ));
    state::GLOBAL_STATE.set(app_state.clone()).unwrap();

    // Setup audio output for TTS
    let host = cpal::default_host();
    let (out_dev, _out_stream) = audio::pick_output_stream(&host).unwrap_or_else(|msg| {
      eprintln!("❌ {}", msg);
      process::exit(1)
    });
    
    let out_cfg_supported = out_dev.default_output_config()?;
    let out_cfg: cpal::StreamConfig = out_cfg_supported.clone().into();
    let out_sample_rate = out_cfg.sample_rate.0;
    let out_channels = out_cfg.channels;

    // Setup channels for TTS and playback
    let (tx_play, rx_play) = bounded::<audio::AudioChunk>(1);
    let (tx_tts, rx_tts) = unbounded::<(String, u64, String)>();
    let (tts_done_tx, tts_done_rx) = crossbeam_channel::bounded(0);
    let (stop_all_tx, stop_all_rx) = unbounded::<()>();
    let (stop_play_tx, stop_play_rx) = unbounded::<()>();

    let interrupt_counter = app_state.interrupt_counter.clone();

    // Start TTS thread
    let tts_handle = thread::spawn({
      let out_sample_rate = out_sample_rate.clone();
      let tx_play = tx_play.clone();
      let stop_all_rx = stop_all_rx.clone();
      let interrupt_counter = interrupt_counter.clone();
      let stop_play_tx = stop_play_tx.clone();

      move || {
        tts::tts_thread(
          out_sample_rate,
          tx_play,
          stop_all_rx,
          interrupt_counter,
          rx_tts,
          stop_play_tx,
          tts_done_tx,
        )
        .unwrap();
      }
    });

    // Start playback thread
    let playback_active = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let gate_until_ms = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let paused = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let volume = Arc::new(std::sync::Mutex::new(1.0_f32));

    let ui_state = state::UiState {
      thinking: Arc::new(std::sync::atomic::AtomicBool::new(false)),
      playing: Arc::new(std::sync::atomic::AtomicBool::new(false)),
      agent_speaking: Arc::new(std::sync::atomic::AtomicBool::new(false)),
      peak: Arc::new(std::sync::Mutex::new(0.0)),
      spinner_index: 0,
    };

    let play_handle = thread::spawn({
      let playback_active = playback_active.clone();
      let gate_until_ms = gate_until_ms.clone();
      let paused = paused.clone();
      let volume = volume.clone();
      let stop_all_rx = stop_all_rx.clone();

      move || {
        playback::playback_thread(
          &START_INSTANT,
          out_dev.clone(),
          out_cfg_supported.clone(),
          out_cfg.clone(),
          rx_play,
          stop_play_rx,
          stop_all_rx,
          playback_active,
          gate_until_ms,
          paused,
          out_channels,
          ui_state,
          volume,
        )
      }
    });

    // Split content into phrases (by newlines or periods)
    let phrases: Vec<&str> = content
      .lines()
      .flat_map(|line| {
        line.split('.')
          .map(|s| s.trim())
          .filter(|s| !s.is_empty())
      })
      .collect();

    println!("📖 Reading {} phrases from '{}'", phrases.len(), filename);

    // Send phrases to TTS
    for phrase in phrases.iter() {
      if !phrase.is_empty() {
        // Strip special characters before TTS
        let cleaned = util::strip_special_chars(phrase);
        if !cleaned.is_empty() {
          println!("{}", phrase);
          let expected_interrupt = interrupt_counter.load(Ordering::SeqCst);
          tx_tts.send((cleaned, expected_interrupt, settings.voice.clone())).unwrap();
          // Wait for TTS to complete this phrase
          let _ = tts_done_rx.recv();
        }
      }
    }

    println!("✅ All phrases completed");

    // Stop all threads
    drop(tx_tts);
    drop(stop_play_tx);
    let _ = stop_all_tx.send(());
    
    // Wait for threads to finish
    let _ = tts_handle.join();
    let _ = play_handle.join();

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
  let settings_path = get_user_home_path()
    .ok_or("Unable to determine home directory")?
    .join(".ai-mate")
    .join("settings");

  // load and file settings, merge cli args and validate
  let agents = match config::load_settings(&settings_path, &args) {
    Ok(v) => v,
    Err(e) => {
      print!("❌ Failed to load settings: {}", e);
      thread::sleep(Duration::from_millis(300));
      process::exit(1);
    }
  };
  let settings = match &args.agent {
    Some(agent_name) => match agents.iter().find(|a| a.name == *agent_name).cloned() {
      Some(a) => a,
      None => {
        print!(
          "❌ Agent '{}' not found. Available agents: {}",
          agent_name,
          agents
            .iter()
            .map(|a| a.name.as_str())
            .collect::<Vec<&str>>()
            .join(", ")
        );
        thread::sleep(Duration::from_millis(300));
        process::exit(1);
      }
    },
    None => {
      // Pick the first agent if none specified
      agents.first().unwrap().clone()
    }
  };

  // Initialize AppState with the selected voice
  let state: Arc<state::AppState> = Arc::new(state::AppState::with_agent(
    settings.clone(),
    agents.clone(),
  ));

  state::GLOBAL_STATE.set(state.clone()).unwrap();
  let ui = state.ui.clone();
  let status_line = state.status_line.clone();

  // Start UI thread
  let ui_handle = ui::spawn_ui_thread(ui.clone(), status_line.clone(), rx_ui);

  // interrupt counter
  let interrupt_counter = state.interrupt_counter.clone();

  // If debate mode requested via CLI, enable it
  if let Some(debate_args) = args.debate {
    if debate_args.len() < 3 {
      eprintln!("❌ --debate requires at least two agent names and a subject");
      process::exit(1);
    }
    let agent1_name = &debate_args[0];
    let agent2_name = &debate_args[1];
    let subject = debate_args[2..].join(" ");
    let agent1 = agents.iter().find(|a| a.name == *agent1_name).cloned();
    let agent2 = agents.iter().find(|a| a.name == *agent2_name).cloned();
    let (agent1, agent2) = match (agent1, agent2) {
      (Some(a1), Some(a2)) => (a1, a2),
      _ => {
        eprintln!(
          "❌ Agents '{}' or '{}' not found. Available agents: {}",
          agent1_name,
          agent2_name,
          agents
            .iter()
            .map(|a| a.name.as_str())
            .collect::<Vec<&str>>()
            .join(", ")
        );
        process::exit(1);
      }
    };

    // Initialize debate mode in state
    state.debate_enabled.store(true, Ordering::SeqCst);
    *state.debate_subject.lock().unwrap() = subject;
    *state.debate_agents.lock().unwrap() = vec![agent1, agent2];
    state.debate_turn.store(0, Ordering::SeqCst);
  }

  // Clones for threads
  let tx_ui_for_keyboard = tx_ui.clone();
  let stop_all_rx_for_keyboard = stop_all_rx.clone();
  let stop_all_rx_for_playback = stop_all_rx.clone();
  let (stop_play_tx, stop_play_rx) = unbounded::<()>(); // stop playback signal

  // Resolve Whisper model path and log it
  let whisper_path = config::resolved_whisper_model_path(&settings.whisper_model_path);
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

  log::log("info", &format!("Agent: {}", settings.name));
  log::log("info", &format!("TTS: {}", settings.tts));
  log::log("info", &format!("Language: {}", settings.language));
  log::log("info", &format!("TTS voice: {}", settings.voice));
  log::log("info", &format!("LLM provider: {}", settings.provider));

  if settings.tts == "kokoro" {
    tts::start_kokoro_engine()?;
  }

  if settings.provider == "ollama" {
    log::log("info", &format!("ollama base url: {}", settings.baseurl));
  } else {
    log::log("info", &format!("llama-server url: {}", settings.baseurl));
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
    // voice_state not needed; voice passed per message
    let out_sample_rate = out_sample_rate.clone();
    let tx_play = tx_play.clone();
    let stop_all_rx = stop_all_rx.clone();
    let interrupt_counter = interrupt_counter.clone();

    move || {
      tts::tts_thread(
        out_sample_rate,
        tx_play,
        stop_all_rx,
        interrupt_counter,
        rx_tts,
        stop_play_tx_for_tts,
        tts_done_tx,
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
  let rec_handle = ThreadBuilder::new()
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
  let tx_ui_for_conv = tx_ui.clone();
  let tts_done_rx_for_conv = tts_done_rx.clone();

  let conv_handle = thread::spawn(move || {
    conversation::conversation_thread(
      rx_utt_for_conv,
      stop_all_rx_for_conv.clone(),
      stop_all_tx_for_conv.clone(),
      interrupt_counter_for_conv.clone(),
      whisper_path_for_conv.clone(),
      settings_for_conv.clone(),
      ui_for_conv.clone(),
      conversation_history_for_conv.clone(),
      tx_ui_for_conv.clone(),
      tx_tts_for_conv.clone(),
      tts_done_rx_for_conv.clone(),
    )
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
        interrupt_counter.clone(),
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
