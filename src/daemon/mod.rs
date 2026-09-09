// ------------------------------------------------------------------
//  Daemon mode: detached background vtmate driven by global shortcuts
// ------------------------------------------------------------------

pub mod controller;
pub mod desktop;
pub mod hotkeys;
pub mod ipc;
pub mod paths;
pub mod server;

use crate::config::{self, Args};
use crate::engine;
use crate::state::{self, GLOBAL_STATE};
use crate::util;
use crossbeam_channel::unbounded;
use global_hotkey::GlobalHotKeyManager;
use interprocess::local_socket::ListenerOptions;
use interprocess::local_socket::prelude::*;
use ipc::{ClientMsg, ServerMsg, StatusView};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

// API
// ------------------------------------------------------------------

/// Exit code of the child when a shortcut could not be grabbed.
pub const EXIT_HOTKEYS_TAKEN: i32 = 3;
/// Exit code of the child when the settings could not be loaded.
pub const EXIT_BAD_SETTINGS: i32 = 2;

const PROBE_TIMEOUT: Duration = Duration::from_millis(500);

static STOPPING: AtomicBool = AtomicBool::new(false);

/// `vtmate --daemon`: validate the settings here (we have a terminal), then
/// start the detached child and wait until it answers on the socket.
pub fn spawn_detached(args: &Args) -> ! {
  if let Some(status) = ipc::probe(PROBE_TIMEOUT) {
    println!(
      "vtmate daemon already running (pid {}, agent '{}'). Run `vtmate` to attach.",
      status.pid, status.agent
    );
    plain_exit(1);
  }
  paths::remove_runtime_files();

  // Settings errors print here instead of in the child (whose output is discarded).
  let _ = config::ensure_settings_file();
  let settings_path = match config::resolve_settings_path(args) {
    Ok(p) => p,
    Err(e) => {
      println!("❌ {}", e);
      plain_exit(1);
    }
  };
  let mut check_args = args.clone();
  check_args.ptt = Some(true);
  let agents = match config::load_settings(&settings_path, &check_args) {
    Ok(a) => a,
    Err(e) => {
      println!("❌ Failed to load settings: {}", e);
      plain_exit(1);
    }
  };
  let general = config::load_general_settings(&settings_path).unwrap_or_default();
  if let Err(e) = config::select_agent(&agents, args.agent.as_deref(), &general) {
    println!("❌ {}", e);
    plain_exit(1);
  }
  let daemon_settings = match config::load_daemon_settings(&settings_path) {
    Ok(d) => d,
    Err(e) => {
      println!("❌ {}", e);
      plain_exit(1);
    }
  };

  let exe = match std::env::current_exe() {
    Ok(p) => p,
    Err(e) => {
      println!("❌ cannot locate the vtmate executable: {}", e);
      plain_exit(1);
    }
  };
  let mut cmd = Command::new(exe);
  cmd.arg("--daemon-foreground");
  if let Some(cfg) = &args.config {
    cmd.arg("-c").arg(cfg);
  }
  if let Some(agent) = &args.agent {
    cmd.arg("-a").arg(agent);
  }
  if args.verbose {
    cmd.arg("--verbose");
  }
  if args.save {
    cmd.arg("-s");
  }
  cmd
    .stdin(Stdio::null())
    .stdout(Stdio::null())
    .stderr(Stdio::null());
  #[cfg(unix)]
  {
    use std::os::unix::process::CommandExt;
    // SAFETY: setsid is async-signal-safe and only touches this process.
    unsafe {
      cmd.pre_exec(|| {
        libc::setsid();
        Ok(())
      });
    }
  }
  #[cfg(windows)]
  {
    use std::os::windows::process::CommandExt;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
  }
  let mut child = match cmd.spawn() {
    Ok(c) => c,
    Err(e) => {
      println!("❌ cannot start the daemon: {}", e);
      plain_exit(1);
    }
  };

  print!("⏳ starting vtmate daemon...");
  let _ = std::io::Write::flush(&mut std::io::stdout());
  let deadline = Instant::now() + Duration::from_secs(60);
  loop {
    if let Ok(Some(status)) = child.try_wait() {
      println!("\r\x1b[K");
      match std::fs::read_to_string(paths::start_error_file()) {
        Ok(err) if status.code() == Some(EXIT_HOTKEYS_TAKEN) => {
          println!(
            "❌ vtmate daemon not started: these shortcuts are already taken by another application:"
          );
          for line in err.lines().filter(|l| !l.trim().is_empty()) {
            println!("   {}", line);
          }
          println!(
            "   Change them in {} under [daemon].",
            settings_path.display()
          );
        }
        Ok(err) => {
          println!("❌ vtmate daemon not started:");
          for line in err.lines().filter(|l| !l.trim().is_empty()) {
            println!("   {}", line);
          }
        }
        Err(_) => {
          println!(
            "❌ vtmate daemon failed to start (exit {}), see {}",
            status.code().unwrap_or(-1),
            paths::log_file().display()
          );
        }
      }
      let _ = std::fs::remove_file(paths::start_error_file());
      plain_exit(1);
    }
    if let Some(status) = ipc::probe(Duration::from_millis(300)) {
      println!(
        "\r\x1b[K✅ vtmate daemon started (pid {}), agent '{}'",
        status.pid, status.agent
      );
      for (setting, combo) in daemon_settings.combos() {
        println!("   {:<36} {}", setting, combo);
      }
      println!("   Run `vtmate` to attach, `vtmate --daemon-stop` to stop.");
      plain_exit(0);
    }
    if Instant::now() >= deadline {
      println!(
        "\r\x1b[K⚠️  daemon did not answer within 60s; it may still be loading models. See {}",
        paths::log_file().display()
      );
      plain_exit(1);
    }
    thread::sleep(Duration::from_millis(100));
  }
}

