// ------------------------------------------------------------------
//  UI
// ------------------------------------------------------------------

use crate::state::{get_speed, get_voice, GLOBAL_STATE};
use crossbeam_channel::Receiver;
use crossterm::{
  cursor::{Hide, MoveTo},
  execute,
  style::{Print, ResetColor},
  terminal::{self, Clear, ClearType, ScrollUp},
};
use std::io::{self, Write};
use std::sync::{
  atomic::{AtomicBool, Ordering},
  Arc, Mutex,
};
use std::thread;
use std::time::Duration;

// API
// ------------------------------------------------------------------

pub static STOP_STREAM: AtomicBool = AtomicBool::new(false);

// ANSI labels
pub const USER_LABEL: &str = "\x1b[47;30mUSER:\x1b[0m";
pub const ASSIST_LABEL: &str = "\x1b[48;5;22;37mASSISTANT:\x1b[0m";

const CHAR_DELAY_MS: u64 = 4;

pub fn spawn_ui_thread(
  ui_state: crate::state::UiState,
  status_line: Arc<Mutex<String>>,
  rx_ui: Receiver<String>,
) -> thread::JoinHandle<()> {
  thread::spawn(move || {
    // Make ui_state mutable for interior mutability
    let mut ui_state = ui_state;
    let mut out = io::stdout();
    execute!(out, Hide).unwrap();
    let spinner = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
    let mut bottom_bar = String::new();
    let mut buffer: Vec<String> = Vec::new();
    let mut first_line_done = false;

    crossterm::execute!(
      std::io::stdout(),
      crossterm::terminal::Clear(ClearType::All),
      MoveTo(0, 0)
    )
    .unwrap();

    let banner = r#"
      █████╗ ██╗      ███╗   ███╗ █████╗ ████████╗███████╗
     ██╔══██╗██║      ████╗ ████║██╔══██╗╚══██╔══╝██╔════╝
     ███████║██║█████╗██╔████╔██║███████║   ██║   █████╗  
     ██╔══██║██║╚════╝██║╚██╔╝██║██╔══██║   ██║   ██╔══╝  
     ██║  ██║██║      ██║ ╚═╝ ██║██║  ██║   ██║   ███████╗
     ╚═╝  ╚═╝╚═╝      ╚═╝     ╚═╝╚═╝  ╚═╝   ╚═╝   ╚══════╝
     "#;
    handle_line_message(
      &mut out,
      banner,
      &mut buffer,
      &mut ui_state,
      &spinner,
      &status_line,
      &mut bottom_bar,
    );

    loop {
      // Process UI messages
      while let Ok(msg) = rx_ui.try_recv() {
        let mut parts = msg.splitn(2, '|');
        let msg_type = parts.next().unwrap_or("");

        match msg_type {
          "line" => {
            let msg_str = parts.next().unwrap_or(msg.as_str());
            if !first_line_done {
              handle_line_message(
                &mut out,
                "",
                &mut buffer,
                &mut ui_state,
                &spinner,
                &status_line,
                &mut bottom_bar,
              );
              thread::sleep(Duration::from_millis(10));
              first_line_done = true;
            }
            handle_line_message(
              &mut out,
              msg_str,
              &mut buffer,
              &mut ui_state,
              &spinner,
              &status_line,
              &mut bottom_bar,
            );
          }
          "stream" => {
            let msg_str = parts.next().unwrap_or(msg.as_str());
            handle_stream_message(
              &mut out,
              msg_str,
              &mut buffer,
              &mut ui_state,
              &spinner,
              &status_line,
              &mut bottom_bar,
            );
          }
          "stop_ui" => {
            let msg_str = "🛑 USER interrupted";
            handle_line_message(
              &mut out,
              msg_str,
              &mut buffer,
              &mut ui_state,
              &spinner,
              &status_line,
              &mut bottom_bar,
            );

            // Mark streaming to stop immediately after printing
            STOP_STREAM.store(true, Ordering::Relaxed);
            // Exit the UI thread loop
            break;
          }
          _ => {}
        }
      }

      // Spinner update
      ui_state.spinner_index = (ui_state.spinner_index + 1) % spinner.len();

      // Render bottom bar
      let (_cols, term_height) = terminal::size().unwrap_or((80, 24));
      bottom_bar = render_bottom_bar(&mut out, &ui_state, &spinner, &status_line, term_height - 1);

      thread::sleep(Duration::from_millis(10));
    }
  })
}


