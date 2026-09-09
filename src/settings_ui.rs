// ------------------------------------------------------------------
//  Settings popup (Ctrl+S)
//
//  The agents of the settings file, edited from the terminal: a list, a form
//  per agent, and the two confirmations (delete an agent, throw away
//  changes). Everything is edited on a working copy; "Save" writes the file
//  and puts the agents it loaded back into the running state, so the next
//  utterance already uses them.
//
//  This module holds the model and the key handling; `render` draws it the
//  way `ui::render_debate_modal` does, over a dimmed conversation.
// ------------------------------------------------------------------

use crate::config::AgentSettings;
use crate::state::AppState;
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::{
  cursor::MoveTo,
  execute,
  style::Print,
  terminal::{self, Clear, ClearType},
};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::sync::atomic::Ordering;

// API
// ------------------------------------------------------------------

/// Which part of the popup is on screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Screen {
  /// The agent list, with the Save / Cancel buttons.
  #[default]
  List,
  /// The new / edit agent form.
  Form,
  /// "Delete <agent>?"
  ConfirmDelete,
  /// "Throw away the changes?" for the whole popup.
  ConfirmDiscard,
  /// The same, for the agent being filled in.
  ConfirmDiscardForm,
}

/// One editable value of an agent, in the order the form shows them. The TTS
/// engine comes before the language and the voice because it decides which
/// of those are on offer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Field {
  Name,
  Tts,
  Language,
  Voice,
  VoiceSpeed,
  Ptt,
  Provider,
  BaseUrl,
  Model,
  ApiKey,
  WhisperModel,
  Threshold,
  EndSilence,
  SystemPrompt,
}

pub const FIELDS: [Field; 14] = [
  Field::Name,
  Field::Tts,
  Field::Language,
  Field::Voice,
  Field::VoiceSpeed,
  Field::Ptt,
  Field::Provider,
  Field::BaseUrl,
  Field::Model,
  Field::ApiKey,
  Field::WhisperModel,
  Field::Threshold,
  Field::EndSilence,
  Field::SystemPrompt,
];

/// The TTS engines an agent can speak with.
pub const TTS_ENGINES: [&str; 4] = ["supertonic", "supersonic2", "kokoro", "opentts"];

/// Longest agent name the form accepts (what `validate_agent_name` allows).
pub const NAME_MAX: usize = 200;

/// The form being filled in, on top of the agent list.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Form {
  /// Position in the working list, or `None` for a new agent.
  pub editing: Option<usize>,
  /// The agent as it is being typed.
  pub draft: AgentSettings,
  /// The agent as the form opened on it, to know whether anything was typed.
  pub original: AgentSettings,
  /// Cursor: a `FIELDS` index, then the Done and Cancel buttons.
  pub cursor: usize,
  /// Character position inside the focused text field.
  pub caret: usize,
  /// What is wrong with the draft, shown under it.
  pub errors: Vec<String>,
}

impl Form {
  fn dirty(&self) -> bool {
    self.draft != self.original
  }
  fn on_done(&self) -> bool {
    self.cursor == FIELDS.len()
  }
  fn on_cancel(&self) -> bool {
    self.cursor == FIELDS.len() + 1
  }
  fn field(&self) -> Option<Field> {
    FIELDS.get(self.cursor).copied()
  }
}

/// The whole popup. Kept in `AppState` so the daemon can mirror it to an
/// attached terminal, which renders it from its own copy.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct SettingsUi {
  pub open: bool,
  pub screen: Screen,
  /// The agents being edited.
  pub agents: Vec<AgentSettings>,
  /// The agents as they are in the file, to know whether anything changed.
  pub saved: Vec<AgentSettings>,
  /// Cursor of the list: an agent row, then the Save and Cancel buttons.
  pub cursor: usize,
  pub form: Form,
  /// A line shown at the bottom of the popup (what went wrong, what was done).
  pub notice: Option<String>,
  /// `notice` is a problem, not a confirmation.
  pub notice_is_error: bool,
  /// Whether the microphone was already gated when the popup opened. It is
  /// gated while you type, so a LIVE agent does not answer what you say to
  /// yourself, and put back the way it was on the way out.
  pub was_recording_paused: bool,
}

impl SettingsUi {
  fn on_save(&self) -> bool {
    self.cursor == self.agents.len()
  }
  fn on_cancel(&self) -> bool {
    self.cursor == self.agents.len() + 1
  }
  fn dirty(&self) -> bool {
    self.agents != self.saved
  }
  fn set_error(&mut self, message: impl Into<String>) {
    self.notice = Some(message.into());
    self.notice_is_error = true;
  }
  fn set_info(&mut self, message: impl Into<String>) {
    self.notice = Some(message.into());
    self.notice_is_error = false;
  }
}

/// Open the popup on the agents of the running settings file. Returns the UI
/// messages to send (the popup, or why it cannot be opened).
pub fn open(state: &AppState) -> Vec<String> {
  let settings_path = state.settings_path.lock().unwrap().clone();
  if settings_path.as_os_str().is_empty() {
    return vec!["line|\n\x1b[31m❌ No settings file in use, nothing to edit\x1b[0m\n".to_string()];
  }
  // straight from the file: the running agents may carry command line
  // overrides (`--ptt`), and those must not be written back as settings
  let agents = match crate::config::try_load_settings(&settings_path, &crate::config::plain_args())
  {
    Ok(agents) => agents,
    Err(e) => {
      return vec![format!(
        "line|\n\x1b[31m❌ Cannot edit {}: {}\x1b[0m\n",
        settings_path.display(),
        e
      )];
    }
  };
  let mut ui = state.settings_ui.lock().unwrap();
  let current = state.agent_name.lock().unwrap().clone();
  *ui = SettingsUi {
    open: true,
    screen: Screen::List,
    cursor: agents.iter().position(|a| a.name == current).unwrap_or(0),
    saved: agents.clone(),
    agents,
    form: Form::default(),
    notice: None,
    notice_is_error: false,
    was_recording_paused: state.recording_paused.swap(true, Ordering::Relaxed),
  };
  vec!["settings_show|".to_string()]
}

/// Handle one key press while the popup is open. Returns the UI messages the
/// caller has to send; the settings lock is never held while they are sent.
pub fn handle_key(state: &AppState, k: &KeyEvent) -> Vec<String> {
  if k.kind == KeyEventKind::Release {
    return Vec::new();
  }
  let mut commit = false;
  let mut messages: Vec<String> = Vec::new();
  let mut restore_mic = None;
  {
    let mut ui = state.settings_ui.lock().unwrap();
    if !ui.open {
      return messages;
    }
    match ui.screen {
      Screen::List => list_key(&mut ui, k, &mut commit),
      Screen::Form => form_key(&mut ui, k),
      Screen::ConfirmDelete => confirm_delete_key(&mut ui, k),
      Screen::ConfirmDiscard | Screen::ConfirmDiscardForm => confirm_discard_key(&mut ui, k),
    }
    if !ui.open {
      messages.push("settings_hide|".to_string());
      restore_mic = Some(ui.was_recording_paused);
    } else {
      messages.push("settings_update|".to_string());
    }
  }
  // closed without saving: the microphone goes back to what it was doing
  // (a save applies the agent instead, which sets this itself)
  if let Some(paused) = restore_mic {
    state.recording_paused.store(paused, Ordering::Relaxed);
  }
  if commit {
    messages.extend(save(state));
  }
  messages
}

