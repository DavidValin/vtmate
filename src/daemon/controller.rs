// ------------------------------------------------------------------
//  Daemon controller: hotkey state machine, attached-client keys,
//  paste / reset / read-aloud actions
// ------------------------------------------------------------------

use super::desktop::{Desktop, SelectionAge};
use super::hotkeys::{Hotkey, HotkeySet};
use super::server::{ClientEvent, registry};
use crate::conversation::DaemonAction;
use crate::keyboard::{KeyCtx, KeyLocalState, KeyOutcome, handle_key};
use crate::state::{AppState, UtteranceKind, UtteranceMeta};
use crossbeam_channel::{Receiver, Sender, select};
use crossterm::event::{KeyCode, KeyEventKind};
use global_hotkey::{GlobalHotKeyEvent, HotKeyState};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

// API
// ------------------------------------------------------------------

pub struct ControllerInputs {
  pub state: Arc<AppState>,
  pub hotkeys: HotkeySet,
  pub rx_action: Receiver<DaemonAction>,
  pub rx_client: Receiver<ClientEvent>,
  pub tx_ui: Sender<String>,
  pub tx_tts: Sender<(String, u64, String)>,
  pub tts_done_rx: Receiver<()>,
  pub tx_utt: Sender<crate::audio::Utterance>,
  pub stop_play_tx: Sender<()>,
  pub tx_cmd_conv: Sender<crate::conversation::Command>,
}

/// A hotkey push-to-talk held longer than this is treated as released
/// (a lost release event, e.g. the X11 grab broken by a focus change).
const PTT_WATCHDOG: Duration = Duration::from_secs(120);

pub fn controller_thread(inputs: ControllerInputs) {
  let mut c = Controller {
    state: inputs.state,
    hotkeys: inputs.hotkeys,
    tx_ui: inputs.tx_ui,
    tx_tts: inputs.tx_tts,
    tts_done_rx: inputs.tts_done_rx,
    tx_utt: inputs.tx_utt,
    stop_play_tx: inputs.stop_play_tx,
    tx_cmd_conv: inputs.tx_cmd_conv,
    active_ptt: None,
    press_at: None,
    swallow_tts_release: false,
    desktop: Desktop::new(),
    key_state: KeyLocalState::default(),
    tts_job: None,
    last_reset_press: None,
    sent_selection_stamp: None,
    sent_selection_text: None,
  };
  let rx_hotkey = GlobalHotKeyEvent::receiver();
  loop {
    select! {
      recv(rx_hotkey) -> ev => {
        if let Ok(ev) = ev {
          c.on_hotkey(ev);
        }
      }
      recv(inputs.rx_action) -> a => {
        match a {
          Ok(a) => c.on_action(a),
          Err(_) => break,
        }
      }
      recv(inputs.rx_client) -> e => {
        match e {
          Ok(e) => c.on_client(e),
          Err(_) => break,
        }
      }
      default(Duration::from_millis(50)) => {
        c.tick();
      }
    }
  }
}

// PRIVATE
// ------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PttSource {
  HotkeyLlm,
  HotkeyPaste,
  AttachSpace,
}

/// Borrow the key-handler context from disjoint fields (leaves `key_state`
/// free to be borrowed mutably alongside it).
macro_rules! key_ctx {
  ($c:expr) => {
    KeyCtx {
      tx_ui: &$c.tx_ui,
      recording_paused: &$c.state.recording_paused,
      stop_play_tx: &$c.stop_play_tx,
      interrupt_counter: &$c.state.interrupt_counter,
      tx_cmd: &$c.tx_cmd_conv,
    }
  };
}

struct Controller {
  state: Arc<AppState>,
  hotkeys: HotkeySet,
  tx_ui: Sender<String>,
  tx_tts: Sender<(String, u64, String)>,
  tts_done_rx: Receiver<()>,
  tx_utt: Sender<crate::audio::Utterance>,
  stop_play_tx: Sender<()>,
  tx_cmd_conv: Sender<crate::conversation::Command>,
  active_ptt: Option<PttSource>,
  press_at: Option<Instant>,
  swallow_tts_release: bool,
  desktop: Desktop,
  key_state: KeyLocalState,
  tts_job: Option<JoinHandle<()>>,
  /// Reset hotkey: once stops the speech, twice within a second resets.
  last_reset_press: Option<Instant>,
  /// The selection sent with the previous message, so the same one is not
  /// sent again: its X timestamp, and its text for platforms with no stamp.
  sent_selection_stamp: Option<u32>,
  sent_selection_text: Option<String>,
}

