// ------------------------------------------------------------------
// ui.rs – real scroll, streaming chunks, bottom bar pinned
// ------------------------------------------------------------------

use crate::state::{GLOBAL_STATE, get_speed, get_voice};
use crossbeam_channel::Receiver;
use crossterm::{
  cursor::MoveTo,
  execute,
  style::{Print, ResetColor},
  terminal::{self, Clear, ClearType, ScrollUp},
};
use std::io::{self, Write};
use std::sync::{
  Arc, Mutex,
  atomic::{AtomicBool, Ordering},
};
use std::thread;
use std::time::Duration;

pub static STOP_STREAM: AtomicBool = AtomicBool::new(false);

// ANSI labels
pub const USER_LABEL: &str = "\x1b[47;30mUSER:\x1b[0m";
pub const ASSIST_LABEL: &str = "\x1b[48;5;22;37mASSISTANT:\x1b[0m";

const CHAR_DELAY_MS: u64 = 10;

#[derive(Clone)]
struct UiState {
  peak: f32,
  spinner_index: usize,
}

// ------------------------------------------------------------------
// Helper: compute viewport for scroll
// ------------------------------------------------------------------
fn viewport(buffer_len: usize, term_height: u16) -> (usize, usize) {
  let visible = term_height.saturating_sub(1) as usize; // terminal_height - 1
  let view_start = buffer_len.saturating_sub(visible);
  (view_start, visible)
}

// ------------------------------------------------------------------
// Spawn UI thread
// ------------------------------------------------------------------
pub fn spawn_ui_thread(
  ui: crate::state::UiState,
  stop_all_rx: Receiver<()>,
  status_line: Arc<Mutex<String>>,
  peak: Arc<Mutex<f32>>,
  ui_rx: Receiver<String>,
) -> thread::JoinHandle<()> {
  thread::spawn(move || {
    let mut out = io::stdout();
    let spinner = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
    let mut ui_state = UiState {
      peak: 0.0,
      spinner_index: 0,
    };
    let mut bottom_bar = String::new();
    let mut buffer: Vec<String> = Vec::new();

    loop {
      // Process UI messages
      while let Ok(msg) = ui_rx.try_recv() {
        let mut parts = msg.splitn(2, '|');
        let msg_type = parts.next().unwrap_or("");

        match msg_type {
          "line" => {
            let msg_str = parts.next().unwrap_or(msg.as_str());
            handle_line_message(
              &mut out,
              msg_str,
              &mut buffer,
              &mut ui_state,
              &peak,
              &spinner,
              &status_line,
              &ui,
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
              &peak,
              &spinner,
              &status_line,
              &ui,
              &mut bottom_bar,
            );
          }
          "stop_ui" => {
            let msg_str = "🛑 USER interrupted";
            // No need to modify parts iterator
            // let msg_str = parts.next().unwrap_or(msg.as_str());
            // Mark streaming to stop immediately
            STOP_STREAM.store(true, Ordering::Relaxed);

            handle_line_message(
              &mut out,
              msg_str,
              &mut buffer,
              &mut ui_state,
              &peak,
              &spinner,
              &status_line,
              &ui,
              &mut bottom_bar,
            );

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
      bottom_bar = render_bottom_bar(
        &mut out,
        &ui_state,
        &spinner,
        &status_line,
        &ui,
        &bottom_bar,
        term_height - 1,
      );

      thread::sleep(Duration::from_millis(10));
    }
  })
}

fn handle_line_message<W: Write>(
  out: &mut W,
  msg_str: &str,
  buffer: &mut Vec<String>,
  ui_state: &mut UiState,
  peak: &Arc<Mutex<f32>>,
  spinner: &[&str],
  status_line: &Arc<Mutex<String>>,
  ui: &crate::state::UiState,
  bottom_bar: &mut String,
) {
  // Start a fresh line for line| messages
  buffer.push(String::new());

  // Stream the chunk on that new line
  stream_chunk(
    out,
    msg_str,
    buffer,
    ui_state,
    peak,
    spinner,
    status_line,
    ui,
    bottom_bar,
  );

  // After message, push another empty line so next content starts fresh
  buffer.push(String::new());

  // Update viewport and clear last line for display
  let (_view_start, visible) = viewport(buffer.len(), terminal::size().unwrap_or((80, 24)).1);

  execute!(
    out,
    MoveTo(0, (std::cmp::min(buffer.len(), visible)) as u16 - 1),
    Clear(ClearType::CurrentLine)
  )
  .unwrap();

  // Redraw bottom bar
  let (_cols, term_height) = terminal::size().unwrap_or((80, 24));
  *bottom_bar = render_bottom_bar(
    out,
    ui_state,
    spinner,
    status_line,
    ui,
    bottom_bar,
    term_height - 1,
  );
}

fn handle_stream_message<W: Write>(
  out: &mut W,
  msg_str: &str,
  buffer: &mut Vec<String>,
  ui_state: &mut UiState,
  peak: &Arc<Mutex<f32>>,
  spinner: &[&str],
  status_line: &Arc<Mutex<String>>,
  ui: &crate::state::UiState,
  bottom_bar: &mut String,
) {
  // Just call stream_chunk directly; it handles wrapping / \n internally
  stream_chunk(
    out,
    msg_str,
    buffer,
    ui_state,
    peak,
    spinner,
    status_line,
    ui,
    bottom_bar,
  );
}

// ------------------------------------------------------------------
// Stream a chunk char-by-char, commit line at '\n' or wrap
// ------------------------------------------------------------------
fn stream_chunk<W: Write>(
  out: &mut W,
  chunk: &str,
  buffer: &mut Vec<String>,
  ui_state: &mut UiState,
  peak: &Arc<Mutex<f32>>,
  spinner: &[&str],
  status_line: &Arc<Mutex<String>>,
  ui: &crate::state::UiState,
  bottom_bar: &mut String,
) {
  let (cols, term_height) = terminal::size().unwrap_or((80, 24));
  let max_width = cols as usize;

  if buffer.is_empty() {
    buffer.push(String::new());
  }

  for ch in chunk.chars() {
    if STOP_STREAM.load(Ordering::Relaxed) {
      STOP_STREAM.store(false, Ordering::Relaxed);
      return;
    }

    let term_height_usize = term_height as usize;
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

      *bottom_bar = render_bottom_bar(
        out,
        ui_state,
        spinner,
        status_line,
        ui,
        bottom_bar,
        term_height - 1,
      );
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

// ------------------------------------------------------------------
// Render bottom bar
// Returns updated bottom_bar string
// ------------------------------------------------------------------
fn render_bottom_bar<W: Write>(
  out: &mut W,
  ui_state: &UiState,
  spinner: &[&str],
  status_line: &Arc<Mutex<String>>,
  ui: &crate::state::UiState,
  bottom_bar: &str,
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
  let mut bar_len = ((ui_state.peak * (max_bar_len as f32)).round() as usize).min(max_bar_len);
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

// ------------------------------------------------------------------
// Helper to compute visible length ignoring ANSI sequences
// ------------------------------------------------------------------
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