/// `vtmate --daemon-stop`
pub fn stop_running() -> ! {
  let Some(status) = ipc::probe(PROBE_TIMEOUT) else {
    println!("vtmate daemon is not running.");
    plain_exit(0);
  };
  let pid = status.pid;
  if let Ok(stream) = ipc::connect() {
    let (reader, mut writer) = stream.split();
    let mut reader = std::io::BufReader::new(reader);
    if ipc::write_msg(&mut writer, &ClientMsg::Stop).is_ok() {
      let _ = ipc::read_msg::<_, ServerMsg>(&mut reader);
    }
  }
  let deadline = Instant::now() + Duration::from_secs(5);
  while paths::pid_alive(pid) && Instant::now() < deadline {
    thread::sleep(Duration::from_millis(100));
  }
  if paths::pid_alive(pid) {
    println!("⚠️  vtmate daemon (pid {}) did not stop in time.", pid);
    plain_exit(1);
  }
  paths::remove_runtime_files();
  println!("✅ vtmate daemon stopped (pid {}).", pid);
  plain_exit(0);
}

/// `vtmate --daemon-status`
pub fn print_status() -> ! {
  match ipc::probe(PROBE_TIMEOUT) {
    Some(s) => {
      print_status_view(&s);
      plain_exit(0);
    }
    None => {
      println!("vtmate daemon is not running.");
      plain_exit(1);
    }
  }
}

pub fn print_status_view(s: &StatusView) {
  println!(
    "vtmate daemon running: pid {}, version {}, {}",
    s.pid,
    s.version,
    if s.ready { "ready" } else { "loading models" }
  );
  println!("   agent:   {}", s.agent);
  println!("   clients: {}", s.clients);
  println!("   uptime:  {}s", s.uptime_s);
  for (setting, combo) in &s.hotkeys {
    println!("   {:<36} {}", setting, combo);
  }
}

/// Orderly stop from any thread (IPC Stop, signal). Never blocks the caller.
pub fn request_stop() {
  if STOPPING.swap(true, Ordering::SeqCst) {
    return;
  }
  thread::spawn(|| {
    crate::log::log("info", "daemon stopping");
    on_exit();
    std::process::exit(0);
  });
}

