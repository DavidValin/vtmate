// ------------------------------------------------------------------
//  Util
// ------------------------------------------------------------------

use crossterm::cursor::Show;
use crossterm::{
  cursor::MoveTo,
  execute,
  terminal::{Clear, ClearType},
};
use directories::UserDirs;
use encoding_rs::*;
use std::cell::Cell;
use std::fs;
use std::io::IsTerminal;
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::process;
use std::sync::OnceLock;
use std::sync::atomic::AtomicU64;
use std::thread;
use std::time::{Duration, Instant};

/// Global timestamp of last speech end (in ms since program start).
pub static SPEECH_END_AT: AtomicU64 = AtomicU64::new(0);

thread_local! {
  static IN_CODE_BLOCK: Cell<bool> = Cell::new(false);
}

// Read file or stdin with encoding fallback
pub fn read_file(path: &str) -> String {
  if path == "-" {
    // Read from stdin
    let mut stdin_bytes = Vec::new();
    io::stdin()
      .read_to_end(&mut stdin_bytes)
      .unwrap_or_else(|e| {
        crate::log::log("error", &format!("Failed to read stdin: {}", e));
        terminate(1);
      });
    match std::str::from_utf8(&stdin_bytes) {
      Ok(s) => s.to_string(),
      Err(_) => {
        let (decoded, _encoding, had_errors) = WINDOWS_1252.decode(&stdin_bytes);
        if !had_errors {
          // eprintln!("⚠️  Stdin encoded as Windows-1252/Latin-1, converting to UTF-8");
          decoded.to_string()
        } else {
          // eprintln!("⚠️  Stdin encoding unknown, using lossy UTF-8 conversion");
          String::from_utf8_lossy(&stdin_bytes).to_string()
        }
      }
    }
  } else {
    match fs::read_to_string(path) {
      Ok(c) => c,
      Err(_) => match fs::read(path) {
        Ok(bytes) => {
          if let Ok(s) = std::str::from_utf8(&bytes) {
            s.to_string()
          } else {
            let (decoded, _encoding, had_errors) = WINDOWS_1252.decode(&bytes);
            if !had_errors {
              // eprintln!("⚠️  File encoded as Windows-1252/Latin-1, converting to UTF-8");
              decoded.to_string()
            } else {
              // eprintln!("⚠️  File encoding unknown, using lossy UTF-8 conversion");
              String::from_utf8_lossy(&bytes).to_string()
            }
          }
        }
        Err(e) => {
          crate::log::log(
            "error",
            &format!("Failed to read file '{}' with error: {}", path, e),
          );
          terminate(1);
        }
      },
    }
  }
}

// ------------------------------------------------------------------

pub fn now_ms(start_instant: &OnceLock<Instant>) -> u64 {
  let start = start_instant.get_or_init(Instant::now);
  start.elapsed().as_millis() as u64
}

pub fn _env_f32(name: &str, default: f32) -> f32 {
  std::env::var(name)
    .ok()
    .and_then(|v| v.parse::<f32>().ok())
    .unwrap_or(default)
}

pub fn env_u64(name: &str, default: u64) -> u64 {
  std::env::var(name)
    .ok()
    .and_then(|v| v.parse::<u64>().ok())
    .unwrap_or(default)
}

pub fn get_flag(lang: &str) -> &str {
  match lang {
    "en" => "🇬🇧",
    "es" => "🇪🇸",
    "zh" => "🇨🇳",
    "ja" => "🇯🇵",
    "pt" => "🇵🇹",
    "it" => "🇮🇹",
    "hi" => "🇮🇳",
    "fr" => "🇫🇷",
    "ar" => "🇸🇦",
    "bn" => "🇧🇩",
    "ca" => "🇪🇸",
    "cs" => "🇨🇿",
    "de" => "🇩🇪",
    "el" => "🇬🇷",
    "fi" => "🇫🇮",
    "gu" => "🇮🇳",
    "hu" => "🇭🇺",
    "kn" => "🇮🇳",
    "ko" => "🇰🇷",
    "mr" => "🇮🇳",
    "nl" => "🇳🇱",
    "pa" => "🇮🇳",
    "ru" => "🇷🇺",
    "sv" => "🇸🇪",
    "sw" => "🇰🇪",
    "ta" => "🇮🇳",
    "te" => "🇮🇳",
    "tr" => "🇹🇷",
    _ => "",
  }
}

pub fn terminal_supported() -> bool {
  let is_tty = std::io::stdout().is_terminal();
  let term = std::env::var("TERM").unwrap_or_default();
  is_tty && term != "dumb"
}

/// Returns the current user's home directory.
/// Works on Unix (~, $HOME) and Windows.
pub fn get_user_home_path() -> Option<PathBuf> {
  if let Ok(h) = std::env::var("HOME") {
    Some(PathBuf::from(h))
  } else {
    UserDirs::new().map(|u| u.home_dir().to_path_buf())
  }
}

/// Strip special characters from text for TTS
/// Handles code blocks (text between ```) by not stripping chars inside them
/// Preserves unicode characters (accents, tildes, etc.)
pub fn strip_special_chars(s: &str) -> String {
  let mut result = String::new();
  let parts: Vec<&str> = s.split("```").collect();
  let mut inside = IN_CODE_BLOCK.with(|c| c.get());
  for (i, part) in parts.iter().enumerate() {
    if !inside {
      result.extend(part.chars().filter(|c| {
        // Keep letters (including unicode letters with accents), digits, spaces, and whitespace
        // Remove only specific punctuation marks
        if c.is_alphanumeric() || c.is_whitespace() {
          true
        } else {
          // Remove specific special characters
          ![
            '+', '.', '~', '*', '&', '-', ',', ';', ':', '(', ')', '[', ']', '{', '}', '"', '”',
            '\'', '#', '`', '|', '!', '?', '/', '\\', '<', '>', '=', '@', '$', '%', '^',
          ]
          .contains(c)
        }
      }));
    } else {
      // Inside code blocks, keep everything
      result.push_str(part);
    }
    // toggle after each fence except after last part
    if i < parts.len() - 1 {
      inside = !inside;
    }
  }
  IN_CODE_BLOCK.with(|c| c.set(inside));
  result
}

pub fn _strip_ansi(s: &str) -> String {
  let mut result = String::new();
  let mut in_escape = false;
  for c in s.chars() {
    if in_escape {
      if c == 'm' {
        in_escape = false;
      }
      continue;
    }
    if c == '\x1b' {
      in_escape = true;
      continue;
    }
    result.push(c);
  }
  result
}

pub fn terminate(code: i32) -> ! {
 // Disable raw mode if enabled, to restore terminal state
  let _ = crossterm::terminal::disable_raw_mode();
  // show cursor and clear bottom line before exiting
  let mut stdout = std::io::stdout();
  let (_cols, rows) = crossterm::terminal::size().unwrap_or((80, 24));
  let _ = execute!(
    stdout,
    MoveTo(0, rows.saturating_sub(1)),
    Clear(ClearType::CurrentLine),
    Show
  );
  stdout.flush().ok();
  thread::sleep(Duration::from_millis(100));
  process::exit(code);
}
