// ------------------------------------------------------------------
//  Log
// ------------------------------------------------------------------

use crossbeam_channel::Sender;
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

static VERBOSE: AtomicBool = AtomicBool::new(false);

static TX_UI: OnceLock<Sender<String>> = OnceLock::new();

/// Optional plain-text log file (daemon mode). Diagnostics only: conversation
/// text never goes through here.
static FILE_SINK: OnceLock<Mutex<std::fs::File>> = OnceLock::new();

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

/// Append log lines to `path` (truncated first). Errors are always written,
/// other levels only in verbose mode.
pub fn set_file_sink(path: &std::path::Path) -> std::io::Result<()> {
  let file = std::fs::File::create(path)?;
  let _ = FILE_SINK.set(Mutex::new(file));
  Ok(())
}

pub fn log(msg_type: &str, msg: &str) {
  if !is_verbose() && msg_type != "error" && msg_type != "warning" {
    return;
  }
  if let Some(sink) = FILE_SINK.get() {
    if let Ok(mut f) = sink.lock() {
      let ts = chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f");
      let _ = writeln!(f, "{} [{}] {}", ts, msg_type, msg);
    }
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
    // The UI channel is bounded(1) and is drained only by the conversation-mode
    // UI loop, so in read-file mode or while the UI is busy the line may not be
    // taken. Wait briefly, then drop it: logging must never block the caller.
    let _ = sender.send_timeout(
      format!("line|{}", formatted),
      std::time::Duration::from_millis(200),
    );
  }
}