/// Tell attached clients goodbye and remove the runtime files. Runs from
/// `util::terminate` (exit hook), the panic hook and `request_stop`.
pub fn on_exit() {
  if let Some(reg) = server::REGISTRY.get() {
    reg.broadcast(ServerMsg::Bye {
      reason: "stopping".to_string(),
    });
    // give the writer threads a moment to flush
    thread::sleep(Duration::from_millis(150));
  }
  paths::remove_runtime_files();
}

/// `vtmate --daemon-foreground`: the daemon process itself.
pub fn run_foreground(args: &Args) -> ! {
  let _ = crate::log::set_file_sink(&paths::log_file());
  crate::log::log(
    "info",
    &format!("vtmate daemon {} starting", env!("CARGO_PKG_VERSION")),
  );
  util::set_exit_hook(Box::new(on_exit));
  std::panic::set_hook(Box::new(|info| {
    crate::log::log("error", &format!("panic: {}", info));
    on_exit();
  }));
  let _ = std::fs::remove_file(paths::start_error_file());

  // ---------------------------------------------------
  // settings
  // ---------------------------------------------------
  let _ = config::ensure_settings_file();
  let settings_path = config::resolve_settings_path(args)
    .unwrap_or_else(|e| fail_start(&e.to_string(), EXIT_BAD_SETTINGS));
  let mut args = args.clone();
  args.ptt = Some(true); // the mic opens only while a push-to-talk combo is held
  let agents = config::load_settings(&settings_path, &args).unwrap_or_else(|e| {
    fail_start(
      &format!("Failed to load settings: {}", e),
      EXIT_BAD_SETTINGS,
    )
  });
  let general = config::load_general_settings(&settings_path).unwrap_or_default();
  let settings = config::select_agent(&agents, args.agent.as_deref(), &general)
    .unwrap_or_else(|e| fail_start(&e, EXIT_BAD_SETTINGS));
  let daemon_settings = config::load_daemon_settings(&settings_path)
    .unwrap_or_else(|e| fail_start(&e.to_string(), EXIT_BAD_SETTINGS));

  // ---------------------------------------------------
  // global shortcuts: must all be free, else we do not start
  // ---------------------------------------------------
  if std::env::var_os("WAYLAND_DISPLAY").is_some() && std::env::var_os("DISPLAY").is_none() {
    crate::log::log(
      "warning",
      "Wayland session: global shortcuts need X11 (or XWayland with DISPLAY set)",
    );
  }
  let hotkeys = hotkeys::HotkeySet::parse(&daemon_settings)
    .unwrap_or_else(|e| fail_start(&e, EXIT_BAD_SETTINGS));
  let manager = GlobalHotKeyManager::new().unwrap_or_else(|e| {
    fail_start(
      &format!(
        "cannot access the desktop for global shortcuts: {} (global shortcuts need X11 on Linux)",
        e
      ),
      EXIT_BAD_SETTINGS,
    )
  });
  if let Err(taken) = hotkeys.register_all(&manager) {
    let mut msg = String::new();
    for (setting, combo) in &taken {
      msg.push_str(&format!("{} ({})\n", combo, setting));
    }
    fail_start(&msg, EXIT_HOTKEYS_TAKEN);
  }
  let _ = server::INFO.set(server::DaemonInfo {
    hotkeys: hotkeys.describe(),
    started: Instant::now(),
  });

  // ---------------------------------------------------
  // pid + socket (before the models, so `--daemon` returns quickly)
  // ---------------------------------------------------
  if let Err(e) = paths::write_pid() {
    fail_start(
      &format!("cannot write {}: {}", paths::pid_file().display(), e),
      EXIT_BAD_SETTINGS,
    );
  }
  let listener = bind_listener().unwrap_or_else(|e| {
    fail_start(
      &format!("cannot open the daemon socket: {}", e),
      EXIT_BAD_SETTINGS,
    )
  });
  let (tx_client, rx_client) = unbounded::<server::ClientEvent>();
  thread::spawn({
    let tx_client = tx_client.clone();
    move || server::server_thread(listener, tx_client)
  });

  // ---------------------------------------------------
  // state
  // ---------------------------------------------------
  let state = std::sync::Arc::new(state::AppState::with_agent(
    settings.clone(),
    agents.clone(),
    false,
    settings_path.clone(),
  ));
  state.daemon_mode.store(true, Ordering::Relaxed);
  state.recording_paused.store(true, Ordering::Relaxed);
  // the daemon only opens the mic while a combo is held, whatever the file says
  *state.ptt_override.lock().unwrap() = args.ptt;
  GLOBAL_STATE.set(state.clone()).ok();

  // ---------------------------------------------------
  // assets, channels, threads
  // ---------------------------------------------------
  crate::assets::ensure_piper_espeak_env();
  crate::assets::ensure_assets_env();
  crate::assets::ensure_supersonic2_assets();
  crate::assets::ensure_supertonic_assets();

  let channels = engine::Channels::new();
  crate::log::set_tx_ui_sender(channels.tx_ui.clone());
  thread::spawn({
    let rx_ui = channels.rx_ui.clone();
    let state = state.clone();
    move || server::ui_sink_thread(rx_ui, state)
  });

  let (tx_action, rx_action) = unbounded::<crate::conversation::DaemonAction>();
  let eng = engine::start(
    &state,
    &settings,
    channels,
    engine::EngineOptions {
      quiet: false,
      save: args.save,
      initial_prompt: None,
      tx_action: Some(tx_action),
    },
  )
  .unwrap_or_else(|e| fail_start(&format!("audio setup failed: {}", e), EXIT_BAD_SETTINGS));

  thread::spawn({
    let inputs = controller::ControllerInputs {
      state: state.clone(),
      hotkeys,
      rx_action,
      rx_client,
      tx_ui: eng.tx_ui.clone(),
      tx_tts: eng.tx_tts.clone(),
      tts_done_rx: eng.tts_done_rx.clone(),
      tx_utt: eng.tx_utt.clone(),
      stop_play_tx: eng.stop_play_tx.clone(),
      tx_cmd_conv: eng.tx_cmd_conv.clone(),
    };
    move || controller::controller_thread(inputs)
  });

  crate::log::log(
    "info",
    &format!(
      "daemon ready to take shortcuts (agent '{}', pid {})",
      settings.name,
      std::process::id()
    ),
  );
  // The hotkey manager must stay alive for the whole daemon life.
  let _keep = manager;
  hotkeys::run_platform_loop()
}

