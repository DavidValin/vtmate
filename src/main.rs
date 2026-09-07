use crate::util::{get_user_home_path, terminate};
use clap::Parser;
use cpal::traits::DeviceTrait;
use crossbeam_channel::{bounded, unbounded};
use crossterm::terminal::{self};
use std::path::Path;

use ctrlc;
use std::io::IsTerminal;
use std::sync::{Arc, OnceLock, atomic::Ordering};
use std::thread;
use std::time::Duration;
use std::time::Instant;

mod assets;
mod attach;
mod audio;
mod config;
mod conversation;
mod daemon;
mod engine;
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
use crate::conversation::Command;

static START_INSTANT: OnceLock<Instant> = OnceLock::new();

fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
  crate::audio::install_alsa_error_handler();
  crate::audio::ensure_alsa_plugin_dir();

  let mut args = crate::config::Args::parse();

  // Force quiet mode if stdin is not a terminal and input is read from pipe
  let stdin_is_tty = std::io::stdin().is_terminal();
  if args.read_file.as_deref() == Some("-") || args.prompt_file.as_deref() == Some("-") {
    if !stdin_is_tty {
      // in stdin mode keyword poll doesn't work, therefore force quiet mode
      args.quiet = true;
    }
  }
  crate::log::set_verbose(args.verbose || false);
  let _ = START_INSTANT.get_or_init(Instant::now);

  // Ctrl-C handler to set should_exit flag
  let should_exit = Arc::new(std::sync::atomic::AtomicBool::new(false));
  ctrlc::set_handler(move || {
    crate::util::terminate(0);
  })
  .expect("Error setting Ctrl-C handler");

  // ---------------------------------------------------
  // daemon mode: control commands, the daemon itself, auto-attach
  // ---------------------------------------------------
  if args.daemon_status {
    daemon::print_status();
  }
  if args.daemon_stop {
    daemon::stop_running();
  }
  if args.daemon {
    daemon::spawn_detached(&args);
  }
  if args.daemon_foreground {
    daemon::run_foreground(&args);
  }
  let bare_conversation_mode = args.read_file.is_none()
    && args.prompt.is_none()
    && args.prompt_file.is_none()
    && !args.quiet
    && args.debate.is_none()
    && !args.list_voices;
  if bare_conversation_mode {
    if daemon::ipc::probe(Duration::from_millis(300)).is_some() {
      attach::run(&args);
    }
    // A daemon process exists but did not answer in time (busy, still
    // starting): give it a few seconds before running a separate session.
    if let Some(pid) = daemon::paths::read_pid().filter(|p| daemon::paths::pid_alive(*p)) {
      for _ in 0..10 {
        thread::sleep(Duration::from_millis(300));
        if daemon::ipc::probe(Duration::from_millis(500)).is_some() {
          attach::run(&args);
        }
      }
      println!(
        "⚠️  a vtmate daemon (pid {}) is running but does not answer; starting a separate terminal session",
        pid
      );
      thread::sleep(Duration::from_millis(1500));
    }
  }

  // make sure piper phonemes are unpacked
  assets::ensure_piper_espeak_env();
  // make sure the user has the whisper + tts models unpacked
  assets::ensure_assets_env();
  assets::ensure_supersonic2_assets();
  assets::ensure_supertonic_assets();

  // ---------------------------------------------------
  // setup thread communication channels
  // ---------------------------------------------------
  let channels = engine::Channels::new();
  log::set_tx_ui_sender(channels.tx_ui.clone());

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
    util::terminate(0);
  }

  // ---------------------------------------------------
  // quiet mode validation
  // ---------------------------------------------------
  if args.quiet
    && args.prompt.is_none()
    && args.prompt_file.is_none()
    && !(args.read_file.as_deref() == Some("-"))
  {
    println!("❌ Quiet mode requires either one of the next options: -p or -i.\n");
    util::terminate(1);
  }

  // ---------------------------------------------------
  // handle --read-file
  // ---------------------------------------------------
  if let Some(ref filename) = args.read_file {
    // Enable raw mode for keyboard input
    let _ = terminal::enable_raw_mode();

    // Load settings first to get agent configuration
    let _ = config::ensure_settings_file();
    let settings_path = config::resolve_settings_path(&args)?;

    let agents = match config::load_settings(&settings_path, &args) {
      Ok(v) => v,
      Err(e) => {
        crate::log::log("error", &format!("Failed to load settings: {}", e));
        util::terminate(1);
      }
    };

    // Select agent: -a, else [general] selected_agent, else the first one
    let general = config::load_general_settings(&settings_path).unwrap_or_default();
    let settings = match config::select_agent(&agents, args.agent.as_deref(), &general) {
      Ok(a) => a,
      Err(e) => {
        crate::log::log("error", &e);
        util::terminate(1);
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
      settings_path.clone(),
    ));
    state::GLOBAL_STATE.set(app_state.clone()).unwrap();

    // Setup audio output for TTS
    let host = cpal::default_host();
    let (out_dev, _out_stream) = audio::pick_output_stream(&host).unwrap_or_else(|msg| {
      crate::log::log("error", &format!("{}", msg));
      util::terminate(1)
    });

    let out_cfg_supported = out_dev.default_output_config()?;
    let out_cfg: cpal::StreamConfig = out_cfg_supported.clone().into();
    let out_sample_rate = out_cfg.sample_rate.0;
    let out_channels = out_cfg.channels;

    // Setup channels for TTS and playback
    let (tx_play, rx_play) = bounded::<audio::AudioChunk>(1);
    let (tx_tts, rx_tts) = unbounded::<(String, u64, String)>();
    let (tts_done_tx, tts_done_rx) = crossbeam_channel::unbounded();
    let (stop_play_tx, stop_play_rx) = unbounded::<()>();
    // Command channel for undo
    let (tx_cmd_conv, _rx_cmd_conv) = unbounded::<Command>();

    let interrupt_counter = app_state.interrupt_counter.clone();

    // Start TTS thread
    let _tts_handle = thread::spawn({
      let out_sample_rate = out_sample_rate.clone();
      let tx_play = tx_play.clone();
      let interrupt_counter = interrupt_counter.clone();
      let stop_play_tx = stop_play_tx.clone();

      move || {
        tts::tts_thread(
          out_sample_rate,
          tx_play,
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
    let wav_tx = audio::init_wav_writer(&wav_path, 0);
    playback::set_wav_tx(wav_tx.clone());

    let _play_handle = thread::spawn({
      let playback_active = playback_active.clone();
      let gate_until_ms = gate_until_ms.clone();
      let paused = paused.clone();
      let volume = volume.clone();

      move || {
        playback::playback_thread(
          &START_INSTANT,
          out_dev.clone(),
          out_cfg_supported.clone(),
          out_cfg.clone(),
          rx_play,
          stop_play_rx,
          playback_active,
          gate_until_ms,
          paused,
          out_channels,
          ui_state,
          volume,
        )
      }
    });

    // Split content into phrases (by newlines or periods). What TTS gets for
    // each phrase has fenced ``` source code removed: it is displayed but
    // never spoken. Computed up front, in order, so the fence state survives
    // the user jumping between phrases.
    let (phrases, tts_texts): (Vec<String>, Vec<String>) =
      util::split_text_for_tts(&content).into_iter().unzip();

    println!("📖 Reading {} phrases from '{}'", phrases.len(), filename);

    // State for phrase navigation
    let current_phrase = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let tts_paused = Arc::new(std::sync::atomic::AtomicBool::new(false));

    // Channel for triggering display updates
    let (display_update_tx, display_update_rx) = unbounded::<()>();

    // Spawn keyboard handler thread for read-file mode
    let _key_handle = thread::spawn({
      let current_phrase = current_phrase.clone();
      let tts_paused = tts_paused.clone();
      let should_exit = should_exit.clone();
      let interrupt_counter = interrupt_counter.clone();
      let stop_play_tx = stop_play_tx.clone();
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
          Arc::new(std::sync::atomic::AtomicBool::new(false)), // dummy recording_paused
          stop_play_tx,
          interrupt_counter,
          Some(read_file_mode),
          tx_cmd_conv,
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
        terminate(0)
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
        // Text for TTS: code blocks removed, special characters stripped
        let cleaned = tts_texts[idx].clone();
        if cleaned.trim().is_empty() {
          // Nothing left to speak (e.g. a punctuation-only line or source code) -
          // skip without waiting on TTS, otherwise the loop would spin on this index forever.
          if current_phrase.load(Ordering::SeqCst) == idx {
            current_phrase.fetch_add(1, Ordering::SeqCst);
          }
        } else {
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
      } else {
        // Empty phrase (e.g. produced by a stray period on its own line) - skip it.
        if current_phrase.load(Ordering::SeqCst) == idx {
          current_phrase.fetch_add(1, Ordering::SeqCst);
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
    util::terminate(0);
  }

  let _ = terminal::enable_raw_mode();
  env_logger::init();
  whisper_rs::install_logging_hooks();

  // ---------------------------------------------------
  // Load Settings
  // ---------------------------------------------------
  // force creation of default config file if unexisting
  let _ = config::ensure_settings_file();
  let settings_path = config::resolve_settings_path(&args)?;

  // load and file settings, merge cli args and validate
  let agents = match config::load_settings(&settings_path, &args) {
    Ok(v) => v,
    Err(e) => {
      print!("❌ Failed to load settings: {}", e);
      thread::sleep(Duration::from_millis(300));
      util::terminate(1);
    }
  };
  // Select agent: -a, else [general] selected_agent, else the first one
  let general = config::load_general_settings(&settings_path).unwrap_or_default();
  let settings = match config::select_agent(&agents, args.agent.as_deref(), &general) {
    Ok(a) => a,
    Err(e) => {
      print!("❌ {}", e);
      thread::sleep(Duration::from_millis(300));
      util::terminate(1);
    }
  };

  // Initialize AppState with the selected voice
  let state: Arc<state::AppState> = Arc::new(state::AppState::with_agent(
    settings.clone(),
    agents.clone(),
    args.quiet,
    settings_path.clone(),
  ));

  state::GLOBAL_STATE.set(state.clone()).unwrap();

  // If initial prompt provided, process it before starting conversation thread
  // (initial prompt handling moved after TTS thread starts to avoid deadlock)
  let ui = state.ui.clone();
  let mut initial_prompt: Option<String> = None;
  let status_line = state.status_line.clone();
  let conversation_history = state.conversation_history.clone();

  // Start UI thread
  let ui_handle = ui::spawn_ui_thread(
    ui.clone(),
    status_line.clone(),
    channels.rx_ui.clone(),
    conversation_history.clone(),
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

  // ---------------------------------------------------
  // Threads: tts, playback, record, conversation
  // ---------------------------------------------------
  let engine = engine::start(
    &state,
    &settings,
    channels,
    engine::EngineOptions {
      quiet: args.quiet,
      save: args.save,
      initial_prompt: initial_prompt.clone(),
      tx_action: None,
    },
  )?;

  // ---------------------------------------------------
  // Thread: keyboard
  // ---------------------------------------------------
  let key_handle = thread::spawn({
    let tx_ui = engine.tx_ui.clone();
    let recording_paused = state.recording_paused.clone();
    let stop_play_tx = engine.stop_play_tx.clone();
    let interrupt_counter = engine.interrupt_counter.clone();
    let tx_cmd_conv = engine.tx_cmd_conv.clone();
    move || {
      keyboard::keyboard_thread(
        tx_ui,
        recording_paused,
        stop_play_tx,
        interrupt_counter,
        None, // No read-file mode
        tx_cmd_conv,
      );
    }
  });

  // Enable debate mode if requested
  if let Some(ref debate_args) = args.debate {
    if debate_args.len() < 2 {
      crate::log::log("error", "--debate requires at least two agent names");
      util::terminate(1);
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
      util::terminate(1);
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
        util::terminate(1);
      }
    };
    state.debate_enabled.store(true, Ordering::SeqCst);
    *state.debate_subject.lock().unwrap() = subject;
    *state.debate_agents.lock().unwrap() = vec![agent1, agent2];
    state.debate_turn.store(0, Ordering::SeqCst);
  }

  // If running in interactive terminal, block until keyboard thread exits.
  let _ = key_handle.join();

  // Join threads after debate flags set
  for h in engine.handles {
    let _ = h.join();
  }
  let _ = ui_handle.join();

  Ok(())
}
