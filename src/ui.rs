// ------------------------------------------------------------------
//  UI
// ------------------------------------------------------------------

use crate::state::{get_speed, get_voice, GLOBAL_STATE};
use crossbeam_channel::Receiver;
use crossterm::{
  cursor::{position, Hide, MoveTo},
  execute,
  style::{Print, ResetColor},
  terminal::{self, Clear, ClearType},
};
use std::io::{self, Write};
use std::sync::{atomic::Ordering, Arc, Mutex};
use std::thread;
use std::time::Duration;

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
  // separate thread for bottom bar update + render
  thread::spawn(move || {
    let mut out = io::stdout();
    let spinner = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
    let mut i = 0usize;

    let ui_for_bg = ui.clone();
    let status_line_for_bg = status_line.clone();
    let peak_for_bg = peak.clone();

    let mut out_for_closure = io::stdout();
    let mut ui_rx_for_closure = ui_rx.clone();
    let mut stop_all_rx_for_closure = stop_all_rx.clone();
    thread::spawn(move || {
      let mut i_for_bg = i;
      loop {
        i_for_bg = (i_for_bg + 1) % spinner.len();
        let full_bar = update_bottom_bar(
          &ui_for_bg,
          &status_line_for_bg,
          &peak_for_bg,
          &spinner,
          &mut i_for_bg,
        );
        print_bottom_bar(&mut out_for_closure, &full_bar).unwrap();

        if stop_all_rx_for_closure.try_recv().is_ok() {
          while let Ok(_) = ui_rx_for_closure.try_recv() {}
        }
        thread::sleep(Duration::from_millis(35));
      }
    });

    // hide cursor
    execute!(out, Hide).unwrap();

    // buffer for top region
    let mut top_lines: Vec<String> = Vec::new();

    loop {
      if stop_all_rx.try_recv().is_ok() {
        // drain ui_rx messages before exiting
        while let Ok(_) = ui_rx.try_recv() {}
        break;
      }

      let state = GLOBAL_STATE.get().expect("AppState not initialized");
      let conversation_paused = state.conversation_paused.load(Ordering::Relaxed);

      let (cols_raw, terminal_height) = terminal::size().unwrap_or((80, 24));
      let cols = cols_raw as usize;
      let mut full_bar = String::new();

      while let Ok(msg) = ui_rx.try_recv() {
        if stop_all_rx.try_recv().is_ok() {
          // drain ui_rx messages before exiting
          while let Ok(_) = ui_rx.try_recv() {}
          break;
}

        let mut parts = msg.splitn(2, '|');
        let msg_type = parts.next().unwrap_or("");
        let msg_str = parts.next().unwrap_or(msg.as_str());

        if !conversation_paused {
          match msg_type {
            "line" => {
              let (label, body) = if msg_str.starts_with(USER_LABEL) {
                (
                  USER_LABEL,
                  msg_str.strip_prefix(USER_LABEL).unwrap_or("").trim(),
                )
              } else if msg_str.starts_with(ASSIST_LABEL) {
                (
                  ASSIST_LABEL,
                  msg_str.strip_prefix(ASSIST_LABEL).unwrap_or("").trim(),
                )
              } else {
                ("", msg_str)
              };

              if !label.is_empty() {
                print_line(&mut top_lines, label);
              }

              if !body.is_empty() {
                print_inline_chunk(&mut out, &mut top_lines, body, terminal_height - 1, cols);
              }
            }
            "stream" => {
              print_inline_chunk(&mut out, &mut top_lines, msg_str, terminal_height - 1, cols);
            }
            _ => {}
          }
        }
        if stop_all_rx.try_recv().is_ok() {
          break;
        }
      }
    }
  })
}

// PRIVATE
// ------------------------------------------------------------------

// delay per character for smooth typing
const STREAM_DELAY_MS: u64 = 10;

fn print_line(buffer: &mut Vec<String>, line: &str) {
  buffer.push(line.to_string());
  buffer.push(String::new()); // force a fresh line after
}

fn print_inline_chunk<W: Write>(
  out: &mut W,
  buffer: &mut Vec<String>,
  chunk: &str,
  terminal_height: u16,
  cols: usize,
) {
  let mut chars_since_redraw = 0;

  // Ensure there is at least one line
  if buffer.is_empty() {
    buffer.push(String::new());
  }

  for ch in chunk.chars() {
    // Wrap line if exceeds terminal width
    if get_visible_len_for(buffer.last().unwrap()) >= cols {
      buffer.push(String::new());
    }

    // Append character or create a new line for breaks
    if ch == '\n' || ch == '.' {
      buffer.push(String::new());
    } else {
      // Re-borrow last line only when appending
      if let Some(last_line) = buffer.last_mut() {
        last_line.push(ch);
      }
    }

    // Redraw in batches
    chars_since_redraw += 1;
    if chars_since_redraw >= 5 {
      redraw_top_region(out, buffer, terminal_height - 1);
      chars_since_redraw = 0;
    }

    thread::sleep(Duration::from_millis(STREAM_DELAY_MS));
  }

  if chars_since_redraw > 0 {
    redraw_top_region(out, buffer, terminal_height - 1);
  }
}