// PRIVATE
// ------------------------------------------------------------------

/// Exit from a control command: no raw mode was enabled, so no terminal
/// restoration is needed (and its escape sequences would just be printed).
fn plain_exit(code: i32) -> ! {
  let _ = std::io::Write::flush(&mut std::io::stdout());
  std::process::exit(code)
}

fn fail_start(msg: &str, code: i32) -> ! {
  crate::log::log("error", &format!("daemon not started: {}", msg.trim()));
  let _ = std::fs::write(paths::start_error_file(), format!("{}\n", msg.trim()));
  let _ = std::fs::remove_file(paths::pid_file());
  #[cfg(unix)]
  {
    let _ = std::fs::remove_file(paths::socket_file());
  }
  std::process::exit(code);
}

fn bind_listener() -> std::io::Result<interprocess::local_socket::Listener> {
  let name = paths::socket_name()?;
  match ListenerOptions::new().name(name.clone()).create_sync() {
    Ok(l) => Ok(l),
    Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
      if ipc::probe(PROBE_TIMEOUT).is_some() {
        return Err(std::io::Error::new(
          std::io::ErrorKind::AddrInUse,
          "another vtmate daemon is running",
        ));
      }
      #[cfg(unix)]
      {
        let _ = std::fs::remove_file(paths::socket_file());
      }
      ListenerOptions::new().name(name).create_sync()
    }
    Err(e) => Err(e),
  }
}
