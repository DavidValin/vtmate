// ------------------------------------------------------------------
//  Util
// ------------------------------------------------------------------

use std::io::IsTerminal;
use std::sync::atomic::AtomicU64;
use std::sync::OnceLock;
use std::time::Instant;

/// Global timestamp of last speech end (in ms since program start).
pub static SPEECH_END_AT: AtomicU64 = AtomicU64::new(0);

// API
// ------------------------------------------------------------------

use directories::UserDirs;
use std::path::PathBuf;

pub fn now_ms(start_instant: &OnceLock<Instant>) -> u64 {
  let start = start_instant.get_or_init(Instant::now);
  start.elapsed().as_millis() as u64
}

pub fn strip_ansi(s: &str) -> String {
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
    "fr" => "🇫🇷",
    "it" => "🇮🇹",
    "hi" => "🇮🇳",
    "pt" => "🇵🇹",
    "ar" => "🇸🇦",
    "bn" => "🇧🇩",
    "ca" => "🇪🇸",
    "cs" => "🇨🇿",
    "de" => "🇩🇪",
    "el" => "🇬🇷",
    "fi" => "🇫🇮",
    "gu" => "🇮🇳",
    "hu" => "🇭🇺",
    "ko" => "🇰🇷",
    "mr" => "🇮🇳",
    "nl" => "🇳🇱",
    "pa" => "🇮🇳",
    "ru" => "🇷🇺",
    "sv" => "🇸🇪",
    "sw" => "🇹🇿",
    "ta" => "🇮🇳",
    "te" => "🇮🇳",
    "tr" => "🇹🇷",
    "kn" => "🇮🇳",
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
