// ------------------------------------------------------------------
//  Log
// ------------------------------------------------------------------

use crossbeam_channel::Sender;
use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
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

/// A line that answers a command rather than reporting a diagnostic ("you are
/// now detached"): same look as the log, always shown, and printed straight to
/// the terminal because it is written while the UI is being torn down.
pub fn notice(msg_type: &str, msg: &str) {
  if let Some(sink) = FILE_SINK.get() {
    if let Ok(mut f) = sink.lock() {
      let ts = chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f");
      let _ = writeln!(f, "{} [{}] {}", ts, msg_type, msg);
    }
  }
  print!("\r\x1b[K{}  \x1b[90m{}\x1b[0m\r\n", emoji_for(msg_type), msg);
  let _ = std::io::Write::flush(&mut std::io::stdout());
}

fn emoji_for(msg_type: &str) -> &'static str {
  match msg_type {
    "debug" => "🐛",
    "info" => "ℹ️",
    "warning" => "⚠️",
    "error" => "❌",
    _ => "",
  }
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
  let formatted = format!("\r\x1b[K{}  \x1b[90m{}\x1b[0m\n", emoji_for(msg_type), msg);
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

// ------------------------------------------------------------------
//  ggml / whisper.cpp bridge
// ------------------------------------------------------------------

// ggml_log_level, from ggml.h.
const GGML_LOG_LEVEL_DEBUG: u32 = 1;
const GGML_LOG_LEVEL_INFO: u32 = 2;
const GGML_LOG_LEVEL_WARN: u32 = 3;
const GGML_LOG_LEVEL_ERROR: u32 = 4;
const GGML_LOG_LEVEL_CONT: u32 = 5;

/// The level of the last message that carried one: ggml continues a line by
/// logging the rest of it at GGML_LOG_LEVEL_CONT, which has no level itself.
static LAST_GGML_LEVEL: AtomicU32 = AtomicU32::new(GGML_LOG_LEVEL_INFO);

/// Take over the single C log callback that ggml and whisper.cpp share.
///
/// Not decoration: whisper-rs's `install_logging_hooks` replaces ggml's default
/// callback (which printed to stderr) with one that forwards to the `log`
/// crate, and release builds do not enable its `log_backend` feature - so the
/// messages went nowhere. A fatal CUDA error reached the user as nothing but
/// `ggml-cuda.cu:85: CUDA error`, the line GGML_ABORT prints itself, while the
/// three lines naming the actual error were dropped. Errors from here are
/// always written to stderr, whatever the UI and the log filter are doing.
pub fn install_ggml_log_callback() {
  // SAFETY: `ggml_log_trampoline` is a plain extern "C" fn that does not
  // unwind and keeps nothing; whisper_log_set passes it on to ggml_log_set.
  unsafe { whisper_rs::set_log_callback(Some(ggml_log_trampoline), std::ptr::null_mut()) };
}

unsafe extern "C" fn ggml_log_trampoline(
  level: u32,
  text: *const std::os::raw::c_char,
  _user_data: *mut std::os::raw::c_void,
) {
  if text.is_null() {
    return;
  }
  // SAFETY: ggml hands us a NUL-terminated string that outlives this call.
  let msg = unsafe { std::ffi::CStr::from_ptr(text) }.to_string_lossy();

  let level = if level == GGML_LOG_LEVEL_CONT {
    LAST_GGML_LEVEL.load(Ordering::Relaxed)
  } else {
    LAST_GGML_LEVEL.store(level, Ordering::Relaxed);
    level
  };

  if level >= GGML_LOG_LEVEL_ERROR {
    // Also straight to stderr, unformatted: what follows an error here is
    // usually GGML_ABORT, and by then the UI channel and the thread draining
    // it may be gone. write_all rather than eprint!, which panics on a failed
    // write - and unwinding out of an extern "C" frame is undefined behaviour.
    let mut err = std::io::stderr();
    let _ = err.write_all(msg.as_bytes());
    let _ = err.flush();
  }

  let trimmed = msg.trim_end_matches(['\r', '\n']);
  if trimmed.is_empty() {
    return;
  }
  match level {
    GGML_LOG_LEVEL_ERROR => log("error", trimmed),
    GGML_LOG_LEVEL_WARN => log("warning", trimmed),
    GGML_LOG_LEVEL_DEBUG | GGML_LOG_LEVEL_INFO => log("debug", trimmed),
    _ => {}
  }
}
