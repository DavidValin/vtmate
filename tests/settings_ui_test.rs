// Drives the settings popup with the real module: stubs for the parts of the
// running session it touches, the real config.rs underneath it.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

mod tts {
  pub fn get_all_available_languages() -> Vec<&'static str> {
    vec!["de", "en", "es"]
  }
  pub fn get_voices_for(tts: &str, lang: &str) -> Vec<String> {
    match (tts, lang) {
      ("supertonic", "en") => vec!["M1".into(), "M2".into(), "F1".into()],
      ("supertonic", "es") => vec!["M1".into()],
      ("kokoro", "en") => vec!["bf_alice".into(), "am_puck".into()],
      _ => vec![],
    }
  }
  pub fn voice_styles_dir_for(_tts: &str) -> Option<std::path::PathBuf> {
    None
  }
}

mod util {
  use std::path::PathBuf;
  pub fn get_user_home_path() -> Option<PathBuf> {
    Some(PathBuf::from("/tmp"))
  }
  pub fn terminate(code: i32) -> ! {
    std::process::exit(code)
  }
}

mod log {
  pub fn log(_level: &str, _msg: &str) {}
}

mod llm {
  pub const LOCAL_PROVIDERS: &[&str] = &["ollama", "llama-server", "openai-compatible"];
  pub const CLOUD_PROVIDERS: &[&str] = &["openai", "anthropic"];
  pub fn is_local_provider(p: &str) -> bool {
    LOCAL_PROVIDERS.contains(&p)
  }
  pub fn is_cloud_provider(p: &str) -> bool {
    CLOUD_PROVIDERS.contains(&p)
  }
  pub fn is_supported_provider(p: &str) -> bool {
    is_local_provider(p) || is_cloud_provider(p)
  }
  pub fn supported_providers_list() -> String {
    "ollama, openai".to_string()
  }
  pub fn api_key_env_var(_p: &str) -> Option<&'static str> {
    Some("API_KEY")
  }
  pub fn resolve_api_key(_p: &str, configured: &str) -> Option<String> {
    (!configured.trim().is_empty()).then(|| configured.to_string())
  }
}

mod state {
  use super::*;
  use crate::config::AgentSettings;

  pub static GLOBAL_STATE: OnceLock<Arc<AppState>> = OnceLock::new();

  #[derive(Debug, Default)]
  pub struct AppState {
    pub settings_ui: Arc<Mutex<crate::settings_ui::SettingsUi>>,
    pub settings_path: Arc<Mutex<std::path::PathBuf>>,
    pub agent_name: Arc<Mutex<String>>,
    pub agents: Arc<Mutex<Vec<AgentSettings>>>,
    pub recording_paused: Arc<AtomicBool>,
    pub ptt_override: Mutex<Option<bool>>,
    pub applied: Mutex<Vec<String>>,
  }

  impl AppState {
    pub fn apply_agent(&self, agent: &AgentSettings) {
      *self.agent_name.lock().unwrap() = agent.name.clone();
      self.applied.lock().unwrap().push(agent.name.clone());
      self.recording_paused.store(agent.ptt, Ordering::Relaxed);
    }
  }
}

#[path = "../src/config.rs"]
mod config;
#[path = "../src/settings_ui.rs"]
mod settings_ui;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use settings_ui::{Screen, SettingsUi};
use state::AppState;

const SETTINGS: &str = r#"[general]
selected_agent = main agent

[daemon]
llm_background_ptt_combo = ctrl+alt+a

[agent]
name = main agent
language = en
tts = supertonic
voice = M1
voice_speed = 1.1
provider = ollama
baseurl = http://127.0.0.1:11434
model = llama3.2:3b
system_prompt = You are a neutral, helpful assistant that keeps every answer short.
sound_threshold_peak = 0.12
end_silence_ms = 2500
ptt = true
whisper_model_path = ~/.whisper-models/ggml-tiny.bin

