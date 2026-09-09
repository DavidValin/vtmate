// ------------------------------------------------------------------
//  Util
// ------------------------------------------------------------------

/// Does this error text mean "not on this GPU" rather than "not at all"?
///
/// The accelerated backends report GPU trouble as free-form text from C++, so
/// substring matching is what there is: ONNX Runtime surfaces a full card as
/// "CUBLAS failure 3: CUBLAS_STATUS_ALLOC_FAILED" out of cublasCreate, ggml as
/// a CUDA error. Callers use it to decide whether retrying on the CPU is worth
/// it - speech and transcription matter more than the acceleration, and on a
/// shared GPU (an LLM in the same VRAM, say) the failure is transient in cause
/// but permanent for this run.
pub fn looks_like_gpu_failure(msg: &str) -> bool {
  let msg = msg.to_ascii_lowercase();
  ["cuda", "cublas", "cudnn", "gpu", "vulkan", "out of memory"]
    .iter()
    .any(|needle| msg.contains(needle))
}

use crossterm::cursor::Show;
use crossterm::{
  cursor::MoveTo,
  execute,
  terminal::{Clear, ClearType},
};
use directories::UserDirs;
use encoding_rs::*;
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
    "bg" => "🇧🇬",
    "hr" => "🇭🇷",
    "da" => "🇩🇰",
    "et" => "🇪🇪",
    "id" => "🇮🇩",
    "lt" => "🇱🇹",
    "lv" => "🇱🇻",
    "pl" => "🇵🇱",
    "ro" => "🇷🇴",
    "sk" => "🇸🇰",
    "sl" => "🇸🇮",
    "uk" => "🇺🇦",
    "vi" => "🇻🇳",
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

/// Remove fenced source code (everything between ``` fences, fences included)
/// from text that is going to be spoken. Source code is never sent to TTS.
///
/// A code block usually spans several phrases, so `in_code` carries the fence
/// state from one call to the next. Callers keep one flag per reply / file and
/// feed the phrases in order.
pub fn strip_code_blocks(s: &str, in_code: &mut bool) -> String {
  let mut result = String::new();
  let parts: Vec<&str> = s.split("```").collect();
  for (i, part) in parts.iter().enumerate() {
    if !*in_code {
      result.push_str(part);
    }
    // toggle after each fence except after last part
    if i < parts.len() - 1 {
      *in_code = !*in_code;
    }
  }
  result
}

/// Strip special characters from text for TTS.
/// Preserves unicode characters (accents, tildes, etc.)
/// NOTE: Keeps sentence-ending punctuation (. ! ?) and commas intact
/// because TTS models need them for proper sentence boundary detection
/// and natural speech rhythm.
pub fn strip_special_chars(s: &str) -> String {
  s.chars()
    .filter(|c| {
      // Keep letters (including unicode letters with accents), digits, spaces, and whitespace
      // Remove only specific special characters (keep . ! ? , ; : for TTS)
      if c.is_alphanumeric() || c.is_whitespace() {
        true
      } else {
        ![
          '+', '~', '*', '&', '-', '(', ')', '[', ']', '{', '}', '"', '”', '\'', '#', '`', '|',
          '/', '\\', '<', '>', '=', '@', '$', '%', '^',
        ]
        .contains(c)
      }
    })
    .collect()
}

/// Text to hand to TTS for one phrase: fenced code removed (stateful across
/// phrases through `in_code`), then special characters stripped.
pub fn tts_text(phrase: &str, in_code: &mut bool) -> String {
  strip_special_chars(&strip_code_blocks(phrase, in_code))
}

#[cfg(test)]
mod tts_text_tests {
  use super::*;

  #[test]
  fn code_blocks_are_never_spoken() {
    let mut in_code = false;
    // fence opened and closed inside one phrase
    assert_eq!(
      tts_text("Use this: ```rust\nfn main() {}\n``` and run it.", &mut in_code),
      "Use this:  and run it."
    );
    assert!(!in_code);
    // fence spanning several phrases
    assert_eq!(tts_text("Here is the code:", &mut in_code), "Here is the code:");
    assert_eq!(tts_text("```python", &mut in_code), "");
    assert!(in_code);
    assert_eq!(tts_text("print('hello world.')", &mut in_code), "");
    assert_eq!(tts_text("x = 1.", &mut in_code), "");
    assert_eq!(tts_text("```", &mut in_code), "");
    assert!(!in_code);
    assert_eq!(tts_text("That prints hello.", &mut in_code), "That prints hello.");
  }

  #[test]
  fn lines_split_the_display_and_punctuation_splits_the_speech() {
    let shape = |text: &str| -> Vec<(usize, String, String)> {
      split_text_for_tts(text, false)
        .into_iter()
        .map(|p| (p.line, p.text, p.tts))
        .collect()
    };

    // a list is spoken item by item, and each item is a line of its own
    assert_eq!(shape("* First phrase\n* Second phrase"), [
      (0, "* First phrase".to_string(), "First phrase".to_string()),
      (1, "* Second phrase".to_string(), "Second phrase".to_string()),
    ]);

    // . ! ? ; each end a spoken phrase inside a line and stay attached to it,
    // while all of them keep pointing at the one line they came from
    let out = split_text_for_tts("Ready? Set! Go; now.", false);
    assert_eq!(
      out.iter().map(|p| p.tts.as_str()).collect::<Vec<_>>(),
      ["Ready?", "Set!", "Go;", "now."]
    );
    assert!(out.iter().all(|p| p.line == 0 && p.text == "Ready? Set! Go; now."));

    // two identical lines stay two lines
    assert_eq!(shape("- milk\n- milk"), [
      (0, "- milk".to_string(), "milk".to_string()),
      (1, "- milk".to_string(), "milk".to_string()),
    ]);

    // a line with nothing to say is kept, so it can be shown and stepped over
    assert_eq!(shape("---"), [(0, "---".to_string(), String::new())]);
  }

