// ------------------------------------------------------------------
//  Compose popup (Enter)
//
//  A typed message with attached .txt / .pdf files. The model and the key
//  handling live here; `ui::render_compose_modal` draws it the way the other
//  popups are drawn, from `AppState::compose_ui` (an attached terminal draws
//  its own mirrored copy, see `StateView`).
// ------------------------------------------------------------------

use crate::state::{AppState, GLOBAL_STATE};
use crate::text_field::{insert_char_at, move_caret_vertical, remove_char_at};
use crate::ui_file_browser::{BrowserEvent, FileBrowser};
use crossbeam_channel::Sender;
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

// API
// ------------------------------------------------------------------

/// Longest text one attached file may contribute to a message.
pub const MAX_FILE_CHARS: usize = 1_000_000;

/// Largest file read from disk at all; a PDF this size is parsed in a child
/// process, and a text file is read whole into memory.
const MAX_FILE_BYTES: u64 = 50 * 1024 * 1024;

/// How long the child process may spend turning one PDF into text.
const PDF_TIMEOUT: Duration = Duration::from_secs(60);

/// Where the keyboard is inside the popup, in Tab order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Focus {
  #[default]
  Text,
  Files,
  Send,
  Attach,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct AttachedFile {
  pub name: String,
  /// Length of `content` in characters, for the list.
  pub chars: usize,
  /// The extracted text. Not mirrored: an attached terminal only lists names.
  #[serde(skip)]
  pub content: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ComposeUi {
  pub open: bool,
  /// Bumped on every open, so a file that finishes loading after the popup
  /// was closed cannot land in the next one.
  pub session: u64,
  pub text: String,
  /// Caret inside `text`, in chars.
  pub caret: usize,
  pub focus: Focus,
  pub files: Vec<AttachedFile>,
  /// Selected row of the file list.
  pub file_cursor: usize,
  /// The file browser while it is open, on top of the popup.
  pub browser: Option<crate::ui_file_browser::FileBrowser>,
  /// Files still being read.
  pub loading: usize,
  /// A line at the bottom of the popup (what went wrong).
  pub notice: Option<String>,
}

/// What the popup did with a key.
pub enum Outcome {
  /// Handled; these messages go to the UI thread.
  Handled(Vec<String>),
  /// The user sent the message: its full text, then the UI messages.
  Submit(String, Vec<String>),
  /// Not the popup's key (Space, for push-to-talk).
  PassThrough,
}

pub fn is_open(state: &AppState) -> bool {
  state.compose_ui.lock().unwrap().open
}

/// Space types a character (and so is not push-to-talk) in the message field.
pub fn space_is_text(state: &AppState) -> bool {
  let ui = state.compose_ui.lock().unwrap();
  ui.open && ui.focus == Focus::Text && ui.browser.is_none()
}

pub fn open(state: &AppState) -> Vec<String> {
  let mut ui = state.compose_ui.lock().unwrap();
  let session = ui.session + 1;
  *ui = ComposeUi {
    open: true,
    session,
    ..Default::default()
  };
  vec!["compose_show|".to_string()]
}

/// Speech transcribed while the popup is open goes into the message field, at
/// the caret. False when the popup is closed and the transcript is a normal
/// turn.
pub fn append_transcript(state: &AppState, transcript: &str) -> bool {
  let mut ui = state.compose_ui.lock().unwrap();
  if !ui.open {
    return false;
  }
  let chars: Vec<char> = ui.text.chars().collect();
  let caret = ui.caret.min(chars.len());
  let needs_space = caret > 0 && !chars[caret - 1].is_whitespace();
  let mut insert = String::new();
  if needs_space {
    insert.push(' ');
  }
  insert.push_str(transcript.trim());
  let added = insert.chars().count();
  let byte = ui
    .text
    .char_indices()
    .nth(caret)
    .map_or(ui.text.len(), |(i, _)| i);
  ui.text.insert_str(byte, &insert);
  ui.caret = caret + added;
  true
}

/// The text of the message as the model receives it: what was typed, then each
/// file in its own block.
pub fn compose_message(text: &str, files: &[AttachedFile]) -> String {
  let mut parts: Vec<String> = Vec::new();
  if !text.trim().is_empty() {
    parts.push(text.trim().to_string());
  }
  for f in files {
    parts.push(format!(
      "=== file: {} ===\n{}\n===",
      f.name,
      f.content.trim_end()
    ));
  }
  parts.join("\n\n")
}

pub fn handle_key(state: &AppState, k: &KeyEvent, tx_ui: &Sender<String>) -> Outcome {
  let mut ui = state.compose_ui.lock().unwrap();
  let browsing = ui.browser.is_some();

  if k.code == KeyCode::Char(' ') {
    if browsing || ui.focus != Focus::Text || k.kind == KeyEventKind::Release {
      return Outcome::PassThrough;
    }
  }
  // A terminal reporting the Kitty keyboard protocol sends a Release event
  // too; without this, every key here would also fire again on release.
  if k.kind != KeyEventKind::Press {
    return Outcome::Handled(Vec::new());
  }

  let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
  let update = || vec!["compose_update|".to_string()];

  if browsing {
    return Outcome::Handled(browser_key(&mut ui, k, tx_ui));
  }

  // Ctrl+Enter (or Alt+Enter, which terminals report without any keyboard
  // protocol) sends from anywhere. Plain Enter is a new line in the message.
  let modified_enter =
    k.code == KeyCode::Enter && (ctrl || k.modifiers.contains(KeyModifiers::ALT));
  if modified_enter {
    return submit(&mut ui);
  }
  if k.code == KeyCode::Esc {
    ui.open = false;
    return Outcome::Handled(vec!["compose_hide|".to_string()]);
  }
  if ctrl {
    // Ctrl+J is the line feed a terminal sends for a new line: a pasted line
    // break and, on some terminals, the Enter key itself.
    if ui.focus == Focus::Text && matches!(k.code, KeyCode::Char('j') | KeyCode::Char('J')) {
      let caret = ui.caret.min(ui.text.chars().count());
      insert_char_at(&mut ui.text, caret, '\n');
      ui.caret = caret + 1;
      return Outcome::Handled(update());
    }
    return Outcome::Handled(Vec::new());
  }

  match k.code {
    KeyCode::Tab => {
      ui.focus = next_focus(&ui, false);
      return Outcome::Handled(update());
    }
    KeyCode::BackTab => {
      ui.focus = next_focus(&ui, true);
      return Outcome::Handled(update());
    }
    _ => {}
  }

  match ui.focus {
    Focus::Text => text_key(&mut ui, k),
    Focus::Files => files_key(&mut ui, k),
    Focus::Send => match k.code {
      KeyCode::Enter => return submit(&mut ui),
      KeyCode::Right => {
        ui.focus = Focus::Attach;
        return Outcome::Handled(update());
      }
      KeyCode::Up => {
        ui.focus = previous_content_focus(&ui);
        return Outcome::Handled(update());
      }
      _ => return Outcome::Handled(Vec::new()),
    },
    Focus::Attach => match k.code {
      KeyCode::Enter => {
        ui.notice = None;
        let start = crate::util::get_user_home_path()
          .or_else(|| std::env::current_dir().ok())
          .unwrap_or_else(|| PathBuf::from("/"));
        match FileBrowser::open(start, &["txt", "pdf"]) {
          Ok(browser) => ui.browser = Some(browser),
          Err(e) => ui.notice = Some(format!("Cannot list the files: {}", e)),
        }
        return Outcome::Handled(update());
      }
      KeyCode::Left => {
        ui.focus = Focus::Send;
        return Outcome::Handled(update());
      }
      KeyCode::Up => {
        ui.focus = previous_content_focus(&ui);
        return Outcome::Handled(update());
      }
      _ => return Outcome::Handled(Vec::new()),
    },
  }
  Outcome::Handled(update())
}

// PRIVATE
// ------------------------------------------------------------------

fn submit(ui: &mut ComposeUi) -> Outcome {
  if ui.loading > 0 {
    ui.notice = Some("Still reading a file, try again in a moment".to_string());
    return Outcome::Handled(vec!["compose_update|".to_string()]);
  }
  let message = compose_message(&ui.text, &ui.files);
  if message.is_empty() {
    ui.notice = Some("Nothing to send: write a message or add a file".to_string());
    return Outcome::Handled(vec!["compose_update|".to_string()]);
  }
  ui.open = false;
  Outcome::Submit(message, vec!["compose_hide|".to_string()])
}

/// The list of focus stops: the file list only takes part while it has files.
fn focus_order(ui: &ComposeUi) -> Vec<Focus> {
  let mut order = vec![Focus::Text];
  if !ui.files.is_empty() {
    order.push(Focus::Files);
  }
  order.push(Focus::Send);
  order.push(Focus::Attach);
  order
}

fn next_focus(ui: &ComposeUi, backward: bool) -> Focus {
  let order = focus_order(ui);
  let at = order.iter().position(|f| *f == ui.focus).unwrap_or(0);
  let to = if backward {
    (at + order.len() - 1) % order.len()
  } else {
    (at + 1) % order.len()
  };
  order[to]
}

/// The file list if there is one, else the message field: where ↑ goes from a
/// button.
fn previous_content_focus(ui: &ComposeUi) -> Focus {
  if ui.files.is_empty() {
    Focus::Text
  } else {
    Focus::Files
  }
}

fn text_key(ui: &mut ComposeUi, k: &KeyEvent) {
  let len = ui.text.chars().count();
  ui.caret = ui.caret.min(len);
  match k.code {
    KeyCode::Char(c) => {
      let caret = ui.caret;
      insert_char_at(&mut ui.text, caret, c);
      ui.caret += 1;
    }
    KeyCode::Enter => {
      let caret = ui.caret;
      insert_char_at(&mut ui.text, caret, '\n');
      ui.caret += 1;
    }
    KeyCode::Backspace => {
      if ui.caret > 0 {
        let idx = ui.caret - 1;
        remove_char_at(&mut ui.text, idx);
        ui.caret = idx;
      }
    }
    KeyCode::Delete => {
      let caret = ui.caret;
      if caret < len {
        remove_char_at(&mut ui.text, caret);
      }
    }
    KeyCode::Left => ui.caret = ui.caret.saturating_sub(1),
    KeyCode::Right => ui.caret = (ui.caret + 1).min(len),
    KeyCode::Home => {
      let (start, _) = line_bounds(&ui.text, ui.caret);
      ui.caret = start;
    }
    KeyCode::End => {
      let (_, end) = line_bounds(&ui.text, ui.caret);
      ui.caret = end;
    }
    KeyCode::Up => {
      if let Some(to) = move_caret_vertical(&ui.text, ui.caret, false) {
        ui.caret = to;
      }
    }
    KeyCode::Down => match move_caret_vertical(&ui.text, ui.caret, true) {
      Some(to) => ui.caret = to,
      None => ui.focus = next_focus(ui, false),
    },
    _ => {}
  }
}

fn files_key(ui: &mut ComposeUi, k: &KeyEvent) {
  let last = ui.files.len().saturating_sub(1);
  ui.file_cursor = ui.file_cursor.min(last);
  match k.code {
    KeyCode::Up => {
      if ui.file_cursor == 0 {
        ui.focus = Focus::Text;
      } else {
        ui.file_cursor -= 1;
      }
    }
    KeyCode::Down => {
      if ui.file_cursor >= last {
        ui.focus = Focus::Send;
      } else {
        ui.file_cursor += 1;
      }
    }
    KeyCode::Delete | KeyCode::Backspace => {
      if !ui.files.is_empty() {
        let at = ui.file_cursor;
        ui.files.remove(at);
        ui.file_cursor = at.min(ui.files.len().saturating_sub(1));
        if ui.files.is_empty() {
          ui.focus = Focus::Send;
        }
      }
    }
    _ => {}
  }
}

/// Keys while the file browser is open: it takes all of them.
fn browser_key(ui: &mut ComposeUi, k: &KeyEvent, tx_ui: &Sender<String>) -> Vec<String> {
  let Some(browser) = ui.browser.as_mut() else {
    return Vec::new();
  };
  match browser.handle_key(k) {
    BrowserEvent::None => {}
    BrowserEvent::Closed => ui.browser = None,
    BrowserEvent::Picked(path) => {
      ui.browser = None;
      ui.notice = None;
      start_loading(ui, path, tx_ui.clone());
    }
  }
  vec!["compose_update|".to_string()]
}

/// Read `path` on a worker thread and add it to the list when done; converting
/// a large PDF takes a while and the keyboard must stay responsive meanwhile.
fn start_loading(ui: &mut ComposeUi, path: PathBuf, tx_ui: Sender<String>) {
  ui.loading += 1;
  let session = ui.session;
  std::thread::spawn(move || {
    let result = load_file(&path);
    if let Some(state) = GLOBAL_STATE.get() {
      let mut ui = state.compose_ui.lock().unwrap();
      if ui.open && ui.session == session {
        ui.loading = ui.loading.saturating_sub(1);
        match result {
          Ok(file) => {
            ui.files.push(file);
            ui.notice = None;
          }
          Err(e) => ui.notice = Some(e),
        }
      }
    }
    let _ = tx_ui.send("compose_update|".to_string());
  });
}

fn load_file(path: &Path) -> Result<AttachedFile, String> {
  let name = path
    .file_name()
    .map(|n| n.to_string_lossy().into_owned())
    .unwrap_or_else(|| path.display().to_string());
  let extension = path
    .extension()
    .map(|e| e.to_string_lossy().to_lowercase())
    .unwrap_or_default();
  if extension != "txt" && extension != "pdf" {
    return Err(format!("{}: only .txt and .pdf files can be attached", name));
  }
  let meta = std::fs::metadata(path).map_err(|e| format!("{}: {}", name, e))?;
  if !meta.is_file() {
    return Err(format!("{}: not a file", name));
  }
  if meta.len() > MAX_FILE_BYTES {
    return Err(format!("{}: larger than {} MB", name, MAX_FILE_BYTES / 1024 / 1024));
  }
  let content = if extension == "pdf" {
    pdf_text(path).map_err(|e| format!("{}: {}", name, e))?
  } else {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {}", name, e))?;
    let (text, _, _) = encoding_rs::UTF_8.decode(&bytes);
    text.into_owned()
  };
  let content = tidy(&content);
  if content.is_empty() {
    return Err(format!(
      "{}: no text found{}",
      name,
      if extension == "pdf" {
        " (a scanned PDF has no text layer)"
      } else {
        ""
      }
    ));
  }
  let chars = content.chars().count();
  if chars > MAX_FILE_CHARS {
    return Err(format!(
      "{}: too long ({} characters, the limit is {})",
      name, chars, MAX_FILE_CHARS
    ));
  }
  Ok(AttachedFile {
    name,
    chars,
    content,
  })
}

/// Trailing spaces off every line and no more than one blank line in a row:
/// PDF extraction leaves long runs of both.
fn tidy(text: &str) -> String {
  let mut out = String::with_capacity(text.len());
  let mut blank_run = 0;
  for line in text.lines() {
    let line = line.trim_end();
    if line.is_empty() {
      blank_run += 1;
      if blank_run > 1 {
        continue;
      }
    } else {
      blank_run = 0;
    }
    out.push_str(line);
    out.push('\n');
  }
  out.trim().to_string()
}

/// The text of a PDF, extracted by this same binary run as a child process
/// (`--extract-pdf-text`). The parser panics on some malformed files and
/// release builds abort on panic, which would take the whole conversation down
/// with them; in a child that costs one attachment.
fn pdf_text(path: &Path) -> Result<String, String> {
  let exe = std::env::current_exe().map_err(|e| e.to_string())?;
  let mut child = Command::new(exe)
    .arg("--extract-pdf-text")
    .arg(path)
    .stdin(Stdio::null())
    .stdout(Stdio::piped())
    .stderr(Stdio::null())
    .spawn()
    .map_err(|e| e.to_string())?;
  let mut stdout = child.stdout.take().ok_or("no output from the PDF reader")?;
  // Drained on its own thread: a child that fills the pipe would otherwise
  // block on write while this one waits for it to exit.
  let reader = std::thread::spawn(move || {
    let mut buf = Vec::new();
    let _ = stdout.read_to_end(&mut buf);
    buf
  });
  let deadline = Instant::now() + PDF_TIMEOUT;
  let status = loop {
    match child.try_wait() {
      Ok(Some(status)) => break status,
      Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
      _ => {
        let _ = child.kill();
        let _ = child.wait();
        return Err("reading the PDF took too long".to_string());
      }
    }
  };
  let bytes = reader.join().unwrap_or_default();
  if !status.success() {
    return Err("could not read the PDF (damaged or protected?)".to_string());
  }
  String::from_utf8(bytes).map_err(|_| "the PDF text is not valid UTF-8".to_string())
}

/// Entry point of `--extract-pdf-text`: the PDF's text on stdout, exit code 0
/// on success.
pub fn print_pdf_text(path: &Path) -> ! {
  match pdf_extract::extract_text(path) {
    Ok(text) => {
      use std::io::Write;
      let mut out = std::io::stdout().lock();
      let _ = out.write_all(text.as_bytes());
      let _ = out.flush();
      std::process::exit(0);
    }
    Err(_) => std::process::exit(1),
  }
}

/// Start and end (chars) of the line the caret is on.
fn line_bounds(text: &str, caret: usize) -> (usize, usize) {
  let chars: Vec<char> = text.chars().collect();
  let caret = caret.min(chars.len());
  let start = chars[..caret]
    .iter()
    .rposition(|c| *c == '\n')
    .map_or(0, |i| i + 1);
  let end = chars[caret..]
    .iter()
    .position(|c| *c == '\n')
    .map_or(chars.len(), |i| caret + i);
  (start, end)
}

#[cfg(test)]
mod tests {
  use super::*;

  fn file(name: &str, content: &str) -> AttachedFile {
    AttachedFile {
      name: name.to_string(),
      chars: content.chars().count(),
      content: content.to_string(),
    }
  }

  #[test]
  fn message_wraps_each_file_in_its_own_block() {
    let msg = compose_message("hello", &[file("a.txt", "one\n"), file("b.pdf", "two")]);
    assert_eq!(
      msg,
      "hello\n\n=== file: a.txt ===\none\n===\n\n=== file: b.pdf ===\ntwo\n==="
    );
  }

  #[test]
  fn message_without_typed_text_is_only_the_files() {
    let msg = compose_message("  \n", &[file("a.txt", "one")]);
    assert_eq!(msg, "=== file: a.txt ===\none\n===");
    assert_eq!(compose_message("", &[]), "");
  }

  #[test]
  fn tidy_collapses_blank_runs_and_trailing_spaces() {
    assert_eq!(tidy("a  \n\n\n\nb\n\n"), "a\n\nb");
  }

  #[test]
  fn line_bounds_finds_the_current_line() {
    assert_eq!(line_bounds("ab\ncde\nf", 4), (3, 6));
    assert_eq!(line_bounds("ab", 2), (0, 2));
  }
}
