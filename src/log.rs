// ------------------------------------------------------------------
//  Log
// ------------------------------------------------------------------

use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};

static VERBOSE: AtomicBool = AtomicBool::new(false);

// API
// ------------------------------------------------------------------

pub fn set_verbose(v: bool) {
  VERBOSE.store(v, Ordering::Relaxed);
}

pub fn is_verbose() -> bool {
  VERBOSE.load(Ordering::Relaxed)
}

pub fn log(msg_type: &str, msg: &str) {
  if !is_verbose() && msg_type != "error" {
    return;
  }
  let mut out = std::io::stdout();
  let emoji = match msg_type {
    "debug" => "🐛",
    "info" => "ℹ️",
    "warning" => "⚠️",
    "error" => "❌",
    _ => "",
  };
  write!(out, "\r\x1b[K{}  \x1b[90m{}\x1b[0m\n", emoji, msg).unwrap();
  out.flush().unwrap();
}
