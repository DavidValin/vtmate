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

/// A GPU failure as one short phrase, for the line on screen.
///
/// ONNX Runtime reports a full card as fifteen lines of C++ template
/// signatures and build paths, in which the only word that matters is
/// "memory". The screen gets the sentence; the raw text goes to the log,
/// which only `--verbose` shows.
///
/// Unrecognised trouble gets a vague phrase rather than a guess: the fallback
/// happens either way, and being wrong about why is worse than saying little.
pub fn describe_gpu_failure(msg: &str) -> &'static str {
  let msg = msg.to_ascii_lowercase();
  let has = |needles: &[&str]| needles.iter().any(|needle| msg.contains(needle));

  // Before the initialisation cases: a full card often reports itself as a
  // failure to initialise (cuBLAS needs a workspace for its handle), and out
  // of memory is both likelier and more useful to say.
  if has(&[
    "alloc_failed",
    "out of memory",
    "outofmemory",
    "cudaerrormemoryallocation",
    "cuda_error_out_of_memory",
  ]) {
    return "the GPU is out of memory";
  }
  if has(&[
    "no cuda-capable device",
    "cudaerrornodevice",
    "no kernel image",
  ]) {
    return "no usable GPU was found";
  }
  if has(&["driver version", "cudaerrorinsufficientdriver"]) {
    return "the GPU driver is too old for this build";
  }
  if has(&["not_initialized", "initializationerror"]) {
    return "the GPU could not be initialised";
  }
  "the GPU refused the work"
}

