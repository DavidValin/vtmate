// ------------------------------------------------------------------
//  Daemon <-> client protocol (newline-delimited JSON over a local socket)
// ------------------------------------------------------------------

use crate::conversation::ChatMessage;
use crate::state::{AppState, UtteranceKind};
use interprocess::local_socket::Stream as LocalStream;
use interprocess::local_socket::prelude::*;
use serde::{Deserialize, Serialize};
use std::io::{BufRead, Write};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

// API
// ------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "t", content = "d")]
pub enum ClientMsg {
  /// Become an attached viewer: expect a Snapshot, then Ui/State/History.
  Attach { version: String },
  /// One-shot status query (reply: Status).
  Status,
  /// Orderly daemon shutdown (reply: Bye). Used by `--daemon-stop`.
  Stop,
  /// A key pressed in the attached terminal.
  Key(crossterm::event::KeyEvent),
  /// Leave; the daemon keeps running.
  Detach,
  /// `-s`/`--save-html` given to an attach-client while the daemon was
  /// already running: turn saving on for the rest of this daemon session.
  /// Only ever turns a flag on, never off.
  StartSave { save: bool, save_html: bool },
  /// Testing without a microphone: process `text` as if it had been
  /// transcribed from a hotkey utterance of this kind. Hidden.
  Say { text: String, kind: UtteranceKind },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "t", content = "d")]
