// ------------------------------------------------------------------
//  UI (single renderer thread)
// ------------------------------------------------------------------

use crate::state::{get_speed, GLOBAL_STATE};

use crossbeam_channel::Receiver;
use crossterm::terminal;
use std::io::Write;
use std::sync::{atomic::Ordering, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

// API
// ------------------------------------------------------------------

// ANSI label styling
pub const USER_LABEL: &str = "\x1b[47;30mUSER:\x1b[0m"; // white bg, black text
pub const ASSIST_LABEL: &str = "\x1b[48;5;22;37mASSISTANT:\x1b[0m"; // dark green bg, white text

fn visible_len(s: &str) -> usize {
  let mut in_escape = false;
  let mut len = 0;
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
    len += 1;
  }
  len
}

pub fn spawn_ui_thread(
  ui: crate::state::UiState,
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

    let mut last_cols = 0usize;
    let mut last_change = Instant::now();
    loop {
      if stop_all_rx.try_recv().is_ok() {
        break;
      }

      let state = GLOBAL_STATE.get().expect("AppState not initialized");
      let speak = state.ui.speaking.load(Ordering::Relaxed);
      let think = ui.thinking.load(Ordering::Relaxed);
      let play = state.ui.playing.load(Ordering::Relaxed);

      // Priority: playing > thinking > idle/speaking.
      // This ensures we show the speaking emoji any time audio isn't playing,
      // even if a stale thinking flag is briefly set.
      let paused = state.recording_paused.load(Ordering::Relaxed);
      let status = if paused {
        format!("⏸️  {}", spinner[i % spinner.len()])
      } else if think {
        format!("🤔. {}", spinner[i % spinner.len()])
      } else if speak {
        format!("🎙  {}", spinner[i % spinner.len()])
      } else if play {
        format!("🔊  {}", spinner[i % spinner.len()])
      } else {
        format!("🎙  {}", spinner[i % spinner.len()])
      };
      // draw amplitude bar
      let (cols_raw, _) = terminal::size().unwrap_or((80, 24));
      let cols = cols_raw as usize;
      if cols != last_cols {
        last_cols = cols;
        last_change = Instant::now();
      }
      // track terminal width to reduce unnecessary redraws

      let resizing = last_change.elapsed().as_millis() < 1000;
      if resizing {
        thread::sleep(Duration::from_millis(100));
        continue;
      }

      let peak_val = match peak.lock() {
        Ok(v) => *v,
        Err(_) => 0.0,
      };
      // cols already defined earlier
      let speed_val = get_speed();
      let speed_str = format!("[{:.1}x]", speed_val);
      let paused_str = if state.recording_paused.load(Ordering::Relaxed) {
        "\x1b[41m\x1b[37m[paused]\x1b[0m"
      } else {
        "\x1b[43m\x1b[30m[listening]\x1b[0m"
      };
      let paused_vis_len = visible_len(paused_str);
      let max_bar_len = if cols > status.len() + 2 + speed_str.len() {
        cols - status.len() - 2 - speed_str.len()
      } else {
        0
      };
      let bar_len = ((peak_val * (max_bar_len as f32)).round() as usize).min(max_bar_len);

      let bar_color = if paused {
        "\x1b[37m"
      } else if state.ui.speaking.load(Ordering::Relaxed) {
        "\x1b[31m"
      } else {
        "\x1b[37m"
      };
      let bar_len = if paused { 1 } else { bar_len };
      let bar = format!("{}{}\x1b[0m", bar_color, "█".repeat(bar_len));

      let status_vis_len = visible_len(&status);
      let _status_len = status_vis_len + 2 + bar_len;
      let spaces = if cols > status.len() + 2 + bar_len + speed_str.len() + paused_str.len() {
        cols - status_vis_len - 2 - bar_len - speed_str.len() - paused_vis_len - 1
      } else {
        0
      };
      let status_without_speed =
        format!("{}{}{}{}", status, "  ".repeat(1), bar, " ".repeat(spaces));
      let status_with_bar = format!("{}{}{}", status_without_speed, speed_str, paused_str);
      // Remember current status and repaint it on the bottom line.
      {
        if let Ok(mut st) = status_line.lock() {
          *st = status_with_bar.clone();
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

// Clear the previous line (used when interrupting).

// PRIVATE
// ------------------------------------------------------------------

fn clear_line_cr() {
  // Clear the current line and return to column 0.
  print!("\r\x1b[K");
}
