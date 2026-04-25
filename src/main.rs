use crate::util::get_user_home_path;
use clap::Parser;
use cpal::traits::DeviceTrait;
use crossbeam_channel::{bounded, unbounded};
use crossterm::terminal::{self};
use std::path::{Path, PathBuf};
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
  let args: config::Args = crate::config::Args::parse();
  crate::log::set_verbose(args.verbose || false);
  let _ = START_INSTANT.get_or_init(Instant::now);

  // make sure piper phonemes are unpacked
  assets::ensure_piper_espeak_env();
  // make sure the user has the whisper + tts models unpacked
  assets::ensure_assets_env();
  assets::ensure_supersonic2_assets();

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
  // quiet mode validation
  // ---------------------------------------------------
  if args.quiet && args.prompt.is_none() && args.prompt_file.is_none() {
    println!("❌ Quiet mode requires either one of the next options: -p or -i.\n");
    process::exit(1);
  }

  // ---------------------------------------------------
  // handle --read-file
  // ---------------------------------------------------
  if let Some(ref filename) = args.read_file {
    // Enable raw mode for keyboard input
    let _ = terminal::enable_raw_mode();

    // Load settings first to get agent configuration
    let _ = config::ensure_settings_file();
    let settings_path = if let Some(ref cfg) = args.config {
      // Resolve potential ~ path
      let mut path = PathBuf::from(cfg.as_str());
      if path.starts_with("~") {
        if let Some(home) = get_user_home_path() {
          let rel = path.strip_prefix("~").unwrap_or(&path);
          path = home.join(rel.to_str().unwrap_or(""));
        }
      }
      path
    } else {
      get_user_home_path()
        .ok_or("Unable to determine home directory")?
        .join(".vtmate")
        .join("settings")
    };

    let agents = match config::load_settings(&settings_path, &args) {
      Ok(v) => v,
      Err(e) => {
        crate::log::log("error", &format!("Failed to load settings: {}", e));
        process::exit(1);
      }
    };

    // Select agent: use --a if specified, otherwise pick first
    let settings = match &args.agent {
      Some(agent_name) => match agents.iter().find(|a| a.name == *agent_name).cloned() {
        Some(a) => a,
        None => {
          crate::log::log(
            "error",
            &format!(
              "Agent '{}' not found. Available agents: {}",
              agent_name,
              agents
                .iter()
                .map(|a| a.name.as_str())
                .collect::<Vec<&str>>()
                .join(", ")
            ),
          );
          process::exit(1);
        }
      },
      None => {
        // Pick the first agent if none specified
        agents.first().unwrap().clone()
      }
    };

    // Read the filename or stdin
    let content = util::read_file(filename);

    // Initialize TTS engines only if needed
    let use_supersonic = agents.iter().any(|a| a.tts == "supersonic2");
    let use_kokoro = agents.iter().any(|a| a.tts == "kokoro");
    if use_supersonic {
      tts::supersonic2_tts::start_supersonic_engine()?;
    }
    if use_kokoro {
      tts::kokoro_tts::start_kokoro_engine()?;
    }

    // Initialize global state for TTS thread
    let app_state = Arc::new(state::AppState::with_agent(
      settings.clone(),
      agents.clone(),
      args.quiet,
    ));
    state::GLOBAL_STATE.set(app_state.clone()).unwrap();

    // Setup audio output for TTS
    let host = cpal::default_host();
    let (out_dev, _out_stream) = audio::pick_output_stream(&host).unwrap_or_else(|msg| {
      crate::log::log("error", &format!("{}", msg));
      process::exit(1)
    });

    let out_cfg_supported = out_dev.default_output_config()?;
    let out_cfg: cpal::StreamConfig = out_cfg_supported.clone().into();
    let out_sample_rate = out_cfg.sample_rate.0;
    let out_channels = out_cfg.channels;

    // Setup channels for TTS and playback
    let (tx_play, rx_play) = bounded::<audio::AudioChunk>(1);
    let (tx_tts, rx_tts) = unbounded::<(String, u64, String)>();
    let (tts_done_tx, tts_done_rx) = crossbeam_channel::unbounded();
    let (stop_all_tx, stop_all_rx) = unbounded::<()>();
    let (stop_play_tx, stop_play_rx) = unbounded::<()>();

    let interrupt_counter = app_state.interrupt_counter.clone();

    // Start TTS thread
    let _tts_handle = thread::spawn({
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
      quiet: args.quiet,
    };

    // Setup WAV writer and txt export for read mode
    let home_dir = get_user_home_path().unwrap();
    let read_dir = home_dir.join(".vtmate").join("read-files");
    std::fs::create_dir_all(&read_dir).ok();
    let base_name = Path::new(filename)
      .file_stem()
      .unwrap_or_else(|| std::ffi::OsStr::new("output"))
      .to_string_lossy();
    let wav_path = read_dir.join(format!("{}.wav", base_name));
    let txt_path = read_dir.join(format!("{}.txt", base_name));
    let wav_tx = audio::init_wav_writer(&wav_path);
    playback::set_wav_tx(wav_tx.clone());
    // Write txt after loop

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
    let phrases: Vec<String> = {
      let mut phrases = Vec::new();
      let mut current = String::new();
      for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
          if !current.is_empty() {
            phrases.push(current.trim().to_string());
            current.clear();
          }
          continue;
        }
        // Split line on periods to handle sentence ends
        let mut parts = trimmed.split('.');
        // Handle first part
        let first = parts.next().unwrap();
        if !current.is_empty() {
          current.push(' ');
        }
        current.push_str(first);
        // Any subsequent parts mean we hit a period
        for part in parts {
          // End current phrase at period
          phrases.push(current.trim().to_string());
          current.clear();
          // Start new phrase with remaining part
          if !part.is_empty() {
            current.push_str(part);
          }
        }
      }
      if !current.is_empty() {
        phrases.push(current.trim().to_string());
      }
      phrases
    };

    println!("📖 Reading {} phrases from '{}'", phrases.len(), filename);

    // State for phrase navigation
    let current_phrase = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let tts_paused = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let should_exit = Arc::new(std::sync::atomic::AtomicBool::new(false));

    // Channel for triggering display updates
    let (display_update_tx, display_update_rx) = unbounded::<()>();

    // Spawn keyboard handler thread for read-file mode
    let _key_handle = thread::spawn({
      let current_phrase = current_phrase.clone();
      let tts_paused = tts_paused.clone();
      let should_exit = should_exit.clone();
      let interrupt_counter = interrupt_counter.clone();
      let stop_play_tx = stop_play_tx.clone();
      let stop_all_tx = stop_all_tx.clone();
      let stop_all_rx = stop_all_rx.clone();
      let display_update_tx = display_update_tx.clone();
      let phrases_len = phrases.len();
      let (tx_ui_dummy, _rx_ui_dummy) = bounded::<String>(1); // Dummy channel for read-file mode

      move || {
        let read_file_mode = keyboard::ReadFileMode {
          current_phrase,
          tts_paused,
          should_exit,
          display_update_tx,
          phrases_len,
        };

        keyboard::keyboard_thread(
          tx_ui_dummy,
          stop_all_tx,
          stop_all_rx,
          Arc::new(std::sync::atomic::AtomicBool::new(false)), // dummy recording_paused
          stop_play_tx,
          interrupt_counter,
          Some(read_file_mode),
        )
      }
    });

    // Clear screen and prepare for phrase display
    use crossterm::{cursor, execute, terminal as term};
    use std::io::{Write, stdout};
    let mut out = stdout();
    execute!(
      out,
      term::Clear(term::ClearType::All),
      cursor::MoveTo(0, 0),
      cursor::Hide
    )
    .unwrap();

    // Track which phrases have been completed
    let displayed_phrases = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));

    // Helper function to update display
    let update_display =
      |out: &mut std::io::Stdout, completed: &[String], current: Option<&str>| {
        execute!(out, term::Clear(term::ClearType::All), cursor::MoveTo(0, 0)).unwrap();

        // Show all completed phrases (unhighlighted)
        for phrase in completed {
          execute!(out, cursor::MoveToColumn(0)).unwrap();
          println!("{}", phrase);
        }

        // Show current phrase with highlight (yellow background, black text)
        if let Some(curr) = current {
          execute!(out, cursor::MoveToColumn(0)).unwrap();
          println!("\x1b[33m{}\x1b[0m", curr);
        }

        out.flush().unwrap();
      };

    let mut last_idx = 0;

    // Main TTS loop
    loop {
      if should_exit.load(Ordering::SeqCst) {
        break;
      }

      let idx = current_phrase.load(Ordering::SeqCst);

      if idx >= phrases.len() {
        break;
      }

      // Handle keyboard navigation - user jumped to a different phrase
      if idx != last_idx {
        // Clear the display and rebuild from scratch
        let mut displayed = displayed_phrases.lock().unwrap();
        displayed.clear();
        // Add all phrases before the current index
        for i in 0..idx {
          displayed.push(phrases[i].clone());
        }
        drop(displayed);
      }

      // Always update last_idx to current
      last_idx = idx;

      // Check for display update requests from keyboard navigation
      while display_update_rx.try_recv().is_ok() {
        // Consume all pending updates
      }

      if tts_paused.load(Ordering::SeqCst) {
        thread::sleep(Duration::from_millis(100));
        continue;
      }

      let phrase = &phrases[idx];

      if !phrase.is_empty() {
        // Strip special characters before TTS
        let cleaned = crate::util::strip_special_chars(phrase);
        if !cleaned.is_empty() {
          // Show this phrase as current (highlighted) - THIS IS WHEN IT STARTS PLAYING
          let displayed = displayed_phrases.lock().unwrap();
          update_display(&mut out, &displayed, Some(phrase));
          drop(displayed);

          let expected_interrupt = interrupt_counter.load(Ordering::SeqCst);
          tx_tts
            .send((cleaned, expected_interrupt, settings.voice.clone()))
            .unwrap();

          // Wait for TTS synthesis to complete or navigation
          let mut navigated_away = false;
          loop {
            match tts_done_rx.try_recv() {
              Ok(_) => break,
              Err(_) => {
                // Check if user navigated away
                if current_phrase.load(Ordering::SeqCst) != idx {
                  // User navigated, break out
                  navigated_away = true;
                  break;
                }
                if should_exit.load(Ordering::SeqCst) {
                  break;
                }
                thread::sleep(Duration::from_millis(50));
              }
            }
          }

          // Check if we navigated away before continuing
          if navigated_away {
            continue; // Skip to next iteration
          }

          // Wait a bit to ensure playback has started
          thread::sleep(Duration::from_millis(100));

          // NOW wait for playback to finish - PHRASE STAYS HIGHLIGHTED DURING PLAYBACK
          while playback_active.load(Ordering::Relaxed) {
            // Check if user navigated away
            if current_phrase.load(Ordering::SeqCst) != idx {
              navigated_away = true;
              break;
            }
            if should_exit.load(Ordering::SeqCst) {
              break;
            }
            thread::sleep(Duration::from_millis(50));
          }

          // Check if we navigated away before marking as completed
          if navigated_away {
            continue; // Skip to next iteration
          }

          // Add extra delay to ensure audio has fully played
          thread::sleep(Duration::from_millis(100));

          // NOW that playback is done, move phrase from current to completed (unhighlighted)
          let mut displayed = displayed_phrases.lock().unwrap();
          if !displayed.contains(phrase) {
            displayed.push(phrase.clone());
          }
          // Update display immediately to show it as completed (no highlight)
          update_display(&mut out, &displayed, None);
          drop(displayed);

          // Only auto-advance if we didn't navigate
          // Auto-advance only if we weren't interrupted or navigated away
          let start_idx = idx;
          // ... existing code remains ...
          // After playback finished
          if current_phrase.load(Ordering::SeqCst) == start_idx {
            current_phrase.fetch_add(1, Ordering::SeqCst);
          }
        }
      }
    }

    print!("\r✅ All phrases completed\n\r");
    // Export txt content
    if let Err(e) = audio::write_txt(&txt_path, &content) {
      eprintln!("Failed to write txt: {}", e);
    }

    execute!(out, cursor::Show).unwrap();
    let _ = terminal::disable_raw_mode();
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
  let settings_path = if let Some(ref cfg) = args.config {
    // Resolve potential ~ path
    let mut path = PathBuf::from(cfg.as_str());
    if path.starts_with("~") {
      if let Some(home) = get_user_home_path() {
        let rel = path.strip_prefix("~").unwrap_or(&path);
        path = home.join(rel.to_str().unwrap_or(""));
      }
    }
    path
  } else {
    get_user_home_path()
      .ok_or("Unable to determine home directory")?
      .join(".vtmate")
      .join("settings")
  };

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
    args.quiet,
  ));

  state::GLOBAL_STATE.set(state.clone()).unwrap();

  // If initial prompt provided, process it before starting conversation thread
  // (initial prompt handling moved after TTS thread starts to avoid deadlock)
  let ui = state.ui.clone();
  let mut initial_prompt: Option<String> = None;
  let status_line = state.status_line.clone();

  // Start UI thread
  let ui_handle = ui::spawn_ui_thread(ui.clone(), status_line.clone(), rx_ui);

  // interrupt counter
  let _interrupt_counter = state.interrupt_counter.clone();

  // (Debate logic removed – will be placed after prompt handling)

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

  // ---------------------------------------------------
  // Handle --prompt-file <file_name|-> / -i <file_name|->
  // ---------------------------------------------------
  if let Some(prompt_file) = args.prompt_file.clone() {
    let prompt_from_file = util::read_file(&prompt_file);
    initial_prompt = Some(prompt_from_file.clone());
  }

  // ---------------------------------------------------
  // Handle --prompt-text <prompt> / -p <prompt>
  // ---------------------------------------------------
  if let Some(prompt_text) = args.prompt.clone() {
    initial_prompt = Some(prompt_text);
  }

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
  let rec_handle = if !args.quiet {
    ThreadBuilder::new()
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
      })?
  } else {
    // Dummy thread when quiet mode: do nothing
    thread::spawn(|| Ok::<(), Box<dyn std::error::Error + Send + Sync>>(()))
  };

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

  let init_prompt_for_conv = initial_prompt.clone();
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
      init_prompt_for_conv,
      args.quiet,
      args.save,
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
        None, // No read-file mode
      )
    }
  });

  // Enable debate mode if requested
  if let Some(ref debate_args) = args.debate {
    if debate_args.len() < 2 {
      crate::log::log("error", "--debate requires at least two agent names");
      process::exit(1);
    }
    let agent1_name = &debate_args[0];
    let agent2_name = &debate_args[1];
    let subject = if debate_args.len() >= 3 {
      debate_args[2..].join(" ")
    } else if let Some(ref subj) = initial_prompt {
      subj.clone()
    } else {
      crate::log::log(
        "error",
        "--debate requires a subject when no prompt is provided",
      );
      process::exit(1);
    };
    let agent1 = agents.iter().find(|a| a.name == *agent1_name).cloned();
    let agent2 = agents.iter().find(|a| a.name == *agent2_name).cloned();
    let (agent1, agent2) = match (agent1, agent2) {
      (Some(a1), Some(a2)) => (a1, a2),
      _ => {
        crate::log::log(
          "error",
          &format!(
            "Agents '{}' or '{}' not found. Available agents: {}",
            agent1_name,
            agent2_name,
            agents
              .iter()
              .map(|a| a.name.as_str())
              .collect::<Vec<&str>>()
              .join(", ")
          ),
        );
        process::exit(1);
      }
    };
    state.debate_enabled.store(true, Ordering::SeqCst);
    *state.debate_subject.lock().unwrap() = subject;
    *state.debate_agents.lock().unwrap() = vec![agent1, agent2];
    state.debate_turn.store(0, Ordering::SeqCst);
  }

  // If running in interactive terminal, block until keyboard thread exits.
  if util::terminal_supported() {
    let _ = key_handle.join();
  }

  // Join threads after debate flags set
  let _ = rec_handle.join().unwrap();
  let _ = play_handle.join().unwrap();
  let _ = conv_handle.join().unwrap();
  let _ = ui_handle.join().unwrap();
  let _ = tts_handle.join().unwrap();

  drop(stop_play_tx);
  // drop(tx_tts);

  Ok(())
}