use crossterm::cursor::Show;
use crossterm::{
  cursor::MoveTo,
  execute,
  terminal::{Clear, ClearType, LeaveAlternateScreen},
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
          // eprintln!("▲  Stdin encoded as Windows-1252/Latin-1, converting to UTF-8");
          decoded.to_string()
        } else {
          // eprintln!("▲  Stdin encoding unknown, using lossy UTF-8 conversion");
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
              // eprintln!("▲  File encoded as Windows-1252/Latin-1, converting to UTF-8");
              decoded.to_string()
            } else {
              // eprintln!("▲  File encoding unknown, using lossy UTF-8 conversion");
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

/// The language as a two-letter code, upper case.
///
/// Two columns on every terminal, which a flag is not: a flag is a pair of
/// regional indicators, drawn as one two-column glyph where there is a font
/// for it and as two letters in four columns where there is not, so nothing
/// laying out a line can rely on its width.
pub fn lang_code(lang: &str) -> String {
  lang
    .chars()
    .filter(|c| c.is_ascii_alphabetic())
    .take(2)
    .collect::<String>()
    .to_ascii_uppercase()
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
mod lang_code_tests {
  use super::lang_code;

  #[test]
  fn every_language_is_two_columns() {
    for lang in [
      "en", "es", "zh", "ja", "pt", "it", "hi", "fr", "ar", "bn", "ca", "cs", "de", "el", "fi",
      "gu", "hu", "kn", "ko", "mr", "nl", "pa", "ru", "sv", "sw", "ta", "te", "tr", "bg", "hr",
      "da", "et", "id", "lt", "lv", "pl", "ro", "sk", "sl", "uk", "vi",
    ] {
      assert_eq!(lang_code(lang).chars().count(), 2, "{}", lang);
    }
  }

  #[test]
  fn codes_are_upper_case_and_ascii() {
    assert_eq!(lang_code("es"), "ES");
    assert_eq!(lang_code("EN"), "EN");
    // A longer tag keeps its first two letters; a region suffix is dropped.
    assert_eq!(lang_code("cmn"), "CM");
    assert_eq!(lang_code("pt-BR"), "PT");
    assert_eq!(lang_code(""), "");
  }
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
  fn a_hard_wrapped_paragraph_is_one_block_split_into_its_sentences() {
    // Four source lines, none ending its own paragraph until the last:
    // wrapped mid-sentence at every line break except the final one.
    let text = "Logic is founded on certain thought, which\n\
                were first formulated by a philosopher, an ancient\n\
                thinker. We shall describe them separately here, and\n\
                later consider their collective significance.";
    let out = split_text_for_tts(text, false);
    // One block: every phrase points at the same (line, text).
    assert!(out.iter().all(|p| p.line == 0 && p.text == text));
    // Split into its two sentences for speech, each keeping its full stop.
    assert_eq!(
      out.iter().map(|p| p.tts.as_str()).collect::<Vec<_>>(),
      [
        "Logic is founded on certain thought, which were first formulated by a philosopher, an ancient thinker.",
        "We shall describe them separately here, and later consider their collective significance."
      ]
    );

    // A blank line still ends the paragraph even without a full stop, and a
    // list item is always its own block.
    let with_break_and_list = "No stop here\nand still none\n\n- item one\n- item two";
    let shape = |t: &str| -> Vec<(usize, String, String)> {
      split_text_for_tts(t, false)
        .into_iter()
        .map(|p| (p.line, p.text, p.tts))
        .collect()
    };
    assert_eq!(shape(with_break_and_list), [
      (0, "No stop here\nand still none".to_string(), "No stop here and still none".to_string()),
      (1, "- item one".to_string(), "item one".to_string()),
      (2, "- item two".to_string(), "item two".to_string()),
    ]);
  }

  #[test]
  fn every_numbered_list_style_is_its_own_item() {
    let shape = |t: &str| -> Vec<String> {
      split_text_for_tts(t, false).into_iter().map(|p| p.tts).collect()
    };
    for text in [
      "1. First phrase\n2. Second phrase",
      "1- First phrase\n2- Second phrase",
      "1 First phrase\n2 Second phrase",
    ] {
      assert_eq!(shape(text), ["First phrase", "Second phrase"], "{}", text);
    }
  }

  #[test]
  fn every_bullet_glyph_is_its_own_item() {
    let shape = |t: &str| -> Vec<String> {
      split_text_for_tts(t, false).into_iter().map(|p| p.tts).collect()
    };
    for text in [
      "- First phrase\n- Second phrase",
      "* First phrase\n* Second phrase",
      "\u{2022} First phrase\n\u{2022} Second phrase", // •
      "\u{25e6} First phrase\n\u{25e6} Second phrase", // ◦
      "\u{2023} First phrase\n\u{2023} Second phrase", // ‣
      "\u{25aa} First phrase\n\u{25aa} Second phrase", // ▪
    ] {
      assert_eq!(shape(text), ["First phrase", "Second phrase"], "{}", text);
    }
  }

  #[test]
  fn a_line_starting_with_a_number_is_its_own_item_regardless_of_neighbours() {
    // Every one judged only on itself - no relation between the two numbers,
    // no run to confirm, each still gets its bare "N " marker stripped.
    let shape = |t: &str| -> Vec<String> {
      split_text_for_tts(t, false).into_iter().map(|p| p.tts).collect()
    };
    assert_eq!(
      shape("1 First phrase\n2 Second phrase"),
      ["First phrase", "Second phrase"]
    );
    assert_eq!(
      shape("3 apples were bought.\nThey were eaten."),
      ["apples were bought.", "They were eaten."]
    );
    assert_eq!(
      shape("5 widgets shipped.\n7 more were ordered."),
      ["widgets shipped.", "more were ordered."]
    );
    // A year opening a list entry, the case that motivated this: judged on
    // its own line, not on whether another entry happens to share the year.
    assert_eq!(
      shape("1926 something happened.\n1926 something else happened."),
      ["something happened.", "something else happened."]
    );
  }

  #[test]
  fn an_indented_line_is_its_own_item_even_with_no_bullet() {
    // What a browser hands back for a rendered `<li>` list copied to plain
    // text: no bullet character at all, just a few spaces of indentation
    // per entry - real text copied from a Wikipedia portal page.
    let text = "Actualidad\n\n    Guerras y conflictos: Estados Unidos-Ir\u{e1}n\n    13-26 de septiembre: WXV Global Series Challenger\n    12-26 de septiembre: Juegos Suramericanos";
    let shape = |t: &str| -> Vec<(usize, String)> {
      split_text_for_tts(t, false)
        .into_iter()
        .map(|p| (p.line, p.tts))
        .collect()
    };
    // strip_special_chars drops '-' along with the other punctuation it
    // filters out; unrelated to indentation, just how any hyphen is spoken.
    assert_eq!(shape(text), [
      (0, "Actualidad".to_string()),
      (1, "Guerras y conflictos: Estados UnidosIr\u{e1}n".to_string()),
      (2, "1326 de septiembre: WXV Global Series Challenger".to_string()),
      (3, "1226 de septiembre: Juegos Suramericanos".to_string()),
    ]);
  }

  #[test]
  fn a_heading_ends_its_own_block_even_without_a_blank_line() {
    let shape = |t: &str| -> Vec<(usize, String, String)> {
      split_text_for_tts(t, false)
        .into_iter()
        .map(|p| (p.line, p.text, p.tts))
        .collect()
    };
    assert_eq!(
      shape("THE LAWS OF THOUGHT\nThis is a second phrase."),
      [
        (0, "THE LAWS OF THOUGHT".to_string(), "THE LAWS OF THOUGHT".to_string()),
        (1, "This is a second phrase.".to_string(), "This is a second phrase.".to_string()),
      ]
    );

    // A heading opening the text splits off on its own, even though nothing
    // came before it to flush.
    let title_then_paragraph = "THE FOUNDATIONS OF LOGIC\n\
                                 Logic is founded on certain laws of thought, which\n\
                                 were first formulated by Aristotle, an ancient Greek\n\
                                 philosopher. We shall describe them separately here, and\n\
                                 later consider their collective significance.";
    let out = split_text_for_tts(title_then_paragraph, false);
    let blocks: Vec<usize> = out.iter().map(|p| p.line).collect();
    assert_eq!(blocks, [0, 1, 1], "expected a title block plus two sentences in the paragraph block");
    assert_eq!(out[0].text, "THE FOUNDATIONS OF LOGIC");
    assert!(out[1].text.starts_with("Logic is founded") && out[1].text == out[2].text);

    // A heading in the middle of running prose splits the prose around it
    // into three blocks, instead of trailing onto whatever came before it.
    let heading_mid_paragraph = "Every appearance as such is objectively given and has a\n\
                                  certain content or specificity. We can and should and\n\
                                  commonly do initially regard it with a simple attitude of8\n\
                                  THE LAWS OF THOUGHT\n\
                                  receptiveness and attention to detail. Every appearance is\n\
                                  in itself neutral; the qualification of an appearance (thus\n\
                                  broadly defined) as a reality or an illusion, is a\n\
                                  subsequent issue.";
    let out = split_text_for_tts(heading_mid_paragraph, false);
    let block_texts: Vec<&str> = {
      let mut seen = Vec::new();
      for p in &out {
        if !seen.contains(&p.text.as_str()) {
          seen.push(p.text.as_str());
        }
      }
      seen
    };
    assert_eq!(block_texts.len(), 3, "{:?}", block_texts);
    assert!(block_texts[0].starts_with("Every appearance"));
    assert_eq!(block_texts[1], "THE LAWS OF THOUGHT");
    assert!(block_texts[2].starts_with("receptiveness"));
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

/// Split free text for reading aloud. Returns `(display_block, tts_text)`
/// pairs.
///
/// The two are split differently on purpose. Speaking breaks at every
/// `. ! ? ;`, because each phrase is synthesized on its own and that is what
/// gives the reading its pauses. Displaying breaks at paragraphs instead: a
/// hard-wrapped source line only ends its block when it reads as a genuine
/// paragraph boundary - the line itself ends with '.', a blank line follows,
/// it is a list item (a bullet, a numbered entry, or just indented - a
/// browser copying a rendered `<li>` often does that with no marker at all -
/// always its own block so a list still highlights item by item), or it is a
/// heading (every letter in it uppercase, so whatever follows starts a fresh
/// block instead of trailing on from it) - so a paragraph wrapped across
/// several source lines still highlights, and is navigated, as one.
///
/// `tts_text` has special characters stripped and can be empty, for a block
/// with nothing to say (a rule, a row of dashes, code that is being skipped);
/// such a block is still returned so it can be shown and stepped over.
///
/// `skip_code` decides whether fenced ``` code is spoken. An agent's reply
/// skips it, since hearing brackets and punctuation read out is useless. Text
/// you asked to have read, with `-r` or the read-aloud shortcut, keeps it: the
/// code is part of what you asked for.
pub struct SpokenPhrase {
  /// Which display block this phrase belongs to. Several phrases share a
  /// block when it holds more than one sentence.
  pub line: usize,
  /// The block as written (its source lines joined by '\n'): what gets
  /// displayed and navigated through.
  pub text: String,
  /// What to speak; empty for a block with nothing to say.
  pub tts: String,
}

/// Bullet glyphs a browser's plain-text list markup might use: the plain
/// `-`/`*` a person types, plus the common Unicode bullets a browser emits
/// for a rendered `<li>` (a filled, ring, or triangular bullet, or a small
/// square) instead of leaving it to indentation alone.
const BULLET_CHARS: [char; 6] = ['-', '*', '•', '◦', '‣', '▪'];

/// A bullet, or a numbered entry - "1.", "1)", "1-" or a bare "1 " - each
/// followed by a space: always its own block, whatever it ends with, so a
/// list still highlights and is spoken item by item. A line starting with a
/// number is judged entirely on its own; nothing here looks at neighbouring
/// lines to decide.
fn is_list_line(line: &str) -> bool {
  if let Some(rest) = line.strip_prefix(BULLET_CHARS.as_slice()) {
    return rest.starts_with(' ');
  }
  let digits = line.len() - line.trim_start_matches(|c: char| c.is_ascii_digit()).len();
  if digits == 0 {
    return false;
  }
  let rest = &line[digits..];
  if rest.starts_with(' ') {
    return true;
  }
  let mut chars = rest.chars();
  matches!(chars.next(), Some('.') | Some(')') | Some('-')) && chars.as_str().starts_with(' ')
}

/// The text of a list line after its marker - the bullet or the numbering
/// and whatever separates it from the text are for layout, not for the ear,
/// so a numbered item is spoken as "First phrase", never "one dot First
/// phrase".
fn strip_list_marker(line: &str) -> &str {
  if let Some(rest) = line.strip_prefix(BULLET_CHARS.as_slice()) {
    return rest.trim_start();
  }
  let digits = line.len() - line.trim_start_matches(|c: char| c.is_ascii_digit()).len();
  line[digits..].trim_start_matches(['.', ')', '-']).trim_start()
}

/// A heading: has at least one letter, and every letter in it is uppercase.
/// Always ends its own block, so whatever follows starts a fresh one instead
/// of trailing on from the heading.
fn is_all_caps_line(line: &str) -> bool {
  let mut has_letter = false;
  for c in line.chars() {
    if c.is_alphabetic() {
      has_letter = true;
      if !c.is_uppercase() {
        return false;
      }
    }
  }
  has_letter
}

/// Turn everything collected in `block_display`/`block_spoken` into phrases
/// of `block_no`, splitting the spoken text at every delimiter, then reset
/// both for the next block.
fn flush_block(
  phrases: &mut Vec<SpokenPhrase>,
  block_no: &mut usize,
  block_display: &mut Vec<&str>,
  block_spoken: &mut String,
) {
  let display = block_display.join("\n");
  let before = phrases.len();
  let mut current = String::new();
  for ch in block_spoken.chars() {
    current.push(ch);
    if TTS_DELIMITERS.contains(&ch) {
      push_spoken(phrases, *block_no, &display, &mut current);
    }
  }
  push_spoken(phrases, *block_no, &display, &mut current);
  if phrases.len() == before {
    phrases.push(SpokenPhrase {
      line: *block_no,
      text: display,
      tts: String::new(),
    });
  }
  *block_no += 1;
  block_display.clear();
  block_spoken.clear();
}

pub fn split_text_for_tts(content: &str, skip_code: bool) -> Vec<SpokenPhrase> {
  let mut phrases: Vec<SpokenPhrase> = Vec::new();
  let mut block_no = 0usize;
  // Fence state carries across lines, in order.
  let mut in_code = false;

  let raw_lines: Vec<&str> = content.lines().collect();
  let mut block_display: Vec<&str> = Vec::new();
  let mut block_spoken = String::new();

  for (i, &raw) in raw_lines.iter().enumerate() {
    // A browser copying a rendered `<li>` to plain text commonly indents it
    // a few spaces instead of (or as well as) giving it a bullet character,
    // so indentation is its own "this is a list entry" signal, same as a
    // leading `-`/`*`/number.
    let is_indented = raw.starts_with(' ') || raw.starts_with('\t');
    let line = raw.trim();
    if line.is_empty() {
      continue;
    }
    let is_list = is_list_line(line);
    let is_standalone = is_list || is_all_caps_line(line) || is_indented;

    // A list item or a heading is always its own block: flush whatever was
    // accumulating before it first, so it splits off instead of trailing on
    // whatever precedes it.
    if is_standalone && !block_display.is_empty() {
      flush_block(&mut phrases, &mut block_no, &mut block_display, &mut block_spoken);
    }

    let to_speak = if is_list { strip_list_marker(line) } else { line };
    // Cleaned a whole source line at a time, so the fence state stays in step
    // however the line is broken up afterwards.
    let cleaned = if skip_code {
      tts_text(to_speak, &mut in_code)
    } else {
      // keep the code, only drop the fence markers themselves
      strip_special_chars(&to_speak.replace("```", " "))
    };

    block_display.push(line);
    if !block_spoken.is_empty() && !cleaned.is_empty() {
      block_spoken.push(' ');
    }
    block_spoken.push_str(&cleaned);

    let is_last = i + 1 >= raw_lines.len();
    let next_is_blank = !is_last && raw_lines[i + 1].trim().is_empty();
    let block_ends = is_standalone || line.ends_with('.') || next_is_blank || is_last;
    if !block_ends {
      continue;
    }

    flush_block(&mut phrases, &mut block_no, &mut block_display, &mut block_spoken);
  }
  phrases
}

/// Move what has been collected into `phrases` as one spoken phrase of
/// `block_no`, unless there is nothing in it to say.
fn push_spoken(
  phrases: &mut Vec<SpokenPhrase>,
  block_no: usize,
  display: &str,
  current: &mut String,
) {
  let spoken = current.trim();
  if spoken.chars().any(char::is_alphanumeric) {
    phrases.push(SpokenPhrase {
      line: block_no,
      text: display.to_string(),
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
  // Bounded wait: a daemon-attach reader thread keeps feeding the UI thread
  // regardless of this call, so it can still be writing after the flag above
  // is set. A stuck UI thread must not hang the exit.
  crate::ui::wait_for_ui_stopped(std::time::Duration::from_millis(200));
  // close the wav of the turn --save-html was still recording
  crate::html_export::finish();
  // Drop `-s`'s wav sender, if any: the writer thread only finalizes the
  // file (patches its header's size fields) once every sender clone is
  // gone, and this is the one exit path every close, Ctrl-C and read-file
  // mode's own included, funnels through - `process::exit` below skips
  // destructors, so a static holding the sender would otherwise keep it
  // open forever instead of just until the process happens to end.
  crate::playback::clear_wav_tx();
  run_exit_hook();
   // Disable raw mode if enabled, to restore terminal state
   let _ = crossterm::terminal::disable_raw_mode();
  // Leave the alternate screen only if something (e.g. --clone-voice's
  // progress popup, or the settings/debate popup) actually switched to it -
  // this is the only exit path every close, Ctrl-C included, funnels
  // through, so never leaving would strand the terminal on a blank alternate
  // screen after the process is gone. But sending this unconditionally is
  // not the harmless no-op it looks like on the primary screen: CSI ?1049l
  // restores whatever cursor position a terminal last saved for CSI ?1049h,
  // even with no matching enter this session, which can snap the cursor to
  // an unrelated, stale position instead of leaving it where the very last
  // thing this process printed put it.
  let mut stdout = std::io::stdout();
  if crate::ui::ON_ALT_SCREEN.load(std::sync::atomic::Ordering::Relaxed) {
    let _ = execute!(stdout, LeaveAlternateScreen);
  }
  // show cursor and clear bottom line before exiting
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