[agent]
name = a very long agent name indeed
language = en
tts = kokoro
voice = bf_alice
voice_speed = 1.4
provider = openai
baseurl =
api_key = sk-test
model = gpt-4o-mini
system_prompt = You explain things simply and never use more than 150 words, even when the question is a long one.
sound_threshold_peak = 0.1
end_silence_ms = 2000
ptt = false
whisper_model_path = ~/.whisper-models/ggml-tiny.bin
"#;

fn temp_file(contents: &str) -> std::path::PathBuf {
  let mut path = std::env::temp_dir();
  path.push(format!(
    "vtmate_ui_{}_{}.ini",
    std::process::id(),
    std::time::SystemTime::now()
      .duration_since(std::time::UNIX_EPOCH)
      .unwrap()
      .as_nanos()
  ));
  std::fs::write(&path, contents).unwrap();
  path
}

fn session(contents: &str) -> Arc<AppState> {
  let path = temp_file(contents);
  let state = Arc::new(AppState::default());
  *state.settings_path.lock().unwrap() = path.clone();
  *state.agent_name.lock().unwrap() = "main agent".to_string();
  *state.agents.lock().unwrap() = config::try_load_settings(&path, &config::plain_args()).unwrap();
  let _ = state::GLOBAL_STATE.set(state.clone());
  state
}

fn key(code: KeyCode) -> KeyEvent {
  KeyEvent::new(code, KeyModifiers::NONE)
}

fn press(state: &AppState, code: KeyCode) {
  settings_ui::handle_key(state, &key(code));
}

fn type_text(state: &AppState, text: &str) {
  for c in text.chars() {
    press(state, KeyCode::Char(c));
  }
}

fn ui(state: &AppState) -> SettingsUi {
  state.settings_ui.lock().unwrap().clone()
}

/// The popup as it appears on screen: the escapes are dropped and every
/// cursor move starts a new line, so the rows can be looked at.
fn screen(ui: &SettingsUi) -> String {
  let mut out: Vec<u8> = Vec::new();
  settings_ui::draw(&mut out, ui, &["a conversation line".to_string()]);
  let raw = String::from_utf8_lossy(&out).to_string();
  let mut lines: Vec<String> = vec![String::new()];
  let mut rest = raw.as_str();
  while let Some(at) = rest.find('\u{1b}') {
    lines.last_mut().unwrap().push_str(&rest[..at]);
    let after = &rest[at + 1..];
    let Some((end, final_byte)) = after
      .char_indices()
      .find(|(i, c)| *i > 0 && c.is_ascii_alphabetic())
    else {
      break;
    };
    if final_byte == 'H' {
      lines.push(String::new());
    }
    rest = &after[end + 1..];
  }
  lines.last_mut().unwrap().push_str(rest);
  lines
    .into_iter()
    .filter(|l| l.contains('\u{2502}') || l.contains('\u{250c}') || l.contains('\u{2514}'))
    .collect::<Vec<_>>()
    .join("\n")
}

/// The form cursor is on its "Done" button.
fn on_done(ui: &SettingsUi) -> bool {
  ui.form.cursor == settings_ui::FIELDS.len()
}

/// Tab until "Done" has the focus, then press it.
fn press_done(state: &AppState) {
  for _ in 0..20 {
    if on_done(&ui(state)) {
      break;
    }
    press(state, KeyCode::Tab);
  }
  press(state, KeyCode::Enter);
}