pub fn is_open(state: &AppState) -> bool {
  state.settings_ui.lock().unwrap().open
}

// Saving
// ------------------------------------------------------------------

/// Write the working copy to the settings file, load it back and make it the
/// running configuration. Anything rejected leaves the popup open with the
/// reason, and the file untouched.
fn save(state: &AppState) -> Vec<String> {
  let (agents, was, settings_path) = {
    let ui = state.settings_ui.lock().unwrap();
    (
      ui.agents.clone(),
      ui.saved.clone(),
      state.settings_path.lock().unwrap().clone(),
    )
  };

  let fail = |message: String| -> Vec<String> {
    state.settings_ui.lock().unwrap().set_error(message);
    vec!["settings_update|".to_string()]
  };

  if let Err(problem) = check_agents(&agents) {
    return fail(problem);
  }

  // the session stays on the agent it was on: by name, and by its place in
  // the list when that name was the one just renamed
  let current = state.agent_name.lock().unwrap().clone();
  let selected = agents
    .iter()
    .find(|a| a.name == current)
    .or_else(|| {
      was
        .iter()
        .position(|a| a.name == current)
        .and_then(|at| agents.get(at))
    })
    .or_else(|| agents.first())
    .cloned();
  let Some(selected) = selected else {
    return fail("at least one agent is needed".to_string());
  };

  if let Err(e) = crate::config::save_settings(&settings_path, &agents, &selected.name) {
    return fail(format!(
      "could not write {}: {}",
      settings_path.display(),
      e
    ));
  }

  // read the file back, so what runs is exactly what is on disk
  let mut reloaded =
    match crate::config::try_load_settings(&settings_path, &crate::config::plain_args()) {
      Ok(agents) => agents,
      Err(e) => return fail(format!("saved, but reading it back failed: {}", e)),
    };
  // the command line still has the last word, as it does at startup
  if let Some(ptt) = *state.ptt_override.lock().unwrap() {
    for agent in reloaded.iter_mut() {
      agent.ptt = ptt;
    }
  }
  let active = reloaded
    .iter()
    .find(|a| a.name == selected.name)
    .cloned()
    .unwrap_or(selected);
  *state.agents.lock().unwrap() = reloaded;
  state.apply_agent(&active);

  state.settings_ui.lock().unwrap().open = false;
  vec![
    "settings_hide|".to_string(),
    format!(
      "line|\n\x1b[32m💾 Settings saved - agent '\x1b[37m{}\x1b[32m' is live\x1b[0m\n",
      active.name
    ),
  ]
}

/// Everything that has to hold for the whole list, not only for one agent.
fn check_agents(agents: &[AgentSettings]) -> Result<(), String> {
  if agents.is_empty() {
    return Err("at least one agent is needed".to_string());
  }
  for (i, agent) in agents.iter().enumerate() {
    if let Some(problem) = crate::config::validate_agent(agent).first() {
      return Err(format!("'{}': {}", agent.name, problem));
    }
    if agents.iter().take(i).any(|other| other.name == agent.name) {
      return Err(format!("two agents are named '{}'", agent.name));
    }
  }
  Ok(())
}

// Key handling
// ------------------------------------------------------------------

fn list_key(ui: &mut SettingsUi, k: &KeyEvent, commit: &mut bool) {
  let last = ui.agents.len() + 1; // rows, then Save, then Cancel
  match k.code {
    KeyCode::Esc => {
      if ui.dirty() {
        ui.screen = Screen::ConfirmDiscard;
      } else {
        ui.open = false;
      }
    }
    KeyCode::Up => {
      ui.notice = None;
      ui.cursor = if ui.cursor == 0 { last } else { ui.cursor - 1 };
    }
    KeyCode::Down => {
      ui.notice = None;
      ui.cursor = if ui.cursor >= last { 0 } else { ui.cursor + 1 };
    }
    KeyCode::Tab => {
      // Tab is for leaving the list: it goes straight to Save, wherever the
      // cursor is among the agents. Stepping down through a long list to
      // reach the buttons is what ↑/↓ are for.
      ui.notice = None;
      ui.cursor = if ui.on_save() {
        last // Save -> Cancel
      } else if ui.on_cancel() {
        0 // Cancel -> back to the top of the list
      } else {
        ui.agents.len() // any agent row -> Save
      };
    }
    KeyCode::Left | KeyCode::Right => {
      // the two buttons sit side by side
      if ui.on_save() {
        ui.cursor += 1;
      } else if ui.on_cancel() {
        ui.cursor -= 1;
      }
    }
    KeyCode::Enter => {
      if ui.on_save() {
        *commit = true;
      } else if ui.on_cancel() {
        if ui.dirty() {
          ui.screen = Screen::ConfirmDiscard;
        } else {
          ui.open = false;
        }
      } else {
        edit_agent(ui);
      }
    }
    KeyCode::Char('s') | KeyCode::Char('S') if k.modifiers.contains(KeyModifiers::CONTROL) => {
      *commit = true;
    }
    KeyCode::Char(_) if k.modifiers.contains(KeyModifiers::CONTROL) => {}
    KeyCode::Char('n') | KeyCode::Char('N') => new_agent(ui),
    KeyCode::Char('e') | KeyCode::Char('E') => edit_agent(ui),
    KeyCode::Char('d') | KeyCode::Char('D') => {
      if ui.cursor < ui.agents.len() {
        if ui.agents.len() == 1 {
          ui.set_error("the last agent cannot be deleted");
        } else {
          ui.screen = Screen::ConfirmDelete;
        }
      }
    }
    _ => {}
  }
}

fn new_agent(ui: &mut SettingsUi) {
  // a new agent starts from the selected one: its provider, model and audio
  // levels are almost always what the next agent wants too
  let template = ui
    .agents
    .get(ui.cursor.min(ui.agents.len().saturating_sub(1)))
    .cloned()
    .unwrap_or_else(default_agent);
  let mut draft = AgentSettings {
    name: unique_name(&ui.agents, "new agent"),
    system_prompt_name: None,
    ..template
  };
  if draft.system_prompt.trim().is_empty() {
    draft.system_prompt = "You are a helpful assistant.".to_string();
  }
  ui.notice = None;
  ui.screen = Screen::Form;
  ui.form = Form {
    editing: None,
    caret: draft.name.chars().count(),
    original: draft.clone(),
    draft,
    ..Form::default()
  };
}

