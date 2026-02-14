// ------------------------------------------------------------------
//  UI (single renderer thread)
// ------------------------------------------------------------------

use crate::state::{GLOBAL_STATE, get_speed, get_voice};
use crossbeam_channel::Receiver;
use crossterm::{
  cursor::{Hide, MoveTo},
  execute,
  style::{Print, ResetColor},
  terminal,
  terminal::{Clear, ClearType},
};
use std::io::{self, Write};
use std::sync::{Arc, Mutex, atomic::Ordering};

pub fn print_conversation_line(
  print_lock: &Arc<Mutex<()>>,
  status_line: &Arc<Mutex<String>>,
  s: &str,
) {
  let state = GLOBAL_STATE.get().expect("AppState not initialized");
  if !state.conversation_paused.load(Ordering::Relaxed) {
    crate::ui::ui_println(print_lock, status_line, s);
  }
}
use std::thread;
use std::time::{Duration, Instant};

// API
// ------------------------------------------------------------------

// ANSI label styling
pub const USER_LABEL: &str = "\x1b[47;30mUSER:\x1b[0m"; // white bg, black text
pub const ASSIST_LABEL: &str = "\x1b[48;5;22;37mASSISTANT:\x1b[0m"; // dark green bg, white text

pub fn spawn_ui_thread(
  ui: crate::state::UiState,
  stop_all_rx: Receiver<()>,
  status_line: Arc<Mutex<String>>,
  peak: Arc<Mutex<f32>>,
  ui_rx: Receiver<String>,
) -> thread::JoinHandle<()> {
  thread::spawn(move || {
    let spinner = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
    let mut i = 0usize;

    // Hide cursor and enable raw mode
    let mut out = io::stdout();
    execute!(out, Hide).unwrap();

    let mut last_cols = 0usize;
    let mut last_change = Instant::now();
    loop {
      if stop_all_rx.try_recv().is_ok() {
        break;
      }

      let state = GLOBAL_STATE.get().expect("AppState not initialized");
      let speak = state.ui.agent_speaking.load(Ordering::Relaxed);
      let think = ui.thinking.load(Ordering::Relaxed);
      let play = state.ui.playing.load(Ordering::Relaxed);
      let recording_paused = state.recording_paused.load(Ordering::Relaxed);
      let conversation_paused = state.conversation_paused.load(Ordering::Relaxed);

      let status = if recording_paused {
        format!("⏸️")
      } else if think {
        format!("🤔 {}", spinner[i % spinner.len()])
      } else if speak {
        format!("🎤 {}", spinner[i % spinner.len()])
      } else if play {
        format!("🔊 {}", spinner[i % spinner.len()])
      } else {
        format!("🎤 {}", spinner[i % spinner.len()])
      };

      let (cols_raw, _) = terminal::size().unwrap_or((80, 24));

      let cols = cols_raw as usize;
      if cols != last_cols {
        last_cols = cols;
        last_change = Instant::now();
      }

      let resizing = last_change.elapsed().as_millis() < 1000;
      if resizing {
        thread::sleep(Duration::from_millis(30));
        continue;
      }

      let peak_val = match peak.lock() {
        Ok(v) => *v,
        Err(_) => 0.0,
      };
      let speed_str = format!("[{:.1}x]", get_speed());
      let voice_str = format!("({})", get_voice());

      let recording_paused_str = if recording_paused {
        "\x1b[41m\x1b[37m  paused  \x1b[0m"
      } else {
        "\x1b[43m\x1b[30m listening \x1b[0m"
      };
      let recording_paused_vis_len = visible_len(recording_paused_str);

      // Internal status blocks
      let internal_status = format!(
        "{}{}{}{}",
        if recording_paused {
          "\x1b[47m█\x1b[0m"
        } else {
          "\x1b[100m█\x1b[0m"
        },
        if conversation_paused {
          "\x1b[47m█\x1b[0m"
        } else {
          "\x1b[100m█\x1b[0m"
        },
        if state.playback.paused.load(Ordering::Relaxed) {
          "\x1b[100m█\x1b[0m"
        } else {
          "\x1b[47m█\x1b[0m"
        },
        if state.playback.playback_active.load(Ordering::Relaxed) {
          "\x1b[47m█\x1b[0m"
        } else {
          "\x1b[100m█\x1b[0m"
        }
      );
      let combined_status = format!("{} {} ", voice_str, internal_status);

      // Use the actual visible width of the status for bar calculations
      let max_bar_len = if cols
        > visible_len(&status)
          + 2
          + visible_len(&combined_status)
          + 1
          + visible_len(&speed_str)
          + recording_paused_vis_len
      {
        BAR_WIDTH
      } else {
        let available = cols.saturating_sub(
          visible_len(&status)
            + 2
            + visible_len(&combined_status)
            + 1
            + visible_len(&speed_str)
            + recording_paused_vis_len,
        );
        let max_bar_len = if available > 10 { 10 } else { available };
        max_bar_len
      };

      let bar_len = ((peak_val * (max_bar_len as f32)).round() as usize).min(max_bar_len);
      let bar_color = if recording_paused {
        "\x1b[37m"
      } else if state.ui.agent_speaking.load(Ordering::Relaxed) {
        "\x1b[31m"
      } else {
        "\x1b[37m"
      };
      let bar_len = if recording_paused { 0 } else { bar_len };
      let bar = format!("{}{}\x1b[0m", bar_color, "█".repeat(bar_len));

      let _status_len = visible_len(&status) + 2 + bar_len;
      let spaces = if cols
        > visible_len(&status)
          + 2
          + bar_len
          + visible_len(&speed_str)
          + visible_len(&combined_status)
          + recording_paused_vis_len
      {
        cols
          - visible_len(&status)
          - 2
          - bar_len
          - visible_len(&speed_str)
          - visible_len(&combined_status)
          - recording_paused_vis_len
      } else {
        0
      };

      let status_without_speed = format!("{} {}{}", status, bar, " ".repeat(spaces));
      let status_with_bar = format!(
        "{}{} {}{}",
        status_without_speed, speed_str, combined_status, recording_paused_str
      );

      // Update shared status
      if let Ok(mut st) = status_line.lock() {
        *st = status_with_bar.clone();
      }
      // Draw status line using crossterm
      let _ = draw(&mut out, &status_with_bar);

      // Handle incoming conversation lines
      while let Ok(line) = ui_rx.try_recv() {
        let state = GLOBAL_STATE.get().expect("AppState not initialized");
        let print_lock = &state.print_lock;
        print_conversation_line(print_lock, &status_line, &line);
      }
      i = i.wrapping_add(1);
      thread::sleep(Duration::from_millis(50));
    }
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
  // do nothing further, status will be refreshed by UI thread
  let _ = std::io::stdout().flush();
}

// PRIVATE
// ------------------------------------------------------------------

const BAR_WIDTH: usize = 50;

/// Return the display width of a string.
fn visible_len(s: &str) -> usize {
  // Count display width excluding ANSI escape codes.
  // Approximate double‑width for common emojis used in status.
  let mut len = 0usize;
  let mut chars = s.chars();
  while let Some(c) = chars.next() {
    if c == '\x1b' {
      // Skip until 'm' which ends the escape sequence
      while let Some(next) = chars.next() {
        if next == 'm' {
          break;
        }
      }
    } else {
      // Heuristic: treat certain emojis as width 2
      if c == '\u{FE0F}' {
        // Variation selector, invisible
        continue;
      }
      let double = match c {
        '🤔' | '🎤' | '🔊' => true,

        _ => false,
      };
      len += if double { 2 } else { 1 };
    }
  }
  len
}

fn draw<W: Write>(out: &mut W, status: &str) -> std::io::Result<()> {
  let (_w, h) = terminal::size()?;
  let bottom_y = h.saturating_sub(1);

  // Clear only the bottom line
  execute!(out, MoveTo(0, bottom_y), Clear(ClearType::CurrentLine))?;

  // Print status with reverse attribute
  execute!(
    out,
    MoveTo(0, bottom_y),
    Clear(ClearType::CurrentLine),
    ResetColor,
    Print(status),
    ResetColor,
  )?;

  out.flush()?;
  Ok(())
}

fn clear_line_cr() {
  // Clear the current line and return to column 0.
  print!("\r\x1b[K");
}