pub enum ServerMsg {
  Snapshot {
    status: StatusView,
    agents: Vec<AgentSummary>,
    history: Vec<ChatMessage>,
    state: StateView,
  },
  /// Full history replacement, sent right before a `redraw_full_history|` line.
  History {
    history: Vec<ChatMessage>,
  },
  /// A raw "type|payload" line, exactly what the terminal UI thread consumes.
  Ui {
    line: String,
  },
  State(StateView),
  Status(StatusView),
  Bye {
    reason: String,
  },
  Error {
    message: String,
  },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StatusView {
  pub pid: u32,
  pub version: String,
  /// whisper loaded and hotkeys live
  pub ready: bool,
  pub agent: String,
  pub clients: usize,
  /// (setting name, combo)
  pub hotkeys: Vec<(String, String)>,
  pub uptime_s: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSummary {
  pub name: String,
  pub language: String,
}

/// Everything the bottom bar / debate modal read from `GLOBAL_STATE`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StateView {
  pub agent_name: String,
  pub language: String,
  pub ptt: bool,
  pub recording_paused: bool,
  pub thinking: bool,
  pub playing: bool,
  pub agent_speaking: bool,
  pub peak: f32,
  pub speed: u32,
  pub playback_active: bool,
  pub processing_response: bool,
  pub debate_enabled: bool,
  pub debate_paused: bool,
  pub debate_agents: Vec<String>,
  /// `-s`/`--save-html`, live: drives the bottom bar's SAVING tag on an
  /// attached client, which otherwise only ever sees its own throwaway
  /// mirror of these flags, never the daemon's real ones.
  #[serde(default)]
  pub save_enabled: bool,
  #[serde(default)]
  pub save_html_enabled: bool,
  pub modal_visible: bool,
  pub modal_agent1: usize,
  pub modal_agent2: usize,
  pub modal_focus: u8,
  /// The debate modal's typed initial-message field (Ctrl+D popup), so an
  /// attached terminal can draw the text and caret the daemon is editing.
  #[serde(default)]
  pub modal_subject: String,
  #[serde(default)]
  pub modal_caret: usize,
  #[serde(default)]
  pub modal_max_turns: String,
  #[serde(default)]
  pub modal_max_turns_caret: usize,
  /// The Ctrl+E save popup, same idea: open/closed, its two checkboxes, which
  /// one has focus, and the folder name to show while a save is running.
  #[serde(default)]
  pub save_modal_visible: bool,
  #[serde(default)]
  pub save_modal_check_txt: bool,
  #[serde(default)]
  pub save_modal_check_html: bool,
  #[serde(default)]
  pub save_modal_focus: u8,
  #[serde(default)]
  pub save_modal_folder: Option<String>,
  /// The settings popup while it is open, so an attached terminal can draw
  /// it. The keys that drive it are handled by the daemon, which owns the
  /// agents file; the client only renders this copy.
  #[serde(default)]
  pub settings: Option<crate::settings_ui::SettingsUi>,
}

impl StateView {
  pub fn capture(state: &AppState) -> Self {
    let peak = state.ui.peak.lock().map(|p| *p).unwrap_or(0.0);
    Self {
      agent_name: state.agent_name.lock().unwrap().clone(),
      language: state.language.lock().unwrap().clone(),
      ptt: state.ptt.load(Ordering::Relaxed),
      recording_paused: state.recording_paused.load(Ordering::Relaxed),
      thinking: state.ui.thinking.load(Ordering::Relaxed),
      playing: state.ui.playing.load(Ordering::Relaxed),
      agent_speaking: state.ui.agent_speaking.load(Ordering::Relaxed),
      peak: (peak * 100.0).round() / 100.0,
      speed: state.speed.load(Ordering::Relaxed),
      playback_active: state.playback.playback_active.load(Ordering::Relaxed),
      processing_response: state.processing_response.load(Ordering::Relaxed),
      debate_enabled: state.debate_enabled.load(Ordering::SeqCst),
      debate_paused: state.debate_paused.load(Ordering::SeqCst),
      save_enabled: state.save_enabled.load(Ordering::Relaxed),
      save_html_enabled: state.save_html_enabled.load(Ordering::Relaxed),
      debate_agents: state
        .debate_agents
        .lock()
        .unwrap()
        .iter()
        .map(|a| a.name.clone())
        .collect(),
      modal_visible: state.debate_modal_visible.load(Ordering::SeqCst),
      modal_agent1: *state.debate_modal_selected_agent1.lock().unwrap(),
      modal_agent2: *state.debate_modal_selected_agent2.lock().unwrap(),
      modal_focus: *state.debate_modal_focus.lock().unwrap(),
      modal_subject: state.debate_modal_subject.lock().unwrap().clone(),
      modal_caret: *state.debate_modal_caret.lock().unwrap(),
      modal_max_turns: state.debate_modal_max_turns.lock().unwrap().clone(),
      modal_max_turns_caret: *state.debate_modal_max_turns_caret.lock().unwrap(),
      save_modal_visible: state.save_modal_visible.load(Ordering::SeqCst),
      save_modal_check_txt: state.save_modal_check_txt.load(Ordering::SeqCst),
      save_modal_check_html: state.save_modal_check_html.load(Ordering::SeqCst),
      save_modal_focus: *state.save_modal_focus.lock().unwrap(),
      save_modal_folder: state.save_modal_folder.lock().unwrap().clone(),
      settings: {
        let settings = state.settings_ui.lock().unwrap();
        settings.open.then(|| settings.clone())
      },
    }
  }