fn edit_agent(ui: &mut SettingsUi) {
  let Some(agent) = ui.agents.get(ui.cursor).cloned() else {
    return;
  };
  ui.notice = None;
  ui.screen = Screen::Form;
  ui.form = Form {
    editing: Some(ui.cursor),
    caret: agent.name.chars().count(),
    original: agent.clone(),
    draft: agent,
    ..Form::default()
  };
}

fn confirm_delete_key(ui: &mut SettingsUi, k: &KeyEvent) {
  match k.code {
    KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') => ui.screen = Screen::List,
    KeyCode::Enter | KeyCode::Char('y') | KeyCode::Char('Y') => {
      if ui.cursor < ui.agents.len() {
        let removed = ui.agents.remove(ui.cursor);
        ui.set_info(format!("'{}' removed - not saved yet", removed.name));
      }
      if ui.cursor > ui.agents.len() {
        ui.cursor = ui.agents.len();
      }
      ui.screen = Screen::List;
    }
    _ => {}
  }
}

fn confirm_discard_key(ui: &mut SettingsUi, k: &KeyEvent) {
  let from_form = ui.screen == Screen::ConfirmDiscardForm;
  match k.code {
    KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') => {
      ui.screen = if from_form {
        Screen::Form
      } else {
        Screen::List
      };
    }
    KeyCode::Enter | KeyCode::Char('y') | KeyCode::Char('Y') => {
      if from_form {
        // only the form is thrown away; the list keeps what was saved into it
        ui.screen = Screen::List;
        ui.form = Form::default();
      } else {
        ui.open = false;
      }
    }
    _ => {}
  }
}

/// Leave the form, asking first when something was typed into it.
fn leave_form(ui: &mut SettingsUi) {
  if ui.form.dirty() {
    ui.screen = Screen::ConfirmDiscardForm;
  } else {
    ui.screen = Screen::List;
    ui.form = Form::default();
  }
}

fn form_key(ui: &mut SettingsUi, k: &KeyEvent) {
  let last = FIELDS.len() + 1; // fields, then Done, then Cancel
  let field = ui.form.field();
  let in_text = matches!(
    field,
    Some(Field::Name)
      | Some(Field::BaseUrl)
      | Some(Field::Model)
      | Some(Field::ApiKey)
      | Some(Field::WhisperModel)
      | Some(Field::SystemPrompt)
  );

  match k.code {
    KeyCode::Esc => {
      leave_form(ui);
      return;
    }
    KeyCode::Tab => {
      // Tab leaves the fields for the buttons, the same way it does in the
      // list. Going through the fields one at a time is what ↑/↓ are for.
      ui.form.cursor = if ui.form.on_done() {
        last // Done -> Cancel
      } else if ui.form.on_cancel() {
        0 // Cancel -> back to the first field
      } else {
        FIELDS.len() // any field -> Done
      };
      place_caret(ui, true);
      return;
    }
    KeyCode::BackTab => {
      move_cursor(ui, false, last);
      return;
    }
    KeyCode::Up | KeyCode::Down => {
      let forward = k.code == KeyCode::Down;
      // inside the prompt the arrows walk its lines first
      if field == Some(Field::SystemPrompt) && move_caret_line(ui, forward) {
        return;
      }
      move_cursor(ui, forward, last);
      return;
    }
    KeyCode::Enter => {
      if ui.form.on_done() {
        commit_form(ui);
        return;
      }
      if ui.form.on_cancel() {
        leave_form(ui);
        return;
      }
      if field == Some(Field::SystemPrompt) {
        insert_char(ui, '\n');
        return;
      }
      move_cursor(ui, true, last);
      return;
    }
    _ => {}
  }

  let Some(field) = field else {
    // on a button: left / right walk between them
    match k.code {
      KeyCode::Left if ui.form.on_cancel() => ui.form.cursor -= 1,
      KeyCode::Right if ui.form.on_done() => ui.form.cursor += 1,
      _ => {}
    }
    return;
  };

  match k.code {
    KeyCode::Left if in_text => {
      ui.form.caret = ui.form.caret.saturating_sub(1);
    }
    KeyCode::Right if in_text => {
      let len = field_text(&ui.form.draft, field).chars().count();
      ui.form.caret = (ui.form.caret + 1).min(len);
    }
    KeyCode::Left => step_value(ui, field, -1),
    KeyCode::Right => step_value(ui, field, 1),
    KeyCode::Home if in_text => ui.form.caret = 0,
    KeyCode::End if in_text => {
      ui.form.caret = field_text(&ui.form.draft, field).chars().count();
    }
    KeyCode::Backspace if in_text => {
      if ui.form.caret > 0 {
        let caret = ui.form.caret - 1;
        let mut text = field_text(&ui.form.draft, field);
        remove_at(&mut text, caret);
        set_field_text(ui, field, text);
        ui.form.caret = caret;
      }
    }
    KeyCode::Delete if in_text => {
      let mut text = field_text(&ui.form.draft, field);
      if ui.form.caret < text.chars().count() {
        let caret = ui.form.caret;
        remove_at(&mut text, caret);
        set_field_text(ui, field, text);
      }
    }
    KeyCode::Char(c) if in_text && !k.modifiers.contains(KeyModifiers::CONTROL) => {
      insert_char(ui, c);
    }
    _ => {}
  }
}

fn move_cursor(ui: &mut SettingsUi, forward: bool, last: usize) {
  ui.form.cursor = if forward {
    if ui.form.cursor >= last {
      0
    } else {
      ui.form.cursor + 1
    }
  } else if ui.form.cursor == 0 {
    last
  } else {
    ui.form.cursor - 1
  };
  place_caret(ui, forward);
}

/// Where the caret lands when the cursor arrives on a field: at the end of a
/// one line value, so typing carries on from what is there, and on the near
/// edge of the system prompt - its first line when coming down into it, its
/// last when coming up. Landing on the far edge would send the very next
/// arrow press straight back out of the prompt, with no way to walk its lines
/// from the side the cursor came from.
fn place_caret(ui: &mut SettingsUi, forward: bool) {
  ui.form.caret = match ui.form.field() {
    Some(Field::SystemPrompt) if forward => 0,
    Some(field) => field_text(&ui.form.draft, field).chars().count(),
    None => 0,
  };
}