impl Controller {
  fn speaking_now(&self) -> bool {
    self.state.playback.playback_active.load(Ordering::Relaxed)
      || self.state.processing_response.load(Ordering::Relaxed)
      || self.state.tts_read_active.load(Ordering::Relaxed)
  }

  /// Stop whatever is being spoken or generated.
  fn interrupt_speech(&self) {
    self.state.interrupt_counter.fetch_add(1, Ordering::SeqCst);
    let _ = self.stop_play_tx.try_send(());
    self
      .state
      .processing_response
      .store(false, Ordering::Relaxed);
    self.state.ui.thinking.store(false, Ordering::Relaxed);
  }

  fn ui_line(&self, text: &str) {
    let _ = self
      .tx_ui
      .send_timeout(format!("line|{}", text), Duration::from_millis(50));
  }

  fn set_meta(&self, kind: UtteranceKind, attachment: Option<String>) {
    if let Ok(mut m) = self.state.utterance_meta.lock() {
      *m = UtteranceMeta { kind, attachment };
    }
  }

  fn on_hotkey(&mut self, ev: GlobalHotKeyEvent) {
    let Some(which) = self.hotkeys.which(ev.id()) else {
      return;
    };
    let pressed = ev.state() == HotKeyState::Pressed;
    crate::log::log(
      "debug",
      &format!(
        "hotkey {:?} {}",
        which,
        if pressed { "pressed" } else { "released" }
      ),
    );
    match (which, pressed) {
      (Hotkey::LlmPtt, true) => self.ptt_press(PttSource::HotkeyLlm),
      (Hotkey::LlmPtt, false) => self.ptt_release(PttSource::HotkeyLlm),
      (Hotkey::PastePtt, true) => self.ptt_press(PttSource::HotkeyPaste),
      (Hotkey::PastePtt, false) => self.ptt_release(PttSource::HotkeyPaste),
      (Hotkey::TtsRead, true) => {
        if self.speaking_now() {
          self.interrupt_speech();
          self.swallow_tts_release = true;
        } else {
          self.swallow_tts_release = false;
        }
      }
      (Hotkey::TtsRead, false) => {
        if self.swallow_tts_release {
          self.swallow_tts_release = false;
          return;
        }
        if self.active_ptt.is_some() {
          return;
        }
        self.read_selection_aloud();
      }
      (Hotkey::Reset, true) => {}
      (Hotkey::Reset, false) => self.reset_hotkey(),
    }
  }

  fn ptt_press(&mut self, source: PttSource) {
    if self.active_ptt.is_some() {
      // auto-repeat, or another push-to-talk already holding the mic
      return;
    }
    self.active_ptt = Some(source);
    self.press_at = Some(Instant::now());
    self.interrupt_speech();
    let kind = if source == PttSource::HotkeyPaste {
      UtteranceKind::Paste
    } else {
      UtteranceKind::Llm
    };
    self.set_meta(kind, None);
    self.state.recording_paused.store(false, Ordering::Relaxed);
  }

  fn ptt_release(&mut self, source: PttSource) {
    if self.active_ptt != Some(source) {
      return;
    }
    if source == PttSource::HotkeyLlm {
      let attachment = self.current_selection();
      if let Ok(mut m) = self.state.utterance_meta.lock() {
        m.attachment = attachment;
      }
    }
    // The record callback flushes the buffered audio with the meta above.
    self.state.recording_paused.store(true, Ordering::Relaxed);
    self.active_ptt = None;
    self.press_at = None;
  }

