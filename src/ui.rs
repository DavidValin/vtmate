// ------------------------------------------------------------------
//  UI (single renderer thread)
// ------------------------------------------------------------------

use crossbeam_channel::Receiver;
use crossterm::terminal;
use std::io::Write;
use std::sync::{
  Arc, Mutex,
  atomic::{AtomicBool, Ordering},
};
use std::thread;
use std::time::Duration;

// API
// ------------------------------------------------------------------

#[derive(Clone)]
pub struct UiState {
  pub thinking: Arc<AtomicBool>,
  pub playing: Arc<AtomicBool>,
  pub speaking: Arc<AtomicBool>, // voice activity flag
  pub peak: Arc<Mutex<f32>>,     // current audio peak
}

// ANSI label styling
pub const USER_LABEL: &str = "\x1b[47;30mUSER:\x1b[0m"; // white bg, black text
pub const ASSIST_LABEL: &str = "\x1b[48;5;22;37mASSISTANT:\x1b[0m"; // dark green bg, white text

pub fn spawn_ui_thread(
  ui: UiState,
  stop_all_rx: Receiver<()>,
  status_line: Arc<Mutex<String>>,
  print_lock: Arc<Mutex<()>>,
  peak: Arc<Mutex<f32>>,
) -> thread::JoinHandle<()> {
  thread::spawn(move || {
    let spinner = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
    let mut i = 0usize;

    // hide cursor (stdout)
    print!("\x1b[?25l");
    let _ = std::io::stdout().flush();

    loop {
      if stop_all_rx.try_recv().is_ok() {
        break;
      }

      let speak = ui.speaking.load(Ordering::Relaxed);
      let think = ui.thinking.load(Ordering::Relaxed);
      let play = ui.playing.load(Ordering::Relaxed);

      // Priority: playing > thinking > idle/speaking.
      // This ensures we show the speaking emoji any time audio isn't playing,
      // even if a stale thinking flag is briefly set.
      let status = if think {
        format!("🤔. {}", spinner[i % spinner.len()])
      } else if speak {
        format!("🎙  {}", spinner[i % spinner.len()])
      } else if play {
        format!("🔊  {}", spinner[i % spinner.len()])
      } else {
        format!("🎙  {}", spinner[i % spinner.len()])
      };
      // draw amplitude bar
      let (cols, _) = terminal::size().unwrap_or((80, 24));
      let peak_val = match peak.lock() {
        Ok(v) => *v,
        Err(_) => 0.0,
      };
      let bar_len = (peak_val * (cols as f32)).round() as usize;
      let bar_color = if ui.speaking.load(Ordering::Relaxed) {
        "\x1b[31m"
      } else {
        "\x1b[37m"
      };
      let bar = format!("{}{}\x1b[0m", bar_color, "█".repeat(bar_len));
      let status_with_bar = format!("{}  {}", status, bar);

      // Remember current status and repaint it on the bottom line.
      {
        if let Ok(mut st) = status_line.lock() {
          *st = status.clone();
        }
      }

      // one-line repaint (stdout)
      let _g = print_lock.lock().unwrap();
      clear_line_cr();
      print!("{}", status_with_bar);
      let _ = std::io::stdout().flush();

      i = i.wrapping_add(1);
      thread::sleep(Duration::from_millis(100));
    }

    // clear + show cursor
    let _g = print_lock.lock().unwrap();
    clear_line_cr();
    print!("\x1b[?25h");
    let _ = std::io::stdout().flush();
  })
}

/// Print a content line while a spinner/status line is being repainted.
///
/// This keeps the emojis/spinner ONLY on the latest bottom line:
/// - clear the status line
/// - print the message line (with newline)
/// - redraw the current status line (without newline)
pub fn ui_println(print_lock: &Arc<Mutex<()>>, status_line: &Arc<Mutex<String>>, s: &str) {
  let _g = print_lock.lock().unwrap();
  clear_line_cr();
  println!("{s}");
  clear_line_cr();
  if let Ok(st) = status_line.lock() {
    print!("{}", *st);
  }
  let _ = std::io::stdout().flush();
}

// PRIVATE
// ------------------------------------------------------------------

fn clear_line_cr() {
  // Clear the current line and return to column 0.
  print!("\r\x1b[K");
}