/// Move the caret one line up or down inside the prompt. `false` when there
/// is no such line, so the arrow moves to another field instead.
fn move_caret_line(ui: &mut SettingsUi, down: bool) -> bool {
  let text = ui.form.draft.system_prompt.clone();
  let chars: Vec<char> = text.chars().collect();
  let caret = ui.form.caret.min(chars.len());
  let line_start = chars[..caret]
    .iter()
    .rposition(|c| *c == '\n')
    .map_or(0, |i| i + 1);
  let column = caret - line_start;
  if down {
    let Some(rel) = chars[caret..].iter().position(|c| *c == '\n') else {
      return false;
    };
    let next_start = caret + rel + 1;
    let next_len = chars[next_start..]
      .iter()
      .position(|c| *c == '\n')
      .unwrap_or(chars.len() - next_start);
    ui.form.caret = next_start + column.min(next_len);
  } else {
    if line_start == 0 {
      return false;
    }
    let previous_start = chars[..line_start - 1]
      .iter()
      .rposition(|c| *c == '\n')
      .map_or(0, |i| i + 1);
    let previous_len = line_start - 1 - previous_start;
    ui.form.caret = previous_start + column.min(previous_len);
  }
  true
}

fn insert_char(ui: &mut SettingsUi, c: char) {
  let Some(field) = ui.form.field() else {
    return;
  };
  let mut text = field_text(&ui.form.draft, field);
  if field == Field::Name && text.chars().count() >= NAME_MAX {
    return;
  }
  let caret = ui.form.caret.min(text.chars().count());
  let byte = text
    .char_indices()
    .nth(caret)
    .map(|(i, _)| i)
    .unwrap_or(text.len());
  text.insert(byte, c);
  set_field_text(ui, field, text);
  ui.form.caret = caret + 1;
}

fn remove_at(text: &mut String, index: usize) {
  if let Some((byte, _)) = text.char_indices().nth(index) {
    text.remove(byte);
  }
}

/// Move a select one step, or a slider by one step of its range.
fn step_value(ui: &mut SettingsUi, field: Field, direction: i32) {
  let draft = &mut ui.form.draft;
  match field {
    Field::Tts => {
      let options: Vec<String> = TTS_ENGINES.iter().map(|s| s.to_string()).collect();
      draft.tts = cycle(&options, &draft.tts, direction);
      // the language and the voice belong to an engine: keep them possible
      let languages = languages_for(&draft.tts);
      if !languages.iter().any(|l| *l == draft.language) {
        draft.language = languages.first().cloned().unwrap_or_default();
      }
      let voices = crate::tts::get_voices_for(&draft.tts, &draft.language);
      if !voices.iter().any(|v| *v == draft.voice) {
        draft.voice = voices.first().cloned().unwrap_or_default();
      }
    }
    Field::Language => {
      let languages = languages_for(&draft.tts);
      draft.language = cycle(&languages, &draft.language, direction);
      let voices = crate::tts::get_voices_for(&draft.tts, &draft.language);
      if !voices.iter().any(|v| *v == draft.voice) {
        draft.voice = voices.first().cloned().unwrap_or_default();
      }
    }
    Field::Voice => {
      let voices = crate::tts::get_voices_for(&draft.tts, &draft.language);
      draft.voice = cycle(&voices, &draft.voice, direction);
    }
    Field::Provider => {
      draft.provider = cycle(&providers(), &draft.provider, direction);
    }
    Field::Ptt => draft.ptt = !draft.ptt,
    Field::VoiceSpeed => {
      let steps = ((draft.voice_speed - 1.0) * 10.0).round() as i32 + direction;
      draft.voice_speed = 1.0 + (steps.clamp(0, 80) as f32) / 10.0;
    }
    Field::Threshold => {
      let steps = (draft.sound_threshold_peak * 20.0).round() as i32 + direction;
      draft.sound_threshold_peak = (steps.clamp(0, 20) as f32) / 20.0;
    }
    Field::EndSilence => {
      let steps = (draft.end_silence_ms as i64 / END_SILENCE_STEP_MS as i64) as i32 + direction;
      draft.end_silence_ms = (steps.clamp(1, 40) as u64) * END_SILENCE_STEP_MS;
    }
    _ => {}
  }
}

/// The value after (or before) `current` in `options`, wrapping around. An
/// unknown current value lands on the first option.
fn cycle(options: &[String], current: &str, direction: i32) -> String {
  if options.is_empty() {
    return current.to_string();
  }
  let len = options.len() as i32;
  let at = options.iter().position(|o| o == current).unwrap_or(0) as i32;
  let next = ((at + direction) % len + len) % len;
  options[next as usize].clone()
}

/// Check the draft and put it back in the list.
fn commit_form(ui: &mut SettingsUi) {
  let mut draft = ui.form.draft.clone();
  draft.name = draft.name.trim().to_string();
  draft.baseurl = draft.baseurl.trim().to_string();
  draft.model = draft.model.trim().to_string();
  draft.api_key = draft.api_key.trim().to_string();
  draft.whisper_model_path = draft.whisper_model_path.trim().to_string();
  draft.system_prompt = draft.system_prompt.trim().to_string();

  let mut errors = crate::config::validate_agent(&draft);
  let clashes = ui
    .agents
    .iter()
    .enumerate()
    .any(|(i, other)| Some(i) != ui.form.editing && other.name == draft.name);
  if clashes {
    errors.push(format!("another agent is already named '{}'", draft.name));
  }
  if !errors.is_empty() {
    ui.form.errors = errors;
    return;
  }

  match ui.form.editing {
    Some(index) if index < ui.agents.len() => {
      ui.set_info(format!("'{}' edited - not saved yet", draft.name));
      ui.agents[index] = draft;
      ui.cursor = index;
    }
    _ => {
      ui.set_info(format!("'{}' added - not saved yet", draft.name));
      ui.agents.push(draft);
      ui.cursor = ui.agents.len() - 1;
    }
  }
  ui.screen = Screen::List;
  ui.form = Form::default();
}

// Field values
// ------------------------------------------------------------------

/// Slider granularity of `end_silence_ms`, in milliseconds.
pub const END_SILENCE_STEP_MS: u64 = 250;

impl Field {
  pub fn label(self) -> &'static str {
    match self {
      Field::Name => "Name",
      Field::Tts => "TTS engine",
      Field::Language => "Language",
      Field::Voice => "Voice",
      Field::VoiceSpeed => "Voice speed",
      Field::Ptt => "Push to talk",
      Field::Provider => "LLM provider",
      Field::BaseUrl => "Base url",
      Field::Model => "LLM model",
      Field::ApiKey => "Api key",
      Field::WhisperModel => "Whisper model",
      Field::Threshold => "Mic threshold",
      Field::EndSilence => "End silence",
      Field::SystemPrompt => "System prompt",
    }
  }

  /// The one line of instructions shown while the field has the focus.
  pub fn hint(self) -> &'static str {
    match self {
      Field::Name => "type a name, up to 200 characters - it labels the agent in the status bar",
      Field::Tts => "←/→ pick the voice engine; it decides the languages and voices below",
      Field::Language => "←/→ pick a language of this engine; also used to transcribe you",
      Field::Voice => "←/→ pick a voice of this engine and language",
      Field::VoiceSpeed => "←/→ how fast the agent speaks (1.0 to 9.0)",
      Field::Ptt => "←/→ ON holds SPACE to talk, OFF listens and cuts on silence (LIVE)",
      Field::Provider => "←/→ where the answers come from; hosted ones need an api key",
      Field::BaseUrl => "host and port of a local server, empty for a hosted provider's default",
      Field::Model => "model name as the provider spells it, e.g. llama3.2:3b",
      Field::ApiKey => "hosted providers only; empty falls back to the provider's env var",
      Field::WhisperModel => "path of the whisper .bin used to transcribe (applies on restart)",
      Field::Threshold => "←/→ how loud you have to be to be heard (LIVE mode)",
      Field::EndSilence => "←/→ silence that ends what you say (LIVE mode)",
      Field::SystemPrompt => {
        "what the agent is; ENTER makes a new line, over 5 lines is stored as a block"
      }
    }
  }

  fn is_slider(self) -> bool {
    matches!(
      self,
      Field::VoiceSpeed | Field::Threshold | Field::EndSilence
    )
  }

  fn is_select(self) -> bool {
    matches!(
      self,
      Field::Tts | Field::Language | Field::Voice | Field::Provider | Field::Ptt
    )
  }
}

