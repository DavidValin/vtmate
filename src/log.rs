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

pub fn log(msg_type: &str, msg: &str) {
  if !VERBOSE.load(Ordering::Relaxed) && msg_type != "error" {
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
  write!(out, "\r\x1b[K{}  {}\n", emoji, msg).unwrap();
  out.flush().unwrap();
}
