// ------------------------------------------------------------------
//  File browser popup
//
//  A directory listing with browser-style back/forward history, drawn on top
//  of whatever popup opened it. The compose popup uses it to pick the files
//  it attaches. The model, its key handling and its drawing live here; the
//  model is serializable so a terminal attached to the daemon draws the
//  browser the daemon is driving (see `compose::ComposeUi`).
// ------------------------------------------------------------------

use crossterm::{
  cursor::MoveTo,
  event::{KeyCode, KeyEvent},
  execute,
  style::Print,
  terminal,
};
use serde::{Deserialize, Serialize};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use unicode_width::UnicodeWidthChar;

// API
// ------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirEntryInfo {
  pub name: String,
  pub is_dir: bool,
}

/// What a key did to the browser.
#[derive(Debug, PartialEq, Eq)]
pub enum BrowserEvent {
  None,
  /// A file was chosen.
  Picked(PathBuf),
  /// The browser was dismissed.
  Closed,
}

/// A directory listing with back/forward history (distinct from the `..`
/// entry, which moves up one level like any other navigation and is itself
/// recorded on the back stack).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileBrowser {
  pub current_dir: PathBuf,
  pub entries: Vec<DirEntryInfo>,
  pub selected: usize,
  /// What went wrong with the last key (a directory that cannot be read).
  pub error: Option<String>,
  /// Lowercase extensions of the files listed; directories always are.
  #[serde(skip)]
  extensions: Vec<String>,
  #[serde(skip)]
  history_back: Vec<PathBuf>,
  #[serde(skip)]
  history_forward: Vec<PathBuf>,
}

impl FileBrowser {
  /// Open on `dir`, listing only files whose extension is one of
  /// `extensions` (given without the dot).
  pub fn open(dir: PathBuf, extensions: &[&str]) -> io::Result<Self> {
    let extensions: Vec<String> = extensions.iter().map(|e| e.to_lowercase()).collect();
    // Paths cross to an attached terminal as JSON, which has no way to carry
    // one that is not UTF-8.
    if dir.to_str().is_none() {
      return Err(io::Error::other("the folder's path is not valid UTF-8"));
    }
    let entries = read_entries(&dir, &extensions)?;
    Ok(Self {
      current_dir: dir,
      entries,
      selected: 0,
      error: None,
      extensions,
      history_back: Vec::new(),
      history_forward: Vec::new(),
    })
  }

  pub fn selected_entry(&self) -> Option<&DirEntryInfo> {
    self.entries.get(self.selected)
  }

  /// The path of the selected row; `..` is the parent folder.
  pub fn selected_path(&self) -> Option<PathBuf> {
    let entry = self.selected_entry()?;
    if entry.name == ".." {
      Some(
        self
          .current_dir
          .parent()
          .map(Path::to_path_buf)
          .unwrap_or_else(|| self.current_dir.clone()),
      )
    } else {
      Some(self.current_dir.join(&entry.name))
    }
  }

  pub fn select_next(&mut self) {
    if !self.entries.is_empty() {
      self.selected = (self.selected + 1) % self.entries.len();
    }
  }

  pub fn select_prev(&mut self) {
    if !self.entries.is_empty() {
      self.selected = (self.selected + self.entries.len() - 1) % self.entries.len();
    }
  }

  /// Enter the selected folder, pushing this one onto the back history and
  /// dropping the forward history (a new navigation makes it stale). A folder
  /// that cannot be read leaves the browser where it was.
  pub fn navigate_into_selected(&mut self) {
    let Some(target) = self.selected_path() else {
      return;
    };
    let entries = match read_entries(&target, &self.extensions) {
      Ok(entries) => entries,
      Err(e) => {
        self.error = Some(format!("{}: {}", target.display(), e));
        return;
      }
    };
    self.history_back.push(std::mem::replace(&mut self.current_dir, target));
    self.history_forward.clear();
    self.entries = entries;
    self.selected = 0;
  }

  pub fn go_back(&mut self) {
    let Some(previous) = self.history_back.pop() else {
      return;
    };
    match read_entries(&previous, &self.extensions) {
      Ok(entries) => {
        self.history_forward.push(std::mem::replace(&mut self.current_dir, previous));
        self.entries = entries;
        self.selected = 0;
      }
      Err(e) => {
        self.error = Some(format!("{}: {}", previous.display(), e));
        self.history_back.push(previous);
      }
    }
  }