fn field_text(agent: &AgentSettings, field: Field) -> String {
  match field {
    Field::Name => agent.name.clone(),
    Field::BaseUrl => agent.baseurl.clone(),
    Field::Model => agent.model.clone(),
    Field::ApiKey => agent.api_key.clone(),
    Field::WhisperModel => agent.whisper_model_path.clone(),
    Field::SystemPrompt => agent.system_prompt.clone(),
    _ => String::new(),
  }
}

fn set_field_text(ui: &mut SettingsUi, field: Field, value: String) {
  let draft = &mut ui.form.draft;
  match field {
    Field::Name => draft.name = value,
    Field::BaseUrl => draft.baseurl = value,
    Field::Model => draft.model = value,
    Field::ApiKey => draft.api_key = value,
    Field::WhisperModel => draft.whisper_model_path = value,
    Field::SystemPrompt => draft.system_prompt = value,
    _ => {}
  }
}

/// The languages an engine can speak, i.e. those it has voices for.
pub fn languages_for(tts: &str) -> Vec<String> {
  crate::tts::get_all_available_languages()
    .into_iter()
    .filter(|lang| !crate::tts::get_voices_for(tts, lang).is_empty())
    .map(|lang| lang.to_string())
    .collect()
}

/// Every provider vtmate can talk to, local servers first.
pub fn providers() -> Vec<String> {
  crate::llm::LOCAL_PROVIDERS
    .iter()
    .chain(crate::llm::CLOUD_PROVIDERS.iter())
    .map(|p| p.to_string())
    .collect()
}

/// A name no agent of the list uses yet ("new agent", "new agent 2"...).
fn unique_name(agents: &[AgentSettings], wanted: &str) -> String {
  let mut name = wanted.to_string();
  let mut suffix = 2;
  while agents.iter().any(|a| a.name == name) {
    name = format!("{} {}", wanted, suffix);
    suffix += 1;
  }
  name
}

/// The agent a brand new one is built from when the list is empty.
fn default_agent() -> AgentSettings {
  let tts = "supertonic".to_string();
  let language = languages_for(&tts)
    .first()
    .cloned()
    .unwrap_or_else(|| "en".to_string());
  let voice = crate::tts::get_voices_for(&tts, &language)
    .first()
    .cloned()
    .unwrap_or_default();
  AgentSettings {
    name: "new agent".to_string(),
    language,
    tts,
    voice,
    provider: "ollama".to_string(),
    baseurl: "http://127.0.0.1:11434".to_string(),
    model: "llama3.2:3b".to_string(),
    api_key: String::new(),
    system_prompt: "You are a helpful assistant.".to_string(),
    ptt: true,
    whisper_model_path: "~/.whisper-models/ggml-tiny.bin".to_string(),
    sound_threshold_peak: 0.12,
    end_silence_ms: 2500,
    voice_speed: 1.1,
    system_prompt_name: None,
  }
}

// Rendering
// ------------------------------------------------------------------

const BG: &str = "\x1b[48;5;234m";
const BG_ROW: &str = "\x1b[48;5;238m";
const FG: &str = "\x1b[97m";
const DIM: &str = "\x1b[90m";
const YELLOW: &str = "\x1b[33m";
const RED: &str = "\x1b[91m";
const GREEN: &str = "\x1b[92m";
const CYAN: &str = "\x1b[96m";
/// Back to the default foreground, keeping the popup's background.
const OFF: &str = "\x1b[39m";

/// Columns of the agent list: heading, narrowest it may get, and the width it
/// wants. The system prompt takes whatever room is left after them.
const COLUMNS: [(&str, usize, usize); 8] = [
  ("NAME", 12, 12),
  ("LANG", 4, 4),
  ("MODE", 4, 4),
  ("TTS", 6, 11),
  ("VOICE", 5, 8),
  ("SPD", 4, 4),
  ("PROVIDER", 8, 17),
  ("MODEL", 7, 14),
];

/// Narrowest system prompt column worth showing.
const PROMPT_MIN: usize = 10;

/// Draw the popup of the running session over a dimmed copy of the
/// conversation, or nothing when it is closed.
pub fn render<W: Write>(out: &mut W, buffer: &[String]) {
  let Some(state) = crate::state::GLOBAL_STATE.get() else {
    return;
  };
  let ui = state.settings_ui.lock().unwrap().clone();
  draw(out, &ui, buffer);
}