#[test]
fn the_list_shows_every_column_of_every_agent() {
  let state = session(SETTINGS);
  settings_ui::open(&state);
  let view = screen(&ui(&state));
  println!("{}", view);

  for heading in [
    "NAME", "LANG", "MODE", "TTS", "VOICE", "SPD", "PROVIDER", "MODEL", "PROMPT",
  ] {
    assert!(view.contains(heading), "no {} column:\n{}", heading, view);
  }
  // the name is cut to 12 characters, ending in '...'
  assert!(view.contains("a very lo..."), "{}", view);
  assert!(view.contains("PTT"), "{}", view);
  assert!(view.contains("LIVE"), "{}", view);
  assert!(view.contains("super"), "{}", view);
  assert!(view.contains("1.1x"), "{}", view);
  assert!(view.contains("ollama"), "{}", view);
  assert!(view.contains("llam"), "{}", view);
  // the prompt is cut to what is left of the row
  assert!(view.contains("You are"), "{}", view);
  assert!(view.contains("..."), "{}", view);
  assert!(view.contains("n new agent"), "{}", view);
  assert!(view.contains("e edit agent"), "{}", view);
  assert!(view.contains("d delete agent"), "{}", view);
  assert!(view.contains("Save"), "{}", view);
  assert!(view.contains("Cancel"), "{}", view);
  // every row is the same width, so the box is not broken
  let widths: Vec<usize> = view.lines().map(|l| l.chars().count()).collect();
  assert!(
    widths.windows(2).all(|w| w[0] == w[1]),
    "ragged box: {:?}\n{}",
    widths,
    view
  );
}

#[test]
fn the_form_edits_an_agent_and_saves_it_to_the_file() {
  let state = session(SETTINGS);
  settings_ui::open(&state);
  press(&state, KeyCode::Char('e')); // edit the first agent
  assert_eq!(ui(&state).screen, Screen::Form);
  println!("{}", screen(&ui(&state)));

  // the name field is first and the caret sits at its end
  type_text(&state, " two");
  assert_eq!(ui(&state).form.draft.name, "main agent two");

  // TTS, then the language and voice it allows
  press(&state, KeyCode::Down);
  press(&state, KeyCode::Right); // supertonic -> supersonic2 (no voices)
  press(&state, KeyCode::Right); // -> kokoro
  let draft = ui(&state).form.draft.clone();
  assert_eq!(draft.tts, "kokoro");
  assert_eq!(draft.language, "en");
  assert_eq!(draft.voice, "bf_alice", "the voice follows the engine");

  press(&state, KeyCode::Down); // language
  press(&state, KeyCode::Down); // voice
  press(&state, KeyCode::Right);
  assert_eq!(ui(&state).form.draft.voice, "am_puck");

  press(&state, KeyCode::Down); // voice speed
  press(&state, KeyCode::Right);
  assert_eq!(ui(&state).form.draft.voice_speed, 1.2);

  press(&state, KeyCode::Down); // talk mode
  press(&state, KeyCode::Right);
  assert!(!ui(&state).form.draft.ptt);

  println!("{}", screen(&ui(&state)));

  // done, then save
  press_done(&state);
  assert_eq!(
    ui(&state).screen,
    Screen::List,
    "form went back to the list"
  );

  settings_ui::handle_key(
    &state,
    &KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL),
  );
  assert!(!ui(&state).open, "the popup closed on save");

  let path = state.settings_path.lock().unwrap().clone();
  let saved = config::try_load_settings(&path, &config::plain_args()).unwrap();
  assert_eq!(saved[0].name, "main agent two");
  assert_eq!(saved[0].tts, "kokoro");
  assert_eq!(saved[0].voice, "am_puck");
  assert_eq!(saved[0].voice_speed, 1.2);
  assert!(!saved[0].ptt);
  // and the running session is on it
  assert_eq!(*state.agent_name.lock().unwrap(), "main agent two");
  assert_eq!(state.agents.lock().unwrap()[0].name, "main agent two");
  assert_eq!(
    config::load_general_settings(&path).unwrap().selected_agent,
    "main agent two"
  );
}