// PRIVATE
// ------------------------------------------------------------------

// computes viewport for scroll
fn viewport(buffer_len: usize, term_height: u16) -> (usize, usize) {
  let visible = term_height.saturating_sub(1) as usize;
  let view_start = buffer_len.saturating_sub(visible);
  (view_start, visible)
}

// handles a complete line print
fn handle_line_message<W: Write>(
  out: &mut W,
  msg_str: &str,
  buffer: &mut Vec<String>,
  ui_state: &mut crate::state::UiState,
  spinner: &[&str],
  status_line: &Arc<Mutex<String>>,
  bottom_bar: &mut String,
) {
  let (cols, term_height) = terminal::size().unwrap_or((80, 24));
  let max_width = cols as usize;

  if buffer.is_empty() {
    buffer.push(String::new());
  }
  // If the last line is not full width, start a new line
  if buffer.last().unwrap().len() != max_width {
    buffer.push(String::new());
  }

  for ch in msg_str.chars() {
    let is_newline_or_wrap = ch == '\n' || get_visible_len_for(buffer.last().unwrap()) >= max_width;

    if is_newline_or_wrap {
      buffer.push(String::new());

      let (_view_start, visible) = viewport(buffer.len(), term_height);

      if buffer.len() > visible {
        execute!(out, ScrollUp(1)).unwrap();
      }

      execute!(
        out,
        MoveTo(0, (std::cmp::min(buffer.len(), visible)) as u16 - 1),
        Clear(ClearType::CurrentLine)
      )
      .unwrap();

      *bottom_bar = render_bottom_bar(out, ui_state, spinner, status_line, term_height - 1);
    } else {
      buffer.last_mut().unwrap().push(ch);

      let (_view_start, visible) = viewport(buffer.len(), term_height);
      let y_disp = if buffer.len() > visible {
        visible - 1
      } else {
        buffer.len() - 1
      };

      execute!(
        out,
        MoveTo(0, y_disp as u16),
        Clear(ClearType::CurrentLine),
        Print(buffer.last().unwrap())
      )
      .unwrap();
    }
  }

  // After message, push another empty line so next content starts fresh
  buffer.push(String::new());

  // Update viewport and clear last line for display
  let (_view_start, visible) = viewport(buffer.len(), terminal::size().unwrap_or((80, 24)).1);

  if buffer.len() > visible {
    execute!(out, ScrollUp(1)).unwrap();
  }

  execute!(
    out,
    MoveTo(0, (std::cmp::min(buffer.len(), visible)) as u16 - 1),
    Clear(ClearType::CurrentLine)
  )
  .unwrap();

  // Redraw bottom bar
  let (_cols, term_height) = terminal::size().unwrap_or((80, 24));
  *bottom_bar = render_bottom_bar(out, ui_state, spinner, status_line, term_height - 1);
}

fn handle_stream_message<W: Write>(
  out: &mut W,
  msg_str: &str,
  buffer: &mut Vec<String>,
  ui_state: &mut crate::state::UiState,
  spinner: &[&str],
  status_line: &Arc<Mutex<String>>,
  bottom_bar: &mut String,
) {
  stream_chunk(
    out,
    msg_str,
    buffer,
    ui_state,
    spinner,
    status_line,
    bottom_bar,
  );
}

