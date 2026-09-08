// ------------------------------------------------------------------
//  Attached terminal: the normal TUI, driven by the running daemon
// ------------------------------------------------------------------

use crate::daemon::ipc::{self, ClientMsg, ServerMsg};
use crate::state::{AppState, GLOBAL_STATE};
use crate::util;
use crossbeam_channel::bounded;
use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use crossterm::terminal;
use interprocess::local_socket::prelude::*;
use std::io::BufReader;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

// API
// ------------------------------------------------------------------

/// Attach to the running daemon. Returns only by terminating the process.
pub fn run(args: &crate::config::Args) -> ! {
  if args.agent.is_some() || args.config.is_some() {
    println!(
      "ℹ️  a vtmate daemon is running: attaching to it (-a / -c ignored; switch agents with LEFT/RIGHT)"
    );
    thread::sleep(Duration::from_millis(800));
  }
  let stream = match ipc::connect() {
    Ok(s) => s,
    Err(e) => {
      println!("❌ cannot connect to the vtmate daemon: {}", e);
      util::terminate(1);
    }
  };
  let (reader, mut writer) = stream.split();
  let mut reader = BufReader::new(reader);
  if let Err(e) = ipc::write_msg(
    &mut writer,
    &ClientMsg::Attach {
      version: env!("CARGO_PKG_VERSION").to_string(),
    },
  ) {
    println!("❌ daemon connection failed: {}", e);
    util::terminate(1);
  }
  let (status, agents, history, view) = match ipc::read_msg::<_, ServerMsg>(&mut reader) {
    Ok(Some(ServerMsg::Snapshot {
      status,
      agents,
      history,
      state,
    })) => (status, agents, history, state),
    Ok(Some(ServerMsg::Error { message })) => {
      println!("❌ daemon: {}", message);
      util::terminate(1);
    }
    other => {
      println!(
        "❌ unexpected reply from the daemon: {:?}",
        other.ok().flatten()
      );
      util::terminate(1);
    }
  };

  // Mirror state: the UI thread renders from GLOBAL_STATE exactly as in
  // the terminal mode; the daemon keeps it up to date over the socket.
  let mut state = AppState::new();
  state.agents = Arc::new(
    agents
      .iter()
      .map(|a| crate::config::AgentSettings {
        name: a.name.clone(),
        language: a.language.clone(),
        ..Default::default()
      })
      .collect(),
  );
  let state = Arc::new(state);
  state.daemon_mode.store(true, Ordering::Relaxed);
  view.apply(&state);
  *state.conversation_history.lock().unwrap() = history;
  let _ = GLOBAL_STATE.set(state.clone());

  let _ = terminal::enable_raw_mode();
  let (tx_ui, rx_ui) = bounded::<String>(1);
  let _ui_handle = crate::ui::spawn_ui_thread(
    state.ui.clone(),
    state.status_line.clone(),
    rx_ui,
    state.conversation_history.clone(),
  );
  let _ = tx_ui.send(format!(
    "line|\x1b[36m🔗 attached to vtmate daemon (pid {}) - Ctrl+C detaches, the daemon keeps running\x1b[0m",
    status.pid
  ));
  let _ = tx_ui.send("redraw_full_history|".to_string());

  let exit_reason: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));

  // reader thread: daemon -> local UI / mirror state
  thread::spawn({
    let state = state.clone();
    let tx_ui = tx_ui.clone();
    let exit_reason = exit_reason.clone();
    move || {
      loop {
        match ipc::read_msg::<_, ServerMsg>(&mut reader) {
          Ok(Some(ServerMsg::Ui { line })) => {
            let _ = tx_ui.send(line);
          }
          Ok(Some(ServerMsg::State(v))) => v.apply(&state),
          Ok(Some(ServerMsg::History { history })) => {
            *state.conversation_history.lock().unwrap() = history;
          }
          Ok(Some(ServerMsg::Bye { reason })) => {
            *exit_reason.lock().unwrap() = Some(format!("daemon {}", reason));
            break;
          }
          Ok(Some(ServerMsg::Error { message })) => {
            let _ = tx_ui.send(format!("line|\x1b[31m❌ daemon: {}\x1b[0m", message));
          }
          Ok(Some(ServerMsg::Snapshot { .. })) | Ok(Some(ServerMsg::Status(_))) => {}
          Ok(None) | Err(_) => {
            *exit_reason.lock().unwrap() = Some("connection to the daemon lost".to_string());
            break;
          }
        }
      }
    }
  });

  // key loop: Ctrl+C detaches, everything else goes to the daemon
  let detached_msg = "detached, vtmate daemon still running (stop it with `vtmate --daemon-stop`)";
  loop {
    if let Some(reason) = exit_reason.lock().unwrap().clone() {
      finish("info", &format!("{}", reason));
    }
    if event::poll(Duration::from_millis(50)).unwrap_or(false) {
      if let Ok(Event::Key(k)) = event::read() {
        let ctrl_c = k.modifiers.contains(KeyModifiers::CONTROL)
          && matches!(k.code, KeyCode::Char('c') | KeyCode::Char('C'));
        if ctrl_c {
          let _ = ipc::write_msg(&mut writer, &ClientMsg::Detach);
          finish("info", detached_msg);
        }
        if ipc::write_msg(&mut writer, &ClientMsg::Key(k)).is_err() {
          finish("error", "connection to the daemon lost");
        }
      }
    }
  }
}

// PRIVATE
// ------------------------------------------------------------------

fn finish(level: &str, msg: &str) -> ! {
  let _ = terminal::disable_raw_mode();
  print!("\r\n");
  crate::log::notice(level, msg);
  thread::sleep(Duration::from_millis(50));
  util::terminate(0);
}