#[test]
fn a_new_agent_is_added_and_a_deleted_one_is_confirmed_first() {
  let state = session(SETTINGS);
  settings_ui::open(&state);

  press(&state, KeyCode::Char('n'));
  assert_eq!(ui(&state).screen, Screen::Form);
  assert_eq!(ui(&state).form.draft.name, "new agent");
  press(&state, KeyCode::Enter); // leave the name as it is, next field
  press_done(&state);
  assert_eq!(ui(&state).agents.len(), 3);

  // delete asks first
  press(&state, KeyCode::Up);
  press(&state, KeyCode::Char('d'));
  assert_eq!(ui(&state).screen, Screen::ConfirmDelete);
  let view = screen(&ui(&state));
  println!("{}", view);
  assert!(view.contains("Delete the agent"), "{}", view);
  press(&state, KeyCode::Esc);
  assert_eq!(ui(&state).agents.len(), 3, "escape kept it");

  press(&state, KeyCode::Char('d'));
  press(&state, KeyCode::Enter);
  assert_eq!(ui(&state).agents.len(), 2);

  settings_ui::handle_key(
    &state,
    &KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL),
  );
  let path = state.settings_path.lock().unwrap().clone();
  let names: Vec<String> = config::try_load_settings(&path, &config::plain_args())
    .unwrap()
    .into_iter()
    .map(|a| a.name)
    .collect();
  assert_eq!(names.len(), 2, "{:?}", names);
  assert!(names.contains(&"new agent".to_string()), "{:?}", names);
}

#[test]
fn leaving_with_changes_asks_before_throwing_them_away() {
  let state = session(SETTINGS);
  settings_ui::open(&state);

  // nothing changed: escape just closes
  press(&state, KeyCode::Esc);
  assert!(!ui(&state).open);

  settings_ui::open(&state);
  press(&state, KeyCode::Char('e'));
  type_text(&state, "!");
  // the form asks too
  press(&state, KeyCode::Esc);
  assert_eq!(ui(&state).screen, Screen::ConfirmDiscardForm);
  press(&state, KeyCode::Char('y'));
  assert_eq!(ui(&state).screen, Screen::List);
  assert!(ui(&state).open);

  // a change that reached the list makes the popup ask on the way out
  press(&state, KeyCode::Char('e'));
  type_text(&state, "!");
  press_done(&state);
  press(&state, KeyCode::Esc);
  assert_eq!(ui(&state).screen, Screen::ConfirmDiscard);
  let view = screen(&ui(&state));
  println!("{}", view);
  assert!(view.contains("Throw away the changes?"), "{}", view);
  press(&state, KeyCode::Char('n'));
  assert_eq!(ui(&state).screen, Screen::List, "kept editing");
  press(&state, KeyCode::Esc);
  press(&state, KeyCode::Enter); // yes
  assert!(!ui(&state).open);

  // nothing was written
  let path = state.settings_path.lock().unwrap().clone();
  assert_eq!(
    config::try_load_settings(&path, &config::plain_args()).unwrap()[0].name,
    "main agent"
  );
}

#[test]
fn a_prompt_over_five_lines_is_saved_as_a_block() {
  let state = session(SETTINGS);
  settings_ui::open(&state);
  press(&state, KeyCode::Char('e'));
  // walk down to the prompt, the last field
  for _ in 0..13 {
    press(&state, KeyCode::Down);
  }
  press(&state, KeyCode::End);
  for _ in 0..6 {
    press(&state, KeyCode::Enter); // a new line inside the prompt
    type_text(&state, "another line");
  }
  let view = screen(&ui(&state));
  println!("{}", view);
  assert!(view.contains("[system_prompt] block"), "{}", view);

  press_done(&state);
  settings_ui::handle_key(
    &state,
    &KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL),
  );
  let path = state.settings_path.lock().unwrap().clone();
  let written = std::fs::read_to_string(&path).unwrap();
  assert!(written.contains("[system_prompt]"), "{}", written);
  let saved = config::try_load_settings(&path, &config::plain_args()).unwrap();
  assert_eq!(saved[0].system_prompt.lines().count(), 7);
}

#[test]
fn a_refused_agent_keeps_the_popup_open_and_the_file_untouched() {
  let state = session(SETTINGS);
  settings_ui::open(&state);
  press(&state, KeyCode::Char('e'));
  // empty the name
  for _ in 0..40 {
    press(&state, KeyCode::Backspace);
  }
  press_done(&state);
  let after = ui(&state);
  assert_eq!(after.screen, Screen::Form, "the form stayed open");
  assert!(!after.form.errors.is_empty(), "it says what is wrong");
  let view = screen(&after);
  println!("{}", view);
  assert!(view.contains("⚠"), "{}", view);
}

