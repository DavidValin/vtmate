// ------------------------------------------------------------------
//  IPC server: attached clients, status / stop requests, UI fan-out
// ------------------------------------------------------------------

use super::ipc::{AgentSummary, ClientMsg, ServerMsg, StateView, StatusView, read_msg, write_msg};
use crate::state::{AppState, GLOBAL_STATE, UtteranceKind};
use crossbeam_channel::{Receiver, Sender, select, unbounded};
use crossterm::event::KeyEvent;
use interprocess::local_socket::Listener;
use interprocess::local_socket::prelude::*;
use std::io::BufReader;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

// API
// ------------------------------------------------------------------

/// What attached clients ask the controller to do.
#[derive(Debug)]
pub enum ClientEvent {
  Key(KeyEvent),
  Say { text: String, kind: UtteranceKind },
}

struct Client {
  id: u64,
  tx: Sender<ServerMsg>,
}

/// Everyone currently attached. Adding a client and taking its snapshot
/// happen under the same lock as `broadcast`, so a client never misses or
/// duplicates a UI line around its attach.
pub struct ClientRegistry {
  clients: Mutex<Vec<Client>>,
  next_id: AtomicU64,
}

impl ClientRegistry {
  pub fn new() -> Self {
    Self {
      clients: Mutex::new(Vec::new()),
      next_id: AtomicU64::new(1),
    }
  }

  pub fn len(&self) -> usize {
    self.clients.lock().map(|c| c.len()).unwrap_or(0)
  }

  pub fn broadcast(&self, msg: ServerMsg) {
    let mut clients = self.clients.lock().unwrap();
    clients.retain(|c| c.tx.send(msg.clone()).is_ok());
  }

  fn attach(&self, state: &AppState, tx: Sender<ServerMsg>) -> (u64, ServerMsg) {
    let mut clients = self.clients.lock().unwrap();
    let id = self.next_id.fetch_add(1, Ordering::SeqCst);
    let snapshot = ServerMsg::Snapshot {
      status: status_view(state, clients.len() + 1),
      agents: state
        .agents()
        .iter()
        .map(|a| AgentSummary {
          name: a.name.clone(),
          language: a.language.clone(),
        })
        .collect(),
      history: state.conversation_history.lock().unwrap().clone(),
      state: StateView::capture(state),
    };
    clients.push(Client { id, tx });
    (id, snapshot)
  }

  fn remove(&self, id: u64) {
    self.clients.lock().unwrap().retain(|c| c.id != id);
  }
}

/// Static daemon facts for status replies.
pub struct DaemonInfo {
  pub hotkeys: Vec<(String, String)>,
  pub started: Instant,
}

pub static INFO: OnceLock<DaemonInfo> = OnceLock::new();
pub static REGISTRY: OnceLock<Arc<ClientRegistry>> = OnceLock::new();

pub fn registry() -> Arc<ClientRegistry> {
  REGISTRY
    .get_or_init(|| Arc::new(ClientRegistry::new()))
    .clone()
}

pub fn status_view(state: &AppState, clients: usize) -> StatusView {
  let info = INFO.get();
  StatusView {
    pid: std::process::id(),
    version: env!("CARGO_PKG_VERSION").to_string(),
    ready: state.stt_ready.load(Ordering::SeqCst),
    agent: state.agent_name.lock().unwrap().clone(),
    clients,
    hotkeys: info.map(|i| i.hotkeys.clone()).unwrap_or_default(),
    uptime_s: info.map(|i| i.started.elapsed().as_secs()).unwrap_or(0),
  }
}

/// Accept loop. One reader thread (and one writer thread for attached
/// viewers) per connection.
pub fn server_thread(listener: Listener, tx_client: Sender<ClientEvent>) {
  for conn in listener.incoming() {
    let stream = match conn {
      Ok(s) => s,
      Err(e) => {
        crate::log::log("warning", &format!("ipc accept failed: {}", e));
        thread::sleep(Duration::from_millis(100));
        continue;
      }
    };
    let tx_client = tx_client.clone();
    thread::spawn(move || handle_connection(stream, tx_client));
  }
}