/// Draw one popup. Split out of `render` so it can be exercised on a
/// `SettingsUi` of its own, without a running session.
pub fn draw<W: Write>(out: &mut W, ui: &SettingsUi, buffer: &[String]) {
  if !ui.open {
    return;
  }
  let ui = ui.clone();
  let (cols, rows) = terminal::size().unwrap_or((80, 24));
  if cols < 34 || rows < 10 {
    return;
  }
  let width = cols.saturating_sub(4).min(120).max(30);
  let inner = width as usize - 4;

  let (title, lines, footer) = match ui.screen {
    Screen::List => list_lines(&ui, inner, rows),
    Screen::Form => form_lines(&ui, inner, rows),
    Screen::ConfirmDelete | Screen::ConfirmDiscard | Screen::ConfirmDiscardForm => {
      confirm_lines(&ui, inner)
    }
  };

  // the bottom bar keeps the last row; the footer (buttons, shortcuts) is
  // never the part that gets cut when the terminal is short
  let room = (rows.saturating_sub(1) as usize).saturating_sub(2);
  let footer_len = footer.len().min(room);
  let lines: Vec<String> = lines.into_iter().take(room - footer_len).collect();
  let height = (lines.len() + footer_len + 2) as u16;
  let x = (cols - width) / 2;
  let y = (rows.saturating_sub(1)).saturating_sub(height) / 2;

  // the conversation stays visible behind the popup, dimmed
  execute!(out, Clear(ClearType::All), MoveTo(0, 0)).unwrap();
  let visible = rows.saturating_sub(1) as usize;
  let view_start = buffer.len().saturating_sub(visible);
  for (i, line) in buffer.iter().enumerate().skip(view_start).take(visible) {
    execute!(
      out,
      MoveTo(0, (i - view_start) as u16),
      Clear(ClearType::CurrentLine),
      Print(format!("{}{}\x1b[0m", DIM, strip_ansi(line)))
    )
    .unwrap();
  }

  let title_bar = pad(
    &format!("{}\x1b[1m {} \x1b[22m{}", FG, title, OFF),
    width as usize - 2,
    '─',
  );
  execute!(
    out,
    MoveTo(x, y),
    Print(format!("{}{}┌{}┐\x1b[0m", BG, FG, title_bar))
  )
  .unwrap();

  let body: Vec<String> = lines
    .into_iter()
    .chain(footer.into_iter().take(footer_len))
    .collect();
  for (i, line) in body.iter().enumerate() {
    execute!(
      out,
      MoveTo(x, y + 1 + i as u16),
      Print(format!(
        "{}{}│{} {} {}{}│\x1b[0m",
        BG,
        FG,
        OFF,
        pad(line, inner, ' '),
        FG,
        BG
      ))
    )
    .unwrap();
  }
  execute!(
    out,
    MoveTo(x, y + height - 1),
    Print(format!(
      "{}{}└{}┘\x1b[0m",
      BG,
      FG,
      "─".repeat(width as usize - 2)
    ))
  )
  .unwrap();
  out.flush().unwrap();
}

fn list_lines(ui: &SettingsUi, inner: usize, rows: u16) -> (String, Vec<String>, Vec<String>) {
  let (widths, prompt_width) = column_widths(inner);
  let mut header = String::new();
  for (i, (name, _, _)) in COLUMNS.iter().enumerate() {
    if let Some(w) = widths.get(i) {
      header.push_str(&format!("{:<width$} ", cut(name, *w), width = *w));
    }
  }
  if let Some(width) = prompt_width {
    header.push_str(&cut("PROMPT", width));
  }

  // Room for the agent rows. `draw` gives the body `rows - 3` lines (the
  // bottom bar and the two borders are its own), and out of those the footer
  // takes its separator, shortcuts, buttons and any notice, while the list
  // itself spends one line on the header, one under it, one on the scroll
  // hint and one above the footer. Reserving the hint line whether or not it
  // is used keeps the count from depending on its own outcome.
  let footer_len = 3 + usize::from(ui.notice.is_some());
  let room = (rows as usize).saturating_sub(7 + footer_len).max(1);
  let scroll = scroll_for(ui.cursor.min(ui.agents.len()), ui.agents.len(), room);

  let mut lines = vec![format!("{}{}{}", DIM, header, OFF), String::new()];
  for (i, agent) in ui.agents.iter().enumerate().skip(scroll).take(room) {
    lines.push(agent_row(
      agent,
      &widths,
      prompt_width,
      i == ui.cursor,
      inner,
    ));
  }
  if ui.agents.len() > room {
    // which slice of the list is on screen, so a long list can be walked
    // without losing track of where in it the cursor is
    lines.push(format!(
      "{}  showing {}-{} of {} - ↑/↓ to scroll{}",
      DIM,
      scroll + 1,
      (scroll + room).min(ui.agents.len()),
      ui.agents.len(),
      OFF
    ));
  }
  lines.push(String::new());

  let mut footer = vec![
    format!("{}{}{}", DIM, "─".repeat(inner), OFF),
    format!(
      "{}n{} new agent   {}e{} edit agent   {}d{} delete agent   {}↑/↓{} move",
      YELLOW, DIM, YELLOW, DIM, YELLOW, DIM, FG, DIM
    ),
    format!(
      "  {}   {}",
      button("Save", ui.on_save()),
      button("Cancel", ui.on_cancel())
    ),
  ];
  if let Some(notice) = &ui.notice {
    footer.push(format!(
      "{}{}{}",
      if ui.notice_is_error { RED } else { GREEN },
      cut(notice, inner),
      OFF
    ));
  }
  (
    format!("Settings - {} agents", ui.agents.len()),
    lines,
    footer,
  )
}

fn agent_row(
  agent: &AgentSettings,
  widths: &[usize],
  prompt_width: Option<usize>,
  selected: bool,
  inner: usize,
) -> String {
  let cells = [
    agent.name.clone(),
    agent.language.clone(),
    if agent.ptt { "PTT" } else { "LIVE" }.to_string(),
    agent.tts.clone(),
    agent.voice.clone(),
    format!("{:.1}x", agent.voice_speed),
    agent.provider.clone(),
    agent.model.clone(),
  ];
  let mut row = String::new();
  for (i, cell) in cells.iter().enumerate() {
    let Some(w) = widths.get(i) else { break };
    let text = cut(cell, *w);
    if i == 0 {
      row.push_str(&format!("{}{:<width$}{} ", YELLOW, text, OFF, width = *w));
    } else {
      row.push_str(&format!("{:<width$} ", text, width = *w));
    }
  }
  if let Some(width) = prompt_width {
    let prompt = agent.system_prompt.replace('\n', " ");
    row.push_str(&format!("{}{}{}", DIM, cut(&prompt, width), OFF));
  }
  if selected {
    format!("{}{}{}{}", BG_ROW, pad(&row, inner, ' '), BG, OFF)
  } else {
    row
  }
}

/// How wide each column of the list gets in `inner` characters: the widths
/// they want, shrunk towards their minimum to make room for the system
/// prompt, and columns dropped from the right only when even that is not
/// enough. The second value is the room left for the prompt.
fn column_widths(inner: usize) -> (Vec<usize>, Option<usize>) {
  let mut widths: Vec<usize> = COLUMNS.iter().map(|(_, _, wanted)| *wanted).collect();
  // one space follows every column
  let total = |widths: &Vec<usize>| widths.iter().sum::<usize>() + widths.len();

  while total(&widths) + PROMPT_MIN > inner {
    // give back a character of the widest column that can still spare one
    let widest = widths
      .iter()
      .enumerate()
      .filter(|(i, w)| **w > COLUMNS[*i].1)
      .max_by_key(|(_, w)| **w)
      .map(|(i, _)| i);
    match widest {
      Some(i) => widths[i] -= 1,
      None => break,
    }
  }
  while total(&widths) > inner && widths.len() > 1 {
    widths.pop();
  }
  let left = inner.saturating_sub(total(&widths));
  (widths, (left >= PROMPT_MIN).then_some(left))
}