  pub fn go_forward(&mut self) {
    let Some(next) = self.history_forward.pop() else {
      return;
    };
    match read_entries(&next, &self.extensions) {
      Ok(entries) => {
        self.history_back.push(std::mem::replace(&mut self.current_dir, next));
        self.entries = entries;
        self.selected = 0;
      }
      Err(e) => {
        self.error = Some(format!("{}: {}", next.display(), e));
        self.history_forward.push(next);
      }
    }
  }

  /// ↑/↓ move the selection, ←/→ walk the history, Enter opens a folder or
  /// picks a file, Backspace goes up one folder, Esc closes.
  pub fn handle_key(&mut self, k: &KeyEvent) -> BrowserEvent {
    self.error = None;
    match k.code {
      KeyCode::Up => self.select_prev(),
      KeyCode::Down => self.select_next(),
      KeyCode::PageUp => self.selected = self.selected.saturating_sub(10),
      KeyCode::PageDown => {
        self.selected = (self.selected + 10).min(self.entries.len().saturating_sub(1));
      }
      KeyCode::Home => self.selected = 0,
      KeyCode::End => self.selected = self.entries.len().saturating_sub(1),
      KeyCode::Left => self.go_back(),
      KeyCode::Right => self.go_forward(),
      KeyCode::Backspace => {
        if let Some(at) = self.entries.iter().position(|e| e.name == "..") {
          self.selected = at;
          self.navigate_into_selected();
        }
      }
      KeyCode::Esc => return BrowserEvent::Closed,
      KeyCode::Enter => {
        let Some(entry) = self.selected_entry() else {
          return BrowserEvent::None;
        };
        if entry.is_dir {
          self.navigate_into_selected();
        } else if let Some(path) = self.selected_path() {
          return BrowserEvent::Picked(path);
        }
      }
      _ => {}
    }
    BrowserEvent::None
  }
}

/// Draw the browser as a bordered box in the middle of the screen, over
/// whatever is already drawn.
pub fn render<W: Write>(out: &mut W, browser: &FileBrowser, title_prefix: &str) {
  let (cols, rows) = terminal::size().unwrap_or((80, 24));
  let width = ((cols as usize * 7 / 10).max(40)).min(cols.saturating_sub(2) as usize).max(10) as u16;
  let height = rows.saturating_sub(2).min(22).max(6);
  let x = cols.saturating_sub(width) / 2;
  let y = rows.saturating_sub(height) / 2;
  let inner = width as usize - 2;

  for row in y..y + height {
    execute!(
      out,
      MoveTo(x, row),
      Print(format!("\x1b[48;5;236m{}\x1b[0m", " ".repeat(width as usize)))
    )
    .unwrap();
  }

  let title = tail_fit(
    &format!(" {} - {} ", title_prefix, browser.current_dir.display()),
    inner,
  );
  let title_pad = inner.saturating_sub(display_width(&title));
  execute!(
    out,
    MoveTo(x, y),
    Print(format!(
      "\x1b[48;5;236m\x1b[97m┌\x1b[1m{}\x1b[22m{}┐\x1b[0m",
      title,
      "─".repeat(title_pad)
    ))
  )
  .unwrap();
  execute!(
    out,
    MoveTo(x, y + height - 1),
    Print(format!(
      "\x1b[48;5;236m\x1b[97m└{}┘\x1b[0m",
      "─".repeat(inner)
    ))
  )
  .unwrap();
  for row in (y + 1)..(y + height - 1) {
    for edge in [x, x + width - 1] {
      execute!(
        out,
        MoveTo(edge, row),
        Print("\x1b[48;5;236m\x1b[97m│\x1b[0m")
      )
      .unwrap();
    }
  }

  // The list keeps the selected row in the middle of its rows, except near
  // either end of the folder.
  let list_rows = (height as usize).saturating_sub(4).max(1);
  let total = browser.entries.len();
  let selected = browser.selected.min(total.saturating_sub(1));
  let top = selected
    .saturating_sub(list_rows / 2)
    .min(total.saturating_sub(list_rows));
  for row in 0..list_rows {
    let line = match browser.entries.get(top + row) {
      Some(entry) => {
        let name = clean(&entry.name);
        let text = fit(
          &format!(" {}{}", name, if entry.is_dir { "/" } else { "" }),
          inner,
        );
        let style = if top + row == selected {
          "\x1b[47;30m"
        } else if entry.is_dir {
          "\x1b[97;1m"
        } else {
          "\x1b[97m"
        };
        format!("{}{}", style, text)
      }
      None if row == 0 && total == 0 => {
        format!("\x1b[90m{}", fit(" (no .txt or .pdf files here)", inner))
      }
      None => " ".repeat(inner),
    };
    execute!(
      out,
      MoveTo(x + 1, y + 1 + row as u16),
      Print(format!("\x1b[48;5;236m{}\x1b[0m", line))
    )
    .unwrap();
  }

  let footer = match &browser.error {
    Some(e) => format!("\x1b[91m▲ {}", fit(&clean(e), inner.saturating_sub(2))),
    None => "\x1b[97m ↑/↓ \x1b[90mSelect  \x1b[97m←/→ \x1b[90mBack/Forward  \x1b[97mEnter \x1b[90mOpen/Add  \x1b[97mEsc \x1b[90mCancel"
      .to_string(),
  };
  execute!(
    out,
    MoveTo(x + 1, y + height - 2),
    Print(format!("\x1b[48;5;236m{}\x1b[0m", footer))
  )
  .unwrap();
  out.flush().unwrap();
}