  #[test]
  fn special_chars_stripped_but_punctuation_kept() {
    let mut in_code = false;
    assert_eq!(tts_text("Hola, ¿qué tal? *bien* (ok)!", &mut in_code), "Hola, ¿qué tal? bien ok!");
  }
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

/// Delimiters that end a spoken phrase inside a line. They stay attached to
/// the phrase they close: the TTS engines read them for intonation and pacing,
/// so a question keeps its rise and a sentence its fall.
const TTS_DELIMITERS: [char; 4] = ['.', '!', '?', ';'];

/// Split free text for reading aloud. Returns `(display_line, tts_text)` pairs.
///
/// The two are split differently on purpose. Speaking breaks at every line and
/// at every `. ! ? ;` inside one, because each phrase is synthesized on its own
/// and that is what gives the reading its pauses. Displaying breaks at lines
/// only: a line split into several spoken phrases is shown once, as it was
/// written, so the text on screen still looks like the text that was selected.
///
/// `tts_text` has special characters stripped and can be empty, for a line with
/// nothing to say (a rule, a row of dashes, code that is being skipped); such a
/// line is still returned so it can be shown and stepped over.
///
/// `skip_code` decides whether fenced ``` code is spoken. An agent's reply
/// skips it, since hearing brackets and punctuation read out is useless. Text
/// you asked to have read, with `-r` or the read-aloud shortcut, keeps it: the
/// code is part of what you asked for.
pub struct SpokenPhrase {
  /// Which display line this phrase belongs to. Several phrases share a line
  /// when it holds more than one sentence.
  pub line: usize,
  /// The line as written: what gets displayed and navigated through.
  pub text: String,
  /// What to speak; empty for a line with nothing to say.
  pub tts: String,
}

pub fn split_text_for_tts(content: &str, skip_code: bool) -> Vec<SpokenPhrase> {
  let mut phrases: Vec<SpokenPhrase> = Vec::new();
  let mut line_no = 0usize;
  // Fence state carries across lines, in order.
  let mut in_code = false;
  for line in content.lines() {
    let line = line.trim();
    if line.is_empty() {
      continue;
    }
    // Cleaned a whole line at a time, so the fence state stays in step however
    // the line is broken up afterwards.
    let spoken = if skip_code {
      tts_text(line, &mut in_code)
    } else {
      // keep the code, only drop the fence markers themselves
      strip_special_chars(&line.replace("```", " "))
    };
    let before = phrases.len();
    let mut current = String::new();
    for ch in spoken.chars() {
      current.push(ch);
      if TTS_DELIMITERS.contains(&ch) {
        push_spoken(&mut phrases, line_no, line, &mut current);
      }
    }
    push_spoken(&mut phrases, line_no, line, &mut current);
    if phrases.len() == before {
      phrases.push(SpokenPhrase {
        line: line_no,
        text: line.to_string(),
        tts: String::new(),
      });
    }
    line_no += 1;
  }
  phrases
}

/// Move what has been collected into `phrases` as one spoken phrase of `line`,
/// unless there is nothing in it to say.
fn push_spoken(
  phrases: &mut Vec<SpokenPhrase>,
  line_no: usize,
  line: &str,
  current: &mut String,
) {
  let spoken = current.trim();
  if spoken.chars().any(char::is_alphanumeric) {
    phrases.push(SpokenPhrase {
      line: line_no,
      text: line.to_string(),
      tts: spoken.to_string(),
    });
  }
  current.clear();
}

static EXIT_HOOK: OnceLock<Box<dyn Fn() + Send + Sync>> = OnceLock::new();

/// Set when the caller has already written the last line the user should read.
/// Exiting normally wipes the bottom line (that is where the status bar sits),
/// which would erase that message if the screen had not scrolled it up first.
pub static EXIT_LINE_PRINTED: std::sync::atomic::AtomicBool =
  std::sync::atomic::AtomicBool::new(false);

/// Register a function that `terminate` runs before exiting (the daemon uses
/// it to remove its pid and socket files). Only the first hook is kept.
pub fn set_exit_hook(hook: Box<dyn Fn() + Send + Sync>) {
  let _ = EXIT_HOOK.set(hook);
}

pub fn run_exit_hook() {
  if let Some(hook) = EXIT_HOOK.get() {
    hook();
  }
}

pub fn terminate(code: i32) -> ! {
  // no more bottom bars: whatever is on screen now is the last thing shown
  crate::ui::UI_SHUTDOWN.store(true, std::sync::atomic::Ordering::Relaxed);
  // close the wav of the turn --save-html was still recording
  crate::html_export::finish();
  run_exit_hook();
   // Disable raw mode if enabled, to restore terminal state
   let _ = crossterm::terminal::disable_raw_mode();
  // show cursor and clear bottom line before exiting
  let mut stdout = std::io::stdout();
  let (_cols, rows) = crossterm::terminal::size().unwrap_or((80, 24));
  if EXIT_LINE_PRINTED.load(std::sync::atomic::Ordering::Relaxed) {
    let _ = execute!(stdout, Show);
  } else {
    let _ = execute!(
      stdout,
      MoveTo(0, rows.saturating_sub(1)),
      Clear(ClearType::CurrentLine),
      Show
    );
  }
  stdout.flush().ok();
  thread::sleep(Duration::from_millis(100));
  process::exit(code);
}