/// Drain `rx_ui` forever: UI lines go to attached clients (or nowhere), and
/// state changes are published at most every 50 ms. Nothing here ever blocks
/// the producers, which is what keeps the bounded(1) UI channel safe headless.
pub fn ui_sink_thread(rx_ui: Receiver<String>, state: Arc<AppState>) {
  let registry = registry();
  let mut last_state: Option<StateView> = None;
  loop {
    select! {
      recv(rx_ui) -> line => {
        let Ok(line) = line else { break };
        if registry.len() == 0 {
          continue;
        }
        if line.starts_with("redraw_full_history|") {
          registry.broadcast(ServerMsg::History {
            history: state.conversation_history.lock().unwrap().clone(),
          });
        }
        registry.broadcast(ServerMsg::Ui { line });
      }
      default(Duration::from_millis(50)) => {
        if registry.len() == 0 {
          last_state = None;
          continue;
        }
        let now = StateView::capture(&state);
        if last_state.as_ref() != Some(&now) {
          registry.broadcast(ServerMsg::State(now.clone()));
          last_state = Some(now);
        }
      }
    }
  }
}

// PRIVATE
// ------------------------------------------------------------------

fn handle_connection(stream: interprocess::local_socket::Stream, tx_client: Sender<ClientEvent>) {
  let (reader, mut writer) = stream.split();
  let mut reader = BufReader::new(reader);
  let Some(state) = GLOBAL_STATE.get() else {
    // still booting
    let _ = write_msg(
      &mut writer,
      &ServerMsg::Error {
        message: "daemon starting".to_string(),
      },
    );
    return;
  };
  let registry = registry();

  let first: ClientMsg = match read_msg(&mut reader) {
    Ok(Some(m)) => m,
    _ => return,
  };
  match first {
    ClientMsg::Status => {
      let _ = write_msg(
        &mut writer,
        &ServerMsg::Status(status_view(state, registry.len())),
      );
    }
    ClientMsg::Stop => {
      let _ = write_msg(
        &mut writer,
        &ServerMsg::Bye {
          reason: "stopping".to_string(),
        },
      );
      super::request_stop();
    }
    ClientMsg::Say { text, kind } => {
      let _ = tx_client.send(ClientEvent::Say { text, kind });
      let _ = write_msg(
        &mut writer,
        &ServerMsg::Status(status_view(state, registry.len())),
      );
    }
    ClientMsg::Attach { version } => {
      if version != env!("CARGO_PKG_VERSION") {
        crate::log::log(
          "warning",
          &format!(
            "client version {} differs from daemon {}",
            version,
            env!("CARGO_PKG_VERSION")
          ),
        );
      }
      let (tx, rx) = unbounded::<ServerMsg>();
      let (id, snapshot) = registry.attach(state, tx);
      if write_msg(&mut writer, &snapshot).is_err() {
        registry.remove(id);
        return;
      }
      crate::log::log("info", &format!("client {} attached", id));
      // writer: drains the client's queue until it is closed or the socket dies
      let writer_handle = thread::spawn(move || {
        for msg in rx.iter() {
          if write_msg(&mut writer, &msg).is_err() {
            break;
          }
          if matches!(msg, ServerMsg::Bye { .. }) {
            break;
          }
        }
      });
      // reader: keys and control messages from the client
      loop {
        match read_msg::<_, ClientMsg>(&mut reader) {
          Ok(Some(ClientMsg::Key(k))) => {
            let _ = tx_client.send(ClientEvent::Key(k));
          }
          Ok(Some(ClientMsg::Say { text, kind })) => {
            let _ = tx_client.send(ClientEvent::Say { text, kind });
          }
          Ok(Some(ClientMsg::Stop)) => {
            super::request_stop();
            break;
          }
          Ok(Some(ClientMsg::Status)) => {
            registry.broadcast(ServerMsg::Status(status_view(state, registry.len())));
          }
          Ok(Some(ClientMsg::Detach)) | Ok(None) | Err(_) => break,
          Ok(Some(ClientMsg::Attach { .. })) => {}
        }
      }
      registry.remove(id);
      crate::log::log("info", &format!("client {} detached", id));
      let _ = writer_handle.join();
    }
    ClientMsg::Key(_) | ClientMsg::Detach => {}
  }
}