// PRIVATE
// ------------------------------------------------------------------

/// Folders first, then files, each alphabetical without regard to case, with
/// `..` on top unless this is the root. Files that are not one of
/// `extensions` are left out.
fn read_entries(dir: &Path, extensions: &[String]) -> io::Result<Vec<DirEntryInfo>> {
  let mut dirs = Vec::new();
  let mut files = Vec::new();
  for entry in std::fs::read_dir(dir)?.flatten() {
    let name = entry.file_name().to_string_lossy().into_owned();
    let Ok(file_type) = entry.file_type() else {
      continue;
    };
    // A link is followed: a link to a folder is one.
    let is_dir = file_type.is_dir() || (file_type.is_symlink() && entry.path().is_dir());
    if is_dir {
      dirs.push(DirEntryInfo { name, is_dir: true });
    } else {
      let extension = Path::new(&name)
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
      if extensions.contains(&extension) {
        files.push(DirEntryInfo {
          name,
          is_dir: false,
        });
      }
    }
  }
  dirs.sort_by_key(|e| e.name.to_lowercase());
  files.sort_by_key(|e| e.name.to_lowercase());

  let mut entries = Vec::with_capacity(dirs.len() + files.len() + 1);
  if dir.parent().is_some() {
    entries.push(DirEntryInfo {
      name: "..".to_string(),
      is_dir: true,
    });
  }
  entries.extend(dirs);
  entries.extend(files);
  Ok(entries)
}

/// A file name can hold control characters, which would move the cursor if
/// printed as they are.
fn clean(s: &str) -> String {
  s.chars()
    .map(|c| if c.is_control() { '?' } else { c })
    .collect()
}

fn display_width(s: &str) -> usize {
  s.chars().map(|c| c.width().unwrap_or(0)).sum()
}

/// `s` cut to `max` columns (ending in `…` when it was), then padded with
/// spaces to exactly `max`.
fn fit(s: &str, max: usize) -> String {
  let mut out = String::new();
  let mut used = 0;
  for c in s.chars() {
    let w = c.width().unwrap_or(0);
    if used + w > max.saturating_sub(1) && display_width(s) > max {
      out.push('…');
      used += 1;
      break;
    }
    out.push(c);
    used += w;
  }
  out.push_str(&" ".repeat(max.saturating_sub(used)));
  out
}

/// `s` cut to `max` columns keeping its end (a long path matters most at the
/// tail), with `…` where the start was cut.
fn tail_fit(s: &str, max: usize) -> String {
  if display_width(s) <= max {
    return s.to_string();
  }
  let mut kept: Vec<char> = Vec::new();
  let mut used = 1; // the ellipsis
  for c in s.chars().rev() {
    let w = c.width().unwrap_or(0);
    if used + w > max {
      break;
    }
    kept.push(c);
    used += w;
  }
  kept.reverse();
  format!("…{}", kept.into_iter().collect::<String>())
}