fn redraw_top_region<W: Write>(out: &mut W, buffer: &[String], max_height: u16) {
  let draw_height = max_height as usize;

  // Determine the start line so the bottom of the buffer is visible
  let start = buffer.len().saturating_sub(draw_height);

  for (i, line) in buffer[start..].iter().enumerate() {
    execute!(
      out,
      MoveTo(0, i as u16),
      Clear(ClearType::CurrentLine),
      Print(line)
    )
    .unwrap();
  }

  // Fill remaining lines if buffer is shorter than draw_height
  for i in buffer[start..].len()..draw_height {
    execute!(out, MoveTo(0, i as u16), Clear(ClearType::CurrentLine)).unwrap();
  }

  out.flush().unwrap();
}

fn print_bottom_bar<W: Write>(out: &mut W, status: &str) -> std::io::Result<()> {
  let (prev_x, prev_y) = position().unwrap_or((0, 0));
  let (_, terminal_height) = terminal::size()?;
  let last_y = terminal_height.saturating_sub(1);

  execute!(out, MoveTo(0, last_y), Clear(ClearType::CurrentLine))?;
  execute!(
    out,
    MoveTo(0, last_y),
    ResetColor,
    Print(status),
    ResetColor
  )?;
  execute!(out, MoveTo(prev_x, prev_y))?;
  out.flush()?;
  Ok(())
}

fn get_visible_len_for(s: &str) -> usize {
  let mut len = 0usize;
  let mut chars = s.chars();
  while let Some(c) = chars.next() {
    if c == '\x1b' {
      // skip ANSI sequences
      while let Some(next) = chars.next() {
        if next == 'm' {
          break;
        }
      }
    } else {
      let double = matches!(c, '🤔' | '🎤' | '🔊');
      len += if double { 2 } else { 1 };
    }
  }
  len
}

fn update_bottom_bar(
  ui: &crate::state::UiState,
  status_line: &Arc<Mutex<String>>,
  peak: &Arc<Mutex<f32>>,
  spinner: &[&str],
  i: &mut usize,
) -> String {
  let state = GLOBAL_STATE.get().expect("AppState not initialized");
  let speak = state.ui.agent_speaking.load(Ordering::Relaxed);
  let think = ui.thinking.load(Ordering::Relaxed);
  let play = state.ui.playing.load(Ordering::Relaxed);
  let recording_paused = state.recording_paused.load(Ordering::Relaxed);
  let conversation_paused = state.conversation_paused.load(Ordering::Relaxed);
  let status = if recording_paused {
    "⏸️".to_string()
  } else if think {
    format!("🤔 {}", spinner[*i % spinner.len()])
  } else if speak {
    format!("🎤 {}", spinner[*i % spinner.len()])
  } else if play {
    format!("🔊 {}", spinner[*i % spinner.len()])
  } else {
    format!("🎤 {}", spinner[*i % spinner.len()])
  };
  let (cols_raw, _x) = terminal::size().unwrap_or((80, 24));
  let cols = cols_raw as usize;
  let peak_val = match peak.lock() {
    Ok(v) => *v,
    Err(_) => 0.0,
  };
  let speed_str = format!("[{:.1}x]", get_speed());
  let voice_str = format!("({})", get_voice());
  let recording_paused_str = if recording_paused {
    "\x1b[43m\x1b[30m  paused  \x1b[0m"
  } else {
    "\x1b[41m\x1b[37m listening \x1b[0m"
  };
  let recording_paused_vis_len = get_visible_len_for(recording_paused_str);
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
  let available = cols.saturating_sub(
    get_visible_len_for(&status)
      + 2
      + get_visible_len_for(&combined_status)
      + 1
      + get_visible_len_for(&speed_str)
      + recording_paused_vis_len,
  );
  let max_bar_len = if available > 40 { 40 } else { available };
  let mut bar_len = ((peak_val * (max_bar_len as f32)).round() as usize).min(max_bar_len);
  if recording_paused {
    bar_len = 0;
  }
  let bar_color = if recording_paused {
    "\x1b[37m"
  } else if speak {
    "\x1b[31m"
  } else {
    "\x1b[37m"
  };
  let bar = format!("{}{}\x1b[0m", bar_color, "█".repeat(bar_len));
  let spaces = cols.saturating_sub(
    get_visible_len_for(&status)
      + 2
      + bar_len
      + get_visible_len_for(&speed_str)
      + get_visible_len_for(&combined_status)
      + recording_paused_vis_len,
  );
  let status_without_speed = format!("{} {}{}", status, bar, " ".repeat(spaces));
  let full_bar = format!(
    "{}{} {}{}",
    status_without_speed, speed_str, combined_status, recording_paused_str
  );
  if let Ok(mut st) = status_line.lock() {
    *st = full_bar.clone();
  }
  full_bar
}