  fn read_selection_aloud(&mut self) {
    if self.state.debate_enabled.load(Ordering::SeqCst) {
      crate::log::log("warning", "read-aloud is not available during a debate");
      return;
    }
    let Some(text) = self
      .desktop
      .selection_present()
      .then(|| self.desktop.read_selection())
      .flatten()
    else {
      crate::log::log("info", "nothing selected to read");
      return;
    };
    let phrases: Vec<String> = crate::util::split_text_for_tts(&text)
      .into_iter()
      .map(|(_, tts)| tts)
      .filter(|t| !t.trim().is_empty())
      .collect();
    if phrases.is_empty() {
      crate::log::log("info", "selection has nothing to speak");
      return;
    }
    self.ui_line(&format!(
      "\n\x1b[36m📖 Reading selection ({} phrases)\x1b[0m",
      phrases.len()
    ));
    if let Some(job) = self.tts_job.take() {
      // previous read: it saw the interrupt and finishes on its own
      let _ = job.is_finished();
    }
    let state = self.state.clone();
    let tx_tts = self.tx_tts.clone();
    let tts_done_rx = self.tts_done_rx.clone();
    let voice = state.voice.lock().unwrap().clone();
    let epoch = state.interrupt_counter.load(Ordering::SeqCst);
    state.tts_read_active.store(true, Ordering::Relaxed);
    self.tts_job = Some(thread::spawn(move || {
      for tts in phrases {
        if state.interrupt_counter.load(Ordering::SeqCst) != epoch {
          break;
        }
        let mut cleaned = tts;
        cleaned.push(' ');
        if tx_tts.send((cleaned, epoch, voice.clone())).is_err() {
          break;
        }
        let _ = tts_done_rx.recv_timeout(Duration::from_secs(60));
      }
      crate::conversation::wait_for_playback(&state, &state.interrupt_counter, epoch);
      state.tts_read_active.store(false, Ordering::Relaxed);
    }));
  }

  /// The text to attach to this message: what was selected since the previous
  /// one. X11 keeps serving the last selected text long after the highlight is
  /// gone and cannot say whether one is still visible, so a selection is only
  /// attached once. Select the text again to send it again.
  fn current_selection(&mut self) -> Option<String> {
    if !self.desktop.selection_present() {
      crate::log::log("debug", "nothing selected: no text attached");
      return None;
    }
    let age = self.desktop.selection_stamp();
    let attachment = self.desktop.read_selection()?;
    let already_sent = match age {
      // dated by the X server: only a selection newer than the last one counts
      SelectionAge::At(now) => Some(now) == self.sent_selection_stamp,
      // left over from before vtmate started watching: not for this message
      SelectionAge::Older => true,
      // undatable: the best we can do is refuse to send the same text twice
      SelectionAge::Unknown => self.sent_selection_text.as_ref() == Some(&attachment),
    };
    crate::log::log(
      "debug",
      &format!(
        "selection {:?} (last sent {:?}), {} characters",
        age,
        self.sent_selection_stamp,
        attachment.chars().count()
      ),
    );
    if already_sent {
      crate::log::log(
        "debug",
        "this selection was not made since the last message: select the text again to send it",
      );
      return None;
    }
    self.sent_selection_stamp = match age {
      SelectionAge::At(now) => Some(now),
      _ => None,
    };
    self.sent_selection_text = Some(attachment.clone());
    crate::log::log(
      "debug",
      &format!("attaching {} selected characters", attachment.chars().count()),
    );
    Some(attachment)
  }

  /// Reset hotkey, like ESC in the terminal: once stops the speech (and
  /// pauses a debate), twice within a second also resets the conversation.
  fn reset_hotkey(&mut self) {
    let now = Instant::now();
    let double = self
      .last_reset_press
      .is_some_and(|prev| now.duration_since(prev) <= Duration::from_millis(1000));
    if double {
      self.last_reset_press = None;
      self.reset_conversation();
      return;
    }
    self.last_reset_press = Some(now);
    self.interrupt_speech();
    let state = &self.state;
    if state.debate_enabled.load(Ordering::SeqCst) && !state.debate_paused.load(Ordering::SeqCst) {
      state.debate_paused.store(true, Ordering::SeqCst);
      self.ui_line("\n\x1b[32m🚩 Debate paused, speak again to continue \x1b[0m\n");
    }
    crate::log::log("debug", "speech stopped by hotkey (press again to reset)");
  }