#[cfg(test)]
mod tests {
  use super::*;
  use crossterm::event::{KeyEventKind, KeyEventState, KeyModifiers};

  struct Scratch(PathBuf);

  impl Scratch {
    fn new(tag: &str) -> Self {
      let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
      let dir = std::env::temp_dir().join(format!("vtmate-fb-{}-{}-{}", tag, std::process::id(), nanos));
      std::fs::create_dir_all(&dir).unwrap();
      Scratch(dir)
    }
  }

  impl Drop for Scratch {
    fn drop(&mut self) {
      let _ = std::fs::remove_dir_all(&self.0);
    }
  }

  fn key(code: KeyCode) -> KeyEvent {
    KeyEvent {
      code,
      modifiers: KeyModifiers::NONE,
      kind: KeyEventKind::Press,
      state: KeyEventState::NONE,
    }
  }

  fn names(b: &FileBrowser) -> Vec<String> {
    b.entries.iter().map(|e| e.name.clone()).collect()
  }

  #[test]
  fn lists_folders_first_and_only_the_wanted_files() {
    let s = Scratch::new("list");
    std::fs::create_dir(s.0.join("zdir")).unwrap();
    std::fs::create_dir(s.0.join("Adir")).unwrap();
    for f in ["b.txt", "A.PDF", "c.png", "noext"] {
      std::fs::write(s.0.join(f), "x").unwrap();
    }
    let b = FileBrowser::open(s.0.clone(), &["txt", "pdf"]).unwrap();
    assert_eq!(names(&b), vec!["..", "Adir", "zdir", "A.PDF", "b.txt"]);
  }

  #[test]
  fn enter_opens_folders_and_picks_files() {
    let s = Scratch::new("enter");
    std::fs::create_dir(s.0.join("sub")).unwrap();
    std::fs::write(s.0.join("sub").join("a.txt"), "x").unwrap();
    let mut b = FileBrowser::open(s.0.clone(), &["txt"]).unwrap();
    b.selected = 1; // "sub"
    assert_eq!(b.handle_key(&key(KeyCode::Enter)), BrowserEvent::None);
    assert_eq!(b.current_dir, s.0.join("sub"));
    b.selected = 1; // "a.txt"
    assert_eq!(
      b.handle_key(&key(KeyCode::Enter)),
      BrowserEvent::Picked(s.0.join("sub").join("a.txt"))
    );
    assert_eq!(b.handle_key(&key(KeyCode::Esc)), BrowserEvent::Closed);
  }

  #[test]
  fn back_and_forward_walk_the_history() {
    let s = Scratch::new("hist");
    std::fs::create_dir(s.0.join("sub")).unwrap();
    let mut b = FileBrowser::open(s.0.clone(), &["txt"]).unwrap();
    b.selected = 1;
    b.navigate_into_selected();
    assert_eq!(b.current_dir, s.0.join("sub"));
    b.go_back();
    assert_eq!(b.current_dir, s.0);
    b.go_forward();
    assert_eq!(b.current_dir, s.0.join("sub"));
    // A fresh navigation drops the forward history.
    b.go_back();
    b.selected = 1;
    b.navigate_into_selected();
    b.go_back();
    b.go_forward();
    assert_eq!(b.current_dir, s.0.join("sub"));
  }

  #[test]
  fn backspace_goes_up_and_an_unreadable_folder_changes_nothing() {
    let s = Scratch::new("up");
    std::fs::create_dir(s.0.join("sub")).unwrap();
    let mut b = FileBrowser::open(s.0.join("sub"), &["txt"]).unwrap();
    b.handle_key(&key(KeyCode::Backspace));
    assert_eq!(b.current_dir, s.0);

    b.entries.push(DirEntryInfo {
      name: "gone".to_string(),
      is_dir: true,
    });
    b.selected = b.entries.len() - 1;
    let before = names(&b);
    b.navigate_into_selected();
    assert_eq!(b.current_dir, s.0);
    assert_eq!(names(&b), before);
    assert!(b.error.is_some());
  }

  #[test]
  fn fitting_text_to_columns() {
    assert_eq!(fit("ab", 5), "ab   ");
    assert_eq!(fit("abcdef", 4), "abc…");
    assert_eq!(tail_fit("/home/me/documents", 10), "…documents");
    assert_eq!(tail_fit("short", 10), "short");
  }
}
