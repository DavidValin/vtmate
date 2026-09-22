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
  // Resolved here (against this client's own cwd), not the daemon's: `-c`
  // takes a path the same way it would for a fresh, non-daemon run.
  let agents_path = if args.config.is_some() {
    match crate::config::resolve_agents_path(args) {
      Ok(p) => Some(p.to_string_lossy().into_owned()),
      Err(e) => {
        println!("✗ {}", e);
        util::terminate(1);
      }
    }
  } else {
    None
  };
  let stream = match ipc::connect() {
    Ok(s) => s,
    Err(e) => {
      println!("✗ cannot connect to the vtmate daemon: {}", e);
      util::terminate(1);
    }
  };
  let (reader, mut writer) = stream.split();
  let mut reader = BufReader::new(reader);
  if let Err(e) = ipc::write_msg(
    &mut writer,
    &ClientMsg::Attach {
      version: env!("CARGO_PKG_VERSION").to_string(),
      agent: args.agent.clone(),
      agents_path,
    },
  ) {
    println!("✗ daemon connection failed: {}", e);
    util::terminate(1);
  }
  if args.save || args.save_html {
    // -s/--save-html against an already-running daemon: turn saving on for
    // the rest of this daemon session instead of silently doing nothing,
    // since this client's own Args/AppState are just a throwaway mirror.
    let _ = ipc::write_msg(
      &mut writer,
      &ClientMsg::StartSave {
        save: args.save,
        save_html: args.save_html,
      },
    );
  }
  let (status, agents, history, view) = match ipc::read_msg::<_, ServerMsg>(&mut reader) {
    Ok(Some(ServerMsg::Snapshot {
      status,
      agents,
      history,
      state,
    })) => (status, agents, history, state),
    Ok(Some(ServerMsg::Error { message })) => {
      println!("✗ daemon: {}", message);
      util::terminate(1);
    }
    other => {
      println!(
        "✗ unexpected reply from the daemon: {:?}",
        other.ok().flatten()
      );
      util::terminate(1);
    }
  };

  // Mirror state: the UI thread renders from GLOBAL_STATE exactly as in
  // the terminal mode; the daemon keeps it up to date over the socket.
  let mut state = AppState::new();
  state.agents = Arc::new(std::sync::Mutex::new(
    agents
      .iter()
      .map(|a| crate::config::AgentSettings {
        name: a.name.clone(),
        language: a.language.clone(),
        ..Default::default()
      })
      .collect(),
  ));
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
    args.no_banner,
  );
  let _ = tx_ui.send(format!(
    "line|\x1b[36m↔ attached to vtmate daemon (pid {}) - Ctrl+C detaches, the daemon keeps running\x1b[0m",
    status.pid
  ));
  // A fresh session (0 turns, conversation or debate) has nothing to
  // rebuild, and `redraw_full_history` unconditionally clears the screen and
  // reprints only from `conversation_history` - sending it here would wipe
  // the banner and the greeting above for nothing. Once there is real
  // history, it still needs this to lay out at the attached terminal's own
  // width.
  if !state.conversation_history.lock().unwrap().is_empty() {
    let _ = tx_ui.send("redraw_full_history|".to_string());
  }

  let exit_reason: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));

  // reader thread: daemon -> local UI / mirror state
  thread::spawn({
    let state = state.clone();
    let tx_ui = tx_ui.clone();
    let exit_reason = exit_reason.clone();
    move || {
      // The daemon keeps running, and generating turns, whether or not anyone
      // is attached - detaching does not pause it. Once finish() has decided
      // to send its own final line, this thread must stop competing with it
      // for the channel's one slot, or an active conversation can keep
      // winning that race indefinitely and the final line never gets a turn,
      // let alone the last one. Reading continues regardless, so the socket
      // is still drained and a Bye or a dropped connection still noticed.
      let forward = |line: String| {
        if !crate::ui::UI_SHUTDOWN.load(Ordering::Relaxed) {
          let _ = tx_ui.send(line);
        }
      };
      loop {
        match ipc::read_msg::<_, ServerMsg>(&mut reader) {
          Ok(Some(ServerMsg::Ui { line })) => {
            forward(line);
          }
          Ok(Some(ServerMsg::State(v))) => {
            let was_open = crate::settings_ui::is_open(&state);
            let was_debate_open = state.debate_modal_visible.load(Ordering::Relaxed);
            let was_save_open = state.save_modal_visible.load(Ordering::Relaxed);
            let was_compose_open = crate::compose::is_open(&state);
            v.apply(&state);
            // The popup is drawn from this mirror, and a state update is what
            // makes the daemon's last key press visible here: an explicit
            // "*_show|"/"*_hide|" line (sent alongside the state change that
            // opens or closes a popup) always wins that race and arrives
            // first, but every keystroke *within* an already-open popup only
            // ever changes state - text typed into the debate subject or
            // max-turns field, or a save-popup checkbox toggle - with no
            // accompanying line of its own. Without re-requesting a redraw
            // here on every such update, the popup would keep showing
            // whatever it last rendered until some unrelated line happened
            // to arrive afterwards.
            let is_open = crate::settings_ui::is_open(&state);
            if is_open {
              forward("settings_update|".to_string());
            } else if was_open {
              forward("settings_hide|".to_string());
            }
            let is_debate_open = state.debate_modal_visible.load(Ordering::Relaxed);
            if is_debate_open {
              forward("modal_update|".to_string());
            } else if was_debate_open {
              forward("modal_hide|".to_string());
            }
            let is_compose_open = crate::compose::is_open(&state);
            if is_compose_open {
              forward("compose_update|".to_string());
            } else if was_compose_open {
              forward("compose_hide|".to_string());
            }
            let is_save_open = state.save_modal_visible.load(Ordering::Relaxed);
            if is_save_open {
              forward("save_modal_update|".to_string());
            } else if was_save_open {
              forward("save_modal_hide|".to_string());
            }
          }
          Ok(Some(ServerMsg::History { history })) => {
            *state.conversation_history.lock().unwrap() = history;
          }
          Ok(Some(ServerMsg::Bye { reason })) => {
            *exit_reason.lock().unwrap() = Some(format!("daemon {}", reason));
            break;
          }
          Ok(Some(ServerMsg::Error { message })) => {
            forward(format!("line|\x1b[31m✗ daemon: {}\x1b[0m", message));
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
      finish(&tx_ui, "info", &format!("{}", reason));
    }
    if event::poll(Duration::from_millis(50)).unwrap_or(false) {
      if let Ok(Event::Key(k)) = event::read() {
        let ctrl_c = k.modifiers.contains(KeyModifiers::CONTROL)
          && matches!(k.code, KeyCode::Char('c') | KeyCode::Char('C'));
        if ctrl_c {
          let _ = ipc::write_msg(&mut writer, &ClientMsg::Detach);
          finish(&tx_ui, "info", detached_msg);
        }
        if ipc::write_msg(&mut writer, &ClientMsg::Key(k)).is_err() {
          finish(&tx_ui, "error", "connection to the daemon lost");
        }
      }
    }
  }
}

// PRIVATE
// ------------------------------------------------------------------

fn finish(tx_ui: &crossbeam_channel::Sender<String>, level: &str, msg: &str) -> ! {
  // UI_SHUTDOWN before the send: the channel is bounded(1), so send() blocks
  // for as long as a slow reveal keeps the UI thread from returning to drain
  // it - setting the flag first is what makes that reveal give up quickly
  // (see stream_chunk) instead of holding this message up behind it.
  crate::ui::UI_SHUTDOWN.store(true, Ordering::Relaxed);
  // "final_line", not "line": the UI thread renders it and stops right there,
  // before looking at anything else queued or still arriving from the reader
  // thread below - a plain "line" only gets drawn, and whatever the reader
  // thread forwards next (the daemon can keep sending for a moment after
  // Detach) would still render after it.
  //
  // Leading "\n\n", matching every other notice injected mid-conversation
  // ("USER interrupted", "Session restarted"): a line message continues
  // whatever row is already current rather than starting fresh - streaming a
  // reply builds it up that way, one chunk at a time - so this would
  // otherwise land glued onto the tail of the last thing the assistant said.
  //
  // The history stays exactly as it printed, the same as a non-daemon exit:
  // ON_ALT_SCREEN guards terminate()'s LeaveAlternateScreen so it only fires
  // when something actually left the primary screen, which is what keeps the
  // cursor from jumping to a stale position here. EXIT_LINE_PRINTED is
  // deliberately left unset: the viewport always reserves the bottom row for
  // the status bar (see viewport()), so this final line never lands there,
  // and terminate() still needs to run its normal clear on that row -
  // otherwise the bar's last frame is left painted on screen after the
  // process exits.
  let _ = tx_ui.send(format!(
    "final_line|\n\n{}",
    crate::log::marked_line(level, msg)
  ));
  crate::ui::wait_for_ui_stopped(Duration::from_millis(200));
  let _ = terminal::disable_raw_mode();
  let mut out = std::io::stdout();
  let _ = crossterm::execute!(out, crossterm::cursor::Show);
  let _ = std::io::Write::flush(&mut out);
  thread::sleep(Duration::from_millis(50));
  util::terminate(0);
}