fn form_lines(ui: &SettingsUi, inner: usize, rows: u16) -> (String, Vec<String>, Vec<String>) {
  let form = &ui.form;
  let label_width = 15usize;
  let value_width = inner.saturating_sub(label_width + 1).max(10);
  let mut lines: Vec<String> = Vec::new();
  // where each field starts, so the focused one can be scrolled into view
  let mut anchors: Vec<usize> = Vec::new();

  for (i, field) in FIELDS.iter().enumerate() {
    let focused = form.cursor == i;
    anchors.push(lines.len());
    let label = format!(
      "{}{:<width$}{}",
      if focused { FG } else { DIM },
      cut(field.label(), label_width),
      OFF,
      width = label_width
    );
    if *field == Field::SystemPrompt {
      lines.push(format!("{} {}", label, prompt_summary(&form.draft)));
      let caret = if focused { form.caret } else { 0 };
      for line in prompt_box(&form.draft.system_prompt, caret, focused, value_width) {
        lines.push(format!("{:<width$} {}", "", line, width = label_width));
      }
    } else {
      lines.push(format!(
        "{} {}",
        label,
        field_value(&form.draft, *field, focused, form.caret, value_width)
      ));
    }
    if focused {
      lines.push(format!(
        "{:<width$} {}{}{}",
        "",
        CYAN,
        cut(field.hint(), value_width),
        OFF,
        width = label_width
      ));
    }
  }
  anchors.push(lines.len());

  // keep the focused field (and its hint) on screen
  let reserved = 7;
  let room = (rows.saturating_sub(1) as usize)
    .saturating_sub(reserved)
    .max(3);
  let anchor = anchors
    .get(form.cursor.min(FIELDS.len()))
    .copied()
    .unwrap_or(0);
  let end = anchors
    .get(form.cursor + 1)
    .copied()
    .unwrap_or(lines.len())
    .min(lines.len());
  let mut start = 0usize;
  if end > room {
    start = end - room;
  }
  if start > anchor {
    start = anchor;
  }
  let visible: Vec<String> = lines.into_iter().skip(start).take(room).collect();

  let mut footer = vec![
    format!("{}{}{}", DIM, "─".repeat(inner), OFF),
    format!(
      "{}↑/↓{} field   {}←/→{} change   {}TAB{} next   {}ESC{} back",
      FG, DIM, FG, DIM, FG, DIM, FG, DIM
    ),
    format!(
      "  {}   {}",
      button("Done", form.on_done()),
      button("Cancel", form.on_cancel())
    ),
  ];
  for problem in form.errors.iter().take(3) {
    footer.push(format!("{}⚠ {}{}", RED, cut(problem, inner - 2), OFF));
  }

  let title = match form.editing {
    Some(_) => format!("Edit agent - {}", cut(&form.draft.name, 24)),
    None => "New agent".to_string(),
  };
  (title, visible, footer)
}

/// The rendered value of one field: a box for text, `◀ value ▶` for a select,
/// a bar for a slider.
fn field_value(
  agent: &AgentSettings,
  field: Field,
  focused: bool,
  caret: usize,
  width: usize,
) -> String {
  if field.is_slider() {
    let (position, text) = match field {
      Field::VoiceSpeed => (
        ((agent.voice_speed - 1.0) / 8.0).clamp(0.0, 1.0),
        format!("{:.1}x", agent.voice_speed),
      ),
      Field::Threshold => (
        agent.sound_threshold_peak.clamp(0.0, 1.0),
        format!("{:.2}", agent.sound_threshold_peak),
      ),
      _ => (
        (agent.end_silence_ms as f32 / 10_000.0).clamp(0.0, 1.0),
        format!(
          "{}s",
          format!("{:.2}", agent.end_silence_ms as f32 / 1000.0)
            .trim_end_matches('0')
            .trim_end_matches('.')
        ),
      ),
    };
    return slider(position, &text, focused, width);
  }
  if field.is_select() {
    let (value, options, at) = match field {
      Field::Tts => {
        let options: Vec<String> = TTS_ENGINES.iter().map(|s| s.to_string()).collect();
        let at = options.iter().position(|o| *o == agent.tts);
        (agent.tts.clone(), options, at)
      }
      Field::Language => {
        let options = languages_for(&agent.tts);
        let at = options.iter().position(|o| *o == agent.language);
        (agent.language.clone(), options, at)
      }
      Field::Voice => {
        let options = crate::tts::get_voices_for(&agent.tts, &agent.language);
        let at = options.iter().position(|o| *o == agent.voice);
        (agent.voice.clone(), options, at)
      }
      Field::Provider => {
        let options = providers();
        let at = options.iter().position(|o| *o == agent.provider);
        (agent.provider.clone(), options, at)
      }
      _ => {
        let options = vec!["OFF".to_string(), "ON".to_string()];
        let value = if agent.ptt { "ON" } else { "OFF" }.to_string();
        let at = Some(agent.ptt as usize);
        (value, options, at)
      }
    };
    let counter = match at {
      Some(i) => format!("{}/{}", i + 1, options.len()),
      None => format!("?/{}", options.len()),
    };
    let arrows = if focused { FG } else { DIM };
    // padded, so the ▶ and the counter do not jump as the value changes
    let body_width = width.saturating_sub(counter.chars().count() + 6);
    let body = pad(&cut(&value, body_width), body_width, ' ');
    return format!(
      "{}◀{} {}{}{} {}▶{} {}{}{}",
      arrows,
      OFF,
      if focused { FG } else { OFF },
      body,
      OFF,
      arrows,
      OFF,
      DIM,
      counter,
      OFF
    );
  }
  let text = field_text(agent, field);
  let shown = if field == Field::ApiKey && !text.is_empty() && !focused {
    "•".repeat(text.chars().count().min(24))
  } else {
    text
  };
  text_box(&shown, if focused { caret } else { 0 }, focused, width)
}

/// A one line text input, scrolled so the caret is always visible.
fn text_box(text: &str, caret: usize, focused: bool, width: usize) -> String {
  let room = width.saturating_sub(2).max(4);
  let chars: Vec<char> = text.chars().collect();
  let caret = caret.min(chars.len());
  let start = caret.saturating_sub(room.saturating_sub(1));
  let slice: String = chars.iter().skip(start).take(room).collect();
  let body = if focused {
    let at = caret - start;
    let mut shown = String::new();
    for (i, c) in slice.chars().enumerate() {
      if i == at {
        shown.push_str(&format!("\x1b[7m{}\x1b[27m", c));
      } else {
        shown.push(c);
      }
    }
    if at >= slice.chars().count() {
      shown.push_str("\x1b[7m \x1b[27m");
    }
    shown
  } else {
    slice
  };
  format!(
    "{}[{}{}{}]{}",
    if focused { FG } else { DIM },
    OFF,
    pad(&body, room, ' '),
    if focused { FG } else { DIM },
    OFF
  )
}