  /// Apply to the attached client's mirror state.
  pub fn apply(&self, state: &AppState) {
    *state.agent_name.lock().unwrap() = self.agent_name.clone();
    *state.language.lock().unwrap() = self.language.clone();
    state.ptt.store(self.ptt, Ordering::Relaxed);
    state
      .recording_paused
      .store(self.recording_paused, Ordering::Relaxed);
    state.ui.thinking.store(self.thinking, Ordering::Relaxed);
    state.ui.playing.store(self.playing, Ordering::Relaxed);
    state
      .ui
      .agent_speaking
      .store(self.agent_speaking, Ordering::Relaxed);
    if let Ok(mut p) = state.ui.peak.lock() {
      *p = self.peak;
    }
    state.speed.store(self.speed, Ordering::Relaxed);
    state
      .playback
      .playback_active
      .store(self.playback_active, Ordering::Relaxed);
    state
      .processing_response
      .store(self.processing_response, Ordering::Relaxed);
    state
      .debate_enabled
      .store(self.debate_enabled, Ordering::SeqCst);
    state
      .debate_paused
      .store(self.debate_paused, Ordering::SeqCst);
    state
      .save_enabled
      .store(self.save_enabled, Ordering::Relaxed);
    state
      .save_html_enabled
      .store(self.save_html_enabled, Ordering::Relaxed);
    {
      // the modal and the bar only read agent names from these entries
      let agents = state.agents();
      let mut da = state.debate_agents.lock().unwrap();
      *da = self
        .debate_agents
        .iter()
        .map(|name| {
          agents
            .iter()
            .find(|a| a.name == *name)
            .cloned()
            .unwrap_or_else(|| crate::config::AgentSettings {
              name: name.clone(),
              ..Default::default()
            })
        })
        .collect();
    }
    state
      .debate_modal_visible
      .store(self.modal_visible, Ordering::SeqCst);
    *state.debate_modal_selected_agent1.lock().unwrap() = self.modal_agent1;
    *state.debate_modal_selected_agent2.lock().unwrap() = self.modal_agent2;
    *state.debate_modal_focus.lock().unwrap() = self.modal_focus;
    *state.debate_modal_subject.lock().unwrap() = self.modal_subject.clone();
    *state.debate_modal_caret.lock().unwrap() = self.modal_caret;
    *state.debate_modal_max_turns.lock().unwrap() = self.modal_max_turns.clone();
    *state.debate_modal_max_turns_caret.lock().unwrap() = self.modal_max_turns_caret;
    state
      .save_modal_visible
      .store(self.save_modal_visible, Ordering::SeqCst);
    state
      .save_modal_check_txt
      .store(self.save_modal_check_txt, Ordering::SeqCst);
    state
      .save_modal_check_html
      .store(self.save_modal_check_html, Ordering::SeqCst);
    *state.save_modal_focus.lock().unwrap() = self.save_modal_focus;
    *state.save_modal_folder.lock().unwrap() = self.save_modal_folder.clone();
    *state.settings_ui.lock().unwrap() = self.settings.clone().unwrap_or_default();
  }
}

/// Serialize one message as a JSON line and flush.
pub fn write_msg<W: Write, T: Serialize>(w: &mut W, m: &T) -> std::io::Result<()> {
  let mut line = serde_json::to_string(m).map_err(std::io::Error::other)?;
  line.push('\n');
  w.write_all(line.as_bytes())?;
  w.flush()
}

/// Read one JSON line. `Ok(None)` on a clean end of stream.
pub fn read_msg<R: BufRead, T: serde::de::DeserializeOwned>(
  r: &mut R,
) -> std::io::Result<Option<T>> {
  let mut line = String::new();
  loop {
    line.clear();
    let n = r.read_line(&mut line)?;
    if n == 0 {
      return Ok(None);
    }
    let t = line.trim();
    if t.is_empty() {
      continue;
    }
    return serde_json::from_str::<T>(t)
      .map(Some)
      .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e));
  }
}

/// Connect to the running daemon's socket.
pub fn connect() -> std::io::Result<LocalStream> {
  let name = super::paths::socket_name()?;
  LocalStream::connect(name)
}

/// Ask the daemon for its status; `None` when no daemon answers within
/// `timeout` (stale socket files are removed).
pub fn probe(timeout: Duration) -> Option<StatusView> {
  let deadline = Instant::now() + timeout;
  let stream = match connect() {
    Ok(s) => s,
    Err(_) => {
      #[cfg(unix)]
      {
        // socket file without a listener: leftover from a killed daemon
        if super::paths::socket_file().exists()
          && !super::paths::read_pid().is_some_and(super::paths::pid_alive)
        {
          super::paths::remove_runtime_files();
        }
      }
      return None;
    }
  };
  let (reader, mut writer) = stream.split();
  let mut reader = std::io::BufReader::new(reader);
  if write_msg(&mut writer, &ClientMsg::Status).is_err() {
    return None;
  }
  // the daemon answers immediately; the deadline only guards a wedged peer
  let handle = std::thread::spawn(move || read_msg::<_, ServerMsg>(&mut reader));
  while !handle.is_finished() {
    if Instant::now() >= deadline {
      return None;
    }
    std::thread::sleep(Duration::from_millis(10));
  }
  match handle.join() {
    Ok(Ok(Some(ServerMsg::Status(s)))) => Some(s),
    _ => None,
  }
}
