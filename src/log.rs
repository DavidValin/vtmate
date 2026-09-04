// ------------------------------------------------------------------
//  Log
// ------------------------------------------------------------------

use crossbeam_channel::Sender;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

static VERBOSE: AtomicBool = AtomicBool::new(false);

static TX_UI: OnceLock<Sender<String>> = OnceLock::new();

// API
// ------------------------------------------------------------------

pub fn set_verbose(v: bool) {
  VERBOSE.store(v, Ordering::Relaxed);
}

pub fn set_tx_ui_sender(sender: Sender<String>) {
  TX_UI.set(sender).ok();
}

pub fn is_verbose() -> bool {
  VERBOSE.load(Ordering::Relaxed)
}

pub fn log(msg_type: &str, msg: &str) {
  if !is_verbose() && msg_type != "error" {
    return;
  }
  let emoji = match msg_type {
    "debug" => "🐛",
    "info" => "ℹ️",
    "warning" => "⚠️",
    "error" => "❌",
    _ => "",
  };
  let formatted = format!("\r\x1b[K{}  \x1b[90m{}\x1b[0m\n", emoji, msg);
  if let Some(sender) = TX_UI.get() {
    // The UI channel is bounded(1) and is only drained by the conversation-mode
    // UI loop. In read-file mode (or while the UI is busy) a blocking send would
    // park the caller forever - e.g. the TTS thread on its second log line -
    // so wait briefly and drop the line rather than deadlock.
    let _ = sender.send_timeout(
      format!("line|{}", formatted),
      std::time::Duration::from_millis(200),
    );
  }
}