#[test]
fn the_popup_fits_a_small_terminal() {
  // rendering falls back to 80x24 with no terminal attached
  let state = session(SETTINGS);
  settings_ui::open(&state);
  let view = screen(&ui(&state));
  for line in view.lines() {
    assert!(line.chars().count() <= 80, "too wide: {:?}", line);
  }
  assert!(
    view.contains("PROMPT"),
    "the prompt column survives 80 columns:\n{}",
    view
  );
}

/// Build a settings file with `n` agents, so the list has to scroll.
fn many_agents(n: usize) -> String {
  let mut out = String::new();
  for i in 0..n {
    out.push_str(&format!(
      "[agent]\nname = agent {:02}\nlanguage = en\ntts = supertonic\nvoice = M1\n\
       voice_speed = 1.2\nptt = true\nprovider = ollama\nbaseurl = http://127.0.0.1:11434\n\
       model = llama3.2:3b\nsystem_prompt = you are agent {:02}\nsound_threshold_peak = 0.05\n\
       end_silence_ms = 700\nwhisper_model_path = ~/.whisper-models/ggml-tiny.bin\n\n",
      i, i
    ));
  }
  out
}

#[test]
fn tab_from_the_list_goes_straight_to_save() {
  let state = session(&many_agents(12));
  settings_ui::open(&state);

  // from the first agent, with eleven more below it
  assert_eq!(ui(&state).cursor, 0);
  press(&state, KeyCode::Tab);
  assert_eq!(ui(&state).cursor, 12, "Tab from a row lands on Save");
  assert!(screen(&ui(&state)).contains("Save"));

  // and from further down the list, not one row at a time
  press(&state, KeyCode::Tab); // Save -> Cancel
  assert_eq!(ui(&state).cursor, 13);
  press(&state, KeyCode::Tab); // Cancel -> back to the list
  assert_eq!(ui(&state).cursor, 0);
  press(&state, KeyCode::Down);
  press(&state, KeyCode::Down);
  assert_eq!(ui(&state).cursor, 2);
  press(&state, KeyCode::Tab);
  assert_eq!(ui(&state).cursor, 12, "Tab from any row lands on Save");
}

#[test]
fn a_long_list_scrolls_and_stays_inside_the_popup() {
  let state = session(&many_agents(40));
  settings_ui::open(&state);

  let shown = |state: &AppState| -> Vec<String> {
    screen(&ui(state))
      .lines()
      .filter(|l| l.contains("agent "))
      .map(|l| l.to_string())
      .collect()
  };

  // the popup never grows past the terminal it is drawn in
  let drawn = screen(&ui(&state));
  assert!(
    drawn.lines().count() <= 24,
    "popup is {} lines tall",
    drawn.lines().count()
  );
  // only a window of the agents is drawn, and it says so
  let first_view = shown(&state);
  assert!(
    first_view.len() < 40,
    "all 40 agents were drawn at once: {}",
    first_view.len()
  );
  assert!(
    drawn.contains(&format!("of {} - ", 40)),
    "no scroll position shown:\n{}",
    drawn
  );
  assert!(first_view.iter().any(|l| l.contains("agent 00")));

  // walking down past the window scrolls it, keeping the cursor in view
  for _ in 0..39 {
    press(&state, KeyCode::Down);
  }
  assert_eq!(ui(&state).cursor, 39);
  let last_view = shown(&state);
  assert!(
    last_view.iter().any(|l| l.contains("agent 39")),
    "the last agent is not on screen:\n{}",
    screen(&ui(&state))
  );
  assert!(
    !last_view.iter().any(|l| l.contains("agent 00")),
    "the list did not scroll"
  );
  assert_eq!(
    last_view.len(),
    first_view.len(),
    "the window changed size while scrolling"
  );
}