/// The prompt as an editable block, wrapped to the width of the box so no
/// part of a long line is hidden, and scrolled to wherever the caret is.
fn prompt_box(text: &str, caret: usize, focused: bool, width: usize) -> Vec<String> {
  let room = width.saturating_sub(2).max(8);
  let max_rows = 6usize;

  // every line of the prompt, cut into rows of `room` characters, with the
  // offset each row starts at so the caret can be placed on one of them
  let mut rows: Vec<(usize, String)> = Vec::new();
  let mut caret_row = 0usize;
  let mut offset = 0usize;
  for line in text.split('\n') {
    let chars: Vec<char> = line.chars().collect();
    let mut at = 0usize;
    loop {
      let take = room.min(chars.len() - at);
      let start = offset + at;
      // the caret sits on this row when it falls inside it, or right at its
      // end when the line does not wrap any further
      if caret >= start
        && caret <= start + take
        && (caret < start + take || at + take >= chars.len())
      {
        caret_row = rows.len();
      }
      rows.push((start, chars[at..at + take].iter().collect()));
      at += take;
      if at >= chars.len() {
        break;
      }
    }
    offset += chars.len() + 1; // the line break
  }

  let first = caret_row
    .saturating_sub(max_rows - 1)
    .min(rows.len().saturating_sub(max_rows.min(rows.len())));
  let mut out = Vec::new();
  for (i, (start, row)) in rows.iter().enumerate().skip(first).take(max_rows) {
    let body = if focused && i == caret_row {
      let at = caret.saturating_sub(*start);
      let mut shown = String::new();
      for (index, c) in row.chars().enumerate() {
        if index == at {
          shown.push_str(&format!("\x1b[7m{}\x1b[27m", c));
        } else {
          shown.push(c);
        }
      }
      if at >= row.chars().count() {
        shown.push_str("\x1b[7m \x1b[27m");
      }
      shown
    } else {
      row.clone()
    };
    out.push(format!(
      "{}│{} {}",
      if focused { FG } else { DIM },
      OFF,
      pad(&body, room, ' ')
    ));
  }
  if rows.len() > first + max_rows {
    out.push(format!(
      "{}│{} {}{} more rows below{}",
      if focused { FG } else { DIM },
      OFF,
      DIM,
      rows.len() - first - max_rows,
      OFF
    ));
  }
  out
}

/// How the prompt will be written to the settings file.
fn prompt_summary(agent: &AgentSettings) -> String {
  let lines = agent.system_prompt.split('\n').count();
  let inline = lines <= crate::config::INLINE_PROMPT_MAX_LINES;
  format!(
    "{}{} line{} - saved {}{}",
    DIM,
    lines,
    if lines == 1 { "" } else { "s" },
    if inline {
      "inline"
    } else {
      "as a [system_prompt] block"
    },
    OFF
  )
}

fn slider(position: f32, text: &str, focused: bool, width: usize) -> String {
  let room = width.saturating_sub(text.chars().count() + 4).clamp(8, 40);
  let filled = ((position * room as f32).round() as usize).min(room.saturating_sub(1));
  let mut bar = String::new();
  for i in 0..room {
    if i == filled {
      bar.push('●');
    } else if i < filled {
      bar.push('━');
    } else {
      bar.push('─');
    }
  }
  format!(
    "{}{}{} {}{}{}",
    if focused { GREEN } else { DIM },
    bar,
    OFF,
    if focused { FG } else { OFF },
    text,
    OFF
  )
}

fn button(text: &str, focused: bool) -> String {
  if focused {
    format!("\x1b[7m{}[ {} ]\x1b[27m{}", FG, text, OFF)
  } else {
    format!("{}[ {} ]{}", DIM, text, OFF)
  }
}

fn confirm_lines(ui: &SettingsUi, inner: usize) -> (String, Vec<String>, Vec<String>) {
  let (title, question, detail) = match ui.screen {
    Screen::ConfirmDelete => {
      let name = ui
        .agents
        .get(ui.cursor)
        .map(|a| a.name.clone())
        .unwrap_or_default();
      (
        "Delete agent".to_string(),
        format!("Delete the agent '{}'?", name),
        "It is removed from the list; the file is only written when you save.".to_string(),
      )
    }
    Screen::ConfirmDiscardForm => (
      "Unsaved agent".to_string(),
      format!("Throw away the changes to '{}'?", ui.form.draft.name),
      "The agent goes back to what the list holds for it.".to_string(),
    ),
    _ => (
      "Unsaved changes".to_string(),
      "Throw away the changes?".to_string(),
      "The settings file keeps what it has now.".to_string(),
    ),
  };
  let lines = vec![
    String::new(),
    format!("  {}{}{}", YELLOW, cut(&question, inner - 2), OFF),
    String::new(),
    format!("  {}{}{}", DIM, cut(&detail, inner - 2), OFF),
    String::new(),
  ];
  let footer = vec![
    format!("{}{}{}", DIM, "─".repeat(inner), OFF),
    format!(
      "  {}y{} / {}ENTER{} yes      {}n{} / {}ESC{} no",
      YELLOW, DIM, FG, DIM, YELLOW, DIM, FG, DIM
    ),
  ];
  (title, lines, footer)
}

// Text helpers
// ------------------------------------------------------------------

/// `text` cut to `max` visible characters, ending in '...' when it did not fit.
pub fn cut(text: &str, max: usize) -> String {
  let count = text.chars().count();
  if count <= max {
    return text.to_string();
  }
  if max <= 3 {
    return text.chars().take(max).collect();
  }
  let mut out: String = text.chars().take(max - 3).collect();
  out.push_str("...");
  out
}

/// `text` brought to exactly `width` visible characters: cut when it is
/// longer, padded with `fill` when it is shorter. Colour codes are copied
/// through and take no room; the caller ends every row with a full reset, so
/// a code left open by the cut cannot leak into the next one.
fn pad(text: &str, width: usize, fill: char) -> String {
  let mut out = String::with_capacity(text.len());
  let mut visible = 0usize;
  let mut chars = text.chars();
  while let Some(c) = chars.next() {
    if c == '\x1b' {
      out.push(c);
      for next in chars.by_ref() {
        out.push(next);
        if next.is_ascii_alphabetic() {
          break;
        }
      }
      continue;
    }
    if visible == width {
      break;
    }
    out.push(c);
    visible += 1;
  }
  out.extend(std::iter::repeat_n(fill, width.saturating_sub(visible)));
  out
}

/// The conversation lines behind the popup are dimmed as a whole, so their
/// own colours have to go.
fn strip_ansi(text: &str) -> String {
  let mut out = String::with_capacity(text.len());
  let mut chars = text.chars();
  while let Some(c) = chars.next() {
    if c == '\x1b' {
      for next in chars.by_ref() {
        if next.is_ascii_alphabetic() {
          break;
        }
      }
    } else {
      out.push(c);
    }
  }
  out
}

/// First row to show so that `cursor` is inside a window of `room` rows.
fn scroll_for(cursor: usize, total: usize, room: usize) -> usize {
  if total <= room || cursor < room {
    return 0;
  }
  (cursor + 1 - room).min(total.saturating_sub(room))
}