  fn reset_conversation(&mut self) {
    self.interrupt_speech();
    self.last_reset_press = None;
    // the selection memory deliberately survives a reset: a selection you did
    // not touch is still not one you picked for the next message
    if matches!(
      self.active_ptt,
      Some(PttSource::HotkeyLlm) | Some(PttSource::HotkeyPaste)
    ) {
      // drop whatever was being recorded
      self.set_meta(UtteranceKind::Discard, None);
      self.state.recording_paused.store(true, Ordering::Relaxed);
      self.active_ptt = None;
      self.press_at = None;
    }
    let state = &self.state;
    if state.debate_enabled.load(Ordering::SeqCst) {
      state.debate_enabled.store(false, Ordering::SeqCst);
      state.debate_agents.lock().unwrap().clear();
      state.debate_turn.store(0, Ordering::SeqCst);
      *state.debate_subject.lock().unwrap() = String::new();
      state.debate_paused.store(false, Ordering::SeqCst);
    }
    state.reset_conversation();
    self.ui_line("");
    self.ui_line("\n\x1b[32m✨ Session restarted (history reset) \x1b[0m\n");
    crate::log::log("info", "conversation reset by hotkey");
    super::desktop::notify("vtmate", "Conversation restarted!");
  }

  fn on_action(&mut self, action: DaemonAction) {
    match action {
      DaemonAction::Paste(text) => {
        crate::log::log(
          "info",
          &format!("pasting {} characters", text.chars().count()),
        );
        self.desktop.paste_text(&text);
        self.ui_line(&format!(
          "\n\x1b[36m📋 Pasted: \x1b[0m{}",
          text.chars().take(80).collect::<String>()
        ));
      }
    }
  }

  fn on_client(&mut self, ev: ClientEvent) {
    match ev {
      ClientEvent::Key(k) => self.on_client_key(k),
      ClientEvent::Say { text, kind } => self.say(text, kind),
    }
  }

  /// A key from an attached terminal: same handling as the terminal mode,
  /// with the space push-to-talk arbitrated against the hotkeys.
  fn on_client_key(&mut self, k: crossterm::event::KeyEvent) {
    let is_space = k.code == KeyCode::Char(' ');
    if is_space
      && self.state.ptt.load(Ordering::Relaxed)
      && !self.state.debate_modal_visible.load(Ordering::SeqCst)
      && matches!(k.kind, KeyEventKind::Press | KeyEventKind::Repeat)
    {
      match self.active_ptt {
        Some(PttSource::AttachSpace) => {}
        Some(_) => return, // a hotkey holds the mic
        None => {
          self.active_ptt = Some(PttSource::AttachSpace);
          self.press_at = Some(Instant::now());
          self.interrupt_speech();
          self.set_meta(UtteranceKind::Llm, None);
        }
      }
    }
    let outcome = {
      let ctx = key_ctx!(self);
      handle_key(&k, &ctx, &mut self.key_state)
    };
    if let KeyOutcome::Quit = outcome {
      crate::log::log("debug", "Ctrl+C from client ignored (use --daemon-stop)");
    }
    self.settle_attach_space();
  }

  fn settle_attach_space(&mut self) {
    if self.active_ptt == Some(PttSource::AttachSpace)
      && self.state.recording_paused.load(Ordering::Relaxed)
    {
      self.active_ptt = None;
      self.press_at = None;
    }
  }

  /// Testing aid: process `text` as if transcribed from a hotkey utterance.
  fn say(&mut self, text: String, kind: UtteranceKind) {
    let attachment = if kind == UtteranceKind::Llm {
      self.current_selection()
    } else {
      None
    };
    if kind == UtteranceKind::Llm {
      self.interrupt_speech();
    }
    let utt = crate::audio::Utterance {
      audio: crate::audio::AudioChunk {
        data: Vec::new(),
        channels: 1,
        sample_rate: 16_000,
      },
      kind,
      attachment,
      text: Some(text),
    };
    if self
      .tx_utt
      .send_timeout(utt, Duration::from_secs(2))
      .is_err()
    {
      crate::log::log("warning", "conversation busy, Say dropped");
    }
  }

  fn tick(&mut self) {
    {
      let ctx = key_ctx!(self);
      self.key_state.tick(&ctx);
    }
    self.settle_attach_space();
    if let (Some(src), Some(at)) = (self.active_ptt, self.press_at) {
      if src != PttSource::AttachSpace && at.elapsed() > PTT_WATCHDOG {
        crate::log::log("warning", "push-to-talk held too long, releasing");
        self.ptt_release(src);
      }
    }
    if self.tts_job.as_ref().is_some_and(|j| j.is_finished()) {
      self.tts_job = None;
    }
    let _ = registry();
  }
}