// Stream a chunk char-by-char, commit line at '\n' or wrap
fn stream_chunk<W: Write>(
  out: &mut W,
  chunk: &str,
  buffer: &mut Vec<String>,
  ui_state: &mut crate::state::UiState,
  spinner: &[&str],
  status_line: &Arc<Mutex<String>>,
  bottom_bar: &mut String,
) {
  let (cols, term_height) = terminal::size().unwrap_or((80, 24));
  let max_width = cols as usize;

  if buffer.is_empty() {
    buffer.push(String::new());
    // If the last line is not full width, start a new line
    if buffer.last().unwrap().len() != max_width {
      buffer.push(String::new());
    }
  }

  for ch in chunk.chars() {
    if STOP_STREAM.load(Ordering::Relaxed) {
      STOP_STREAM.store(false, Ordering::Relaxed);
      return;
    }

    let is_newline_or_wrap = ch == '\n' || get_visible_len_for(buffer.last().unwrap()) >= max_width;

    if is_newline_or_wrap {
      buffer.push(String::new());

      let (_view_start, visible) = viewport(buffer.len(), term_height);

      if buffer.len() > visible {
        execute!(out, ScrollUp(1)).unwrap();
      }

      execute!(
        out,
        MoveTo(0, (std::cmp::min(buffer.len(), visible)) as u16 - 1),
        Clear(ClearType::CurrentLine)
      )
      .unwrap();

      *bottom_bar = render_bottom_bar(out, ui_state, spinner, status_line, term_height - 1);
    } else {
      buffer.last_mut().unwrap().push(ch);

      let (_view_start, visible) = viewport(buffer.len(), term_height);
      let y_disp = if buffer.len() > visible {
        visible - 1
      } else {
        buffer.len() - 1
      };

      execute!(
        out,
        MoveTo(0, y_disp as u16),
        Clear(ClearType::CurrentLine),
        Print(buffer.last().unwrap())
      )
      .unwrap();
    }

    thread::sleep(Duration::from_millis(CHAR_DELAY_MS));
  }
}

fn render_bottom_bar<W: Write>(
  out: &mut W,
  ui_state: &crate::state::UiState,
  spinner: &[&str],
  status_line: &Arc<Mutex<String>>,
  y: u16,
) -> String {
  let state = GLOBAL_STATE.get().expect("AppState not initialized");

  let speak = state.ui.agent_speaking.load(Ordering::Relaxed);
  let play = state.playback.playback_active.load(Ordering::Relaxed);
  let recording_paused = state.recording_paused.load(Ordering::Relaxed);
  let conversation_paused = state.conversation_paused.load(Ordering::Relaxed);

  let status = if recording_paused {
    "⏸️".to_string()
  } else if play {
    format!("🔊 {}", spinner[ui_state.spinner_index])
  } else if speak {
    format!("🎤 {}", spinner[ui_state.spinner_index])
  } else {
    format!("🎤 {}", spinner[ui_state.spinner_index])
  };

  let speed_str = format!("[{:.1}x]", get_speed());
  let voice_str = format!("({})", get_voice());

  let recording_paused_str = if recording_paused {
    "\x1b[43m\x1b[30m  paused  \x1b[0m"
  } else {
    "\x1b[41m\x1b[37m listening \x1b[0m"
  };

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
    },
  );

  let combined_status = format!("{} {} ", voice_str, internal_status);

  let cols = crossterm::terminal::size().unwrap_or((80, 24)).0 as usize;

  let available = cols.saturating_sub(
    get_visible_len_for(&status)
      + 2
      + get_visible_len_for(&combined_status)
      + 1
      + get_visible_len_for(&speed_str)
      + get_visible_len_for(&recording_paused_str),
  );

  let max_bar_len = if available > 40 { 40 } else { available };
  let peak_val = *ui_state.peak.lock().unwrap();
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
      + get_visible_len_for(&recording_paused_str),
  );

  let status_without_speed = format!("{} {}{}", status, bar, " ".repeat(spaces));
  let full_bar = format!(
    "{}{} {}{}",
    status_without_speed, speed_str, combined_status, recording_paused_str
  );

  if let Ok(mut st) = status_line.lock() {
    *st = full_bar.clone();
  }

  execute!(
    out,
    MoveTo(0, y),
    Clear(ClearType::CurrentLine),
    Print(&full_bar),
    ResetColor
  )
  .unwrap();

  out.flush().unwrap();
  full_bar
}

// computes visible length ignoring ANSI sequences
fn get_visible_len_for(s: &str) -> usize {
  let mut len = 0usize;
  let mut chars = s.chars();
  while let Some(c) = chars.next() {
    if c == '\x1b' {
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
