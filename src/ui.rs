// ------------------------------------------------------------------
//  UI
// ------------------------------------------------------------------

use crate::state::{GLOBAL_STATE, get_speed, get_voice};
use crate::util::get_flag;
use crossbeam_channel::Receiver;
use crossterm::{
  cursor::{Hide, MoveTo},
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

// API
// ------------------------------------------------------------------

pub static STOP_STREAM: AtomicBool = AtomicBool::new(false);

// ANSI labels
pub const USER_LABEL: &str = "\x1b[47;30mUSER:\x1b[0m";
pub const ASSIST_LABEL: &str = "\x1b[48;5;22;37mASSISTANT:\x1b[0m";

pub fn get_banner() -> &'static str {
  r#"
 _    _ _______ _______ _______ _______ _______
  \  /     |    |  |  | |_____|    |    |______
   \/      |    |  |  | |     |    |    |______"#
}

const CHAR_DELAY_MS: u64 = 4;

pub fn spawn_ui_thread(
  ui_state: crate::state::UiState,
  status_line: Arc<Mutex<String>>,
  rx_ui: Receiver<String>,
) -> thread::JoinHandle<()> {
  thread::spawn(move || {
    let mut ui_state = ui_state;
    let mut out = io::stdout();
    execute!(out, Hide).unwrap();

    let spinner = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
    let mut bottom_bar = String::new();
    let mut buffer: Vec<String> = Vec::new();
    let mut last_term_size = terminal::size().unwrap_or((80, 24));
    let mut pending_stream: Vec<String> = Vec::new();
    let mut modal_visible = false;

    crossterm::execute!(
      std::io::stdout(),
      crossterm::terminal::Clear(ClearType::All),
      MoveTo(0, 0)
    )
    .unwrap();

    let banner = get_banner();
    handle_line_message(
      &mut out,
      banner,
      &mut buffer,
      &mut ui_state,
      &spinner,
      &status_line,
      &mut bottom_bar,
    );

    let mut waiting_for_first_line = true;
    let mut skip_next_bottom_bar = false;

    loop {
      while let Ok(msg) = rx_ui.try_recv() {
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
              &spinner,
              &status_line,
              &mut bottom_bar,
            );

            for chunk in pending_stream.drain(..) {
              handle_stream_message(
                &mut out,
                &chunk,
                &mut buffer,
                &mut ui_state,
                &spinner,
                &status_line,
                &mut bottom_bar,
              );
            }

            waiting_for_first_line = false;
          }

          "stream" => {
            let msg_str = parts.next().unwrap();

            if waiting_for_first_line {
              pending_stream.push(msg_str.to_string());
              continue;
            }

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
            STOP_STREAM.store(true, Ordering::Relaxed);

            pending_stream.clear();

            handle_line_message(
              &mut out,
              "\n\n🛑 USER interrupted",
              &mut buffer,
              &mut ui_state,
              &spinner,
              &status_line,
              &mut bottom_bar,
            );
            skip_next_bottom_bar = true;
          }

          "modal_show" => {
            modal_visible = true;
            render_debate_modal(&mut out, &mut buffer);
          }

          "modal_hide" => {
            modal_visible = false;
            // Redraw the screen
            execute!(out, Clear(ClearType::All), MoveTo(0, 0)).unwrap();
            redraw_buffer(&mut out, &buffer);
            let (_cols, term_height) = terminal::size().unwrap_or((80, 24));
            bottom_bar =
              render_bottom_bar(&mut out, &ui_state, &spinner, &status_line, term_height - 1);
          }

          "modal_update" => {
            if modal_visible {
              render_debate_modal(&mut out, &mut buffer);
            }
          }

          _ => {}
        }
      }

      // Detect terminal resize
      let (new_cols, new_term_height) = terminal::size().unwrap_or((80, 24));
      if new_term_height != last_term_size.1 || new_cols != last_term_size.0 {
        // Clear the whole screen
        execute!(out, Clear(ClearType::All), Print("\x1b[3J"), MoveTo(0, 0)).unwrap();
        out.flush().unwrap();
        last_term_size = (new_cols, new_term_height);
      }

      ui_state.spinner_index = (ui_state.spinner_index + 1) % spinner.len();

      let (_cols, term_height) = terminal::size().unwrap_or((80, 24));
      if !skip_next_bottom_bar {
        bottom_bar =
          render_bottom_bar(&mut out, &ui_state, &spinner, &status_line, term_height - 1);
      } else {
        skip_next_bottom_bar = false;
      }
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

  for ch in msg_str.chars() {
    let is_newline_or_wrap =
      ch == '\n' || get_visible_len_for(buffer.last().unwrap()) + 1 > max_width;

    if is_newline_or_wrap {
      buffer.push(String::new());
      // Append the character that caused the wrap so it appears on the new line
      buffer.last_mut().unwrap().push(ch);

      let (_view_start, visible) = viewport(buffer.len(), term_height);

      if buffer.len() >= visible {
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
      let y_disp = if buffer.len() >= visible {
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

      out.flush().unwrap();
    }
  }

  // After message, push another empty line so next content starts fresh
  buffer.push(String::new());

  // Update viewport and clear last line for display
  let (_view_start, visible) = viewport(buffer.len(), terminal::size().unwrap_or((80, 24)).1);

  if buffer.len() >= visible {
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

  for ch in chunk.chars() {
    let is_newline_or_wrap =
      ch == '\n' || get_visible_len_for(buffer.last().unwrap()) + 1 > max_width;

    if is_newline_or_wrap {
      let (_view_start, visible) = viewport(buffer.len(), term_height);

      if buffer.len() >= visible {
        execute!(out, ScrollUp(1)).unwrap();
      }
      buffer.push(String::new());
      // Append the character that caused the wrap so it appears on the new line
      buffer.last_mut().unwrap().push(ch);

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
      let y_disp = if buffer.len() >= visible {
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

      out.flush().unwrap();
    }

    if STOP_STREAM.load(Ordering::Relaxed) {
      STOP_STREAM.store(false, Ordering::Relaxed);
      out.flush().unwrap();
      return;
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
  if ui_state.quiet {
    return String::new();
  }
  let state = GLOBAL_STATE.get().expect("AppState not initialized");
  let agent_name = state.agent_name.lock().unwrap().clone();
  let speak = ui_state.agent_speaking.load(Ordering::Relaxed);

  let think = ui_state.thinking.load(Ordering::Relaxed);
  let play = ui_state.playing.load(Ordering::Relaxed);
  let recording_paused = state.recording_paused.load(Ordering::Relaxed);

  let status = if recording_paused {
    "⏸️".to_string()
  } else if play {
    format!("🔊 ")
  } else if speak {
    format!("🎤 ")
  } else if think {
    format!("🤔 {}", spinner[ui_state.spinner_index % spinner.len()])
  } else {
    format!("🎤 ")
  };

  let speed_str = format!("[{:.1}x]", get_speed());

  // Check if debate mode is enabled
  let debate_enabled = state.debate_enabled.load(Ordering::Relaxed);
  let mode = if debate_enabled {
    let debate_agents = state.debate_agents.lock().unwrap();
    if debate_agents.len() >= 2 {
      let agent1_name = debate_agents[0].name.chars().take(8).collect::<String>();
      let agent2_name = debate_agents[1].name.chars().take(8).collect::<String>();
      format!(
        "\x1b[44m\x1b[37m DEBATE \x1b[0m: {} -- {}",
        agent1_name, agent2_name
      )
    } else {
      format!("\x1b[44m\x1b[37m CONVERSATION \x1b[0m")
    }
  } else {
    format!("\x1b[44m\x1b[37m CONVERSATION \x1b[0m")
  };

  let recording_paused_str = if recording_paused {
    "\x1b[43m\x1b[30m  paused  \x1b[0m"
  } else {
    "\x1b[42m\x1b[30m listening \x1b[0m"
  };

  let internal_status = format!(
    "{}{}{}{}",
    if recording_paused {
      "\x1b[90m█\x1b[0m"
    } else {
      "\x1b[97m█\x1b[0m"
    },
    if speak {
      "\x1b[97m█\x1b[0m"
    } else {
      "\x1b[90m█\x1b[0m"
    },
    if state.playback.paused.load(Ordering::Relaxed) {
      "\x1b[90m█\x1b[0m"
    } else {
      "\x1b[97m█\x1b[0m"
    },
    if state.playback.playback_active.load(Ordering::Relaxed) {
      "\x1b[97m█\x1b[0m"
    } else {
      "\x1b[90m█\x1b[0m"
    },
  );

  let ptt = if state.ptt.load(Ordering::Relaxed) {
    "\x1b[41m\x1b[37m PTT \x1b[0m"
  } else {
    ""
  };

  let lang_guard = state.language.lock().unwrap();
  let flag = get_flag(&lang_guard);
  let agent_display = format!("{} {}", flag, agent_name);
  let combined_status = if debate_enabled {
    format!("{} {} {} ", mode, ptt, internal_status)
  } else {
    format!("{} {} {} {} ", mode, ptt, agent_display, internal_status)
  };

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

fn redraw_buffer<W: Write>(out: &mut W, buffer: &[String]) {
  let (_, term_height) = terminal::size().unwrap_or((80, 24));
  let (view_start, visible) = viewport(buffer.len(), term_height);

  for (i, line) in buffer.iter().enumerate().skip(view_start).take(visible) {
    let y = i - view_start;
    execute!(
      out,
      MoveTo(0, y as u16),
      Clear(ClearType::CurrentLine),
      Print(line)
    )
    .unwrap();
  }
  out.flush().unwrap();
}

fn render_debate_modal<W: Write>(out: &mut W, buffer: &[String]) {
  let state = GLOBAL_STATE.get().expect("AppState not initialized");
  let agents = state.agents.as_ref();
  let agent1_idx = *state.debate_modal_selected_agent1.lock().unwrap();
  let agent2_idx = *state.debate_modal_selected_agent2.lock().unwrap();
  let focus = *state.debate_modal_focus.lock().unwrap();

  let (cols, rows) = terminal::size().unwrap_or((80, 24));

  // Calculate modal dimensions
  let modal_width = std::cmp::min(60, cols - 4);
  let modal_height = std::cmp::min(agents.len() as u16 + 10, rows - 4);
  let modal_x = (cols - modal_width) / 2;
  let modal_y = (rows - modal_height) / 2;

  // Clear the screen first
  execute!(out, Clear(ClearType::All), MoveTo(0, 0)).unwrap();

  // Redraw buffer in the background (dimmed)
  let (_, term_height) = terminal::size().unwrap_or((80, 24));
  let (view_start, visible) = viewport(buffer.len(), term_height);
  for (i, line) in buffer.iter().enumerate().skip(view_start).take(visible) {
    let y = i - view_start;
    execute!(
      out,
      MoveTo(0, y as u16),
      Clear(ClearType::CurrentLine),
      Print(format!("\x1b[90m{}\x1b[0m", line))
    )
    .unwrap();
  }

  // Draw modal background
  for y in modal_y..modal_y + modal_height {
    execute!(
      out,
      MoveTo(modal_x, y),
      Print(format!(
        "\x1b[48;5;234m{}\x1b[0m",
        " ".repeat(modal_width as usize)
      ))
    )
    .unwrap();
  }

  // Draw modal border and title
  execute!(
    out,
    MoveTo(modal_x, modal_y),
    Print(format!(
      "\x1b[48;5;234m\x1b[97m┌{}┐\x1b[0m",
      "─".repeat(modal_width as usize - 2)
    ))
  )
  .unwrap();

  let title = " Select Debate Agents ";
  let title_x = modal_x + (modal_width - title.len() as u16) / 2;
  execute!(
    out,
    MoveTo(title_x, modal_y),
    Print(format!("\x1b[48;5;234m\x1b[97;1m{}\x1b[0m", title))
  )
  .unwrap();

  // Draw agent 1 selection
  let agent1_label = " Agent 1: ";
  execute!(
    out,
    MoveTo(modal_x + 2, modal_y + 2),
    Print(format!(
      "\x1b[48;5;234m{}{}\x1b[0m",
      if focus == 0 { "\x1b[97;1m" } else { "\x1b[90m" },
      agent1_label
    ))
  )
  .unwrap();

  // Agent 1 dropdown
  let dropdown1_width = modal_width as usize - 4 - agent1_label.len();
  let agent1_display = if agents[agent1_idx].name.len() > dropdown1_width - 4 {
    format!("{}...", &agents[agent1_idx].name[..dropdown1_width - 7])
  } else {
    agents[agent1_idx].name.clone()
  };

  execute!(
    out,
    MoveTo(modal_x + 2 + agent1_label.len() as u16, modal_y + 2),
    Print(format!(
      "\x1b[48;5;234m{}{:<width$}\x1b[0m",
      if focus == 0 {
        "\x1b[30;47m"
      } else {
        "\x1b[97;48;5;237m"
      },
      format!(" {} ▼", agent1_display),
      width = dropdown1_width
    ))
  )
  .unwrap();

  // Draw agent 2 selection
  let agent2_label = " Agent 2: ";
  execute!(
    out,
    MoveTo(modal_x + 2, modal_y + 4),
    Print(format!(
      "\x1b[48;5;234m{}{}\x1b[0m",
      if focus == 1 { "\x1b[97;1m" } else { "\x1b[90m" },
      agent2_label
    ))
  )
  .unwrap();

  // Agent 2 dropdown
  let dropdown2_width = modal_width as usize - 4 - agent2_label.len();
  let agent2_display = if agents[agent2_idx].name.len() > dropdown2_width - 4 {
    format!("{}...", &agents[agent2_idx].name[..dropdown2_width - 7])
  } else {
    agents[agent2_idx].name.clone()
  };

  execute!(
    out,
    MoveTo(modal_x + 2 + agent2_label.len() as u16, modal_y + 4),
    Print(format!(
      "\x1b[48;5;234m{}{:<width$}\x1b[0m",
      if focus == 1 {
        "\x1b[30;47m"
      } else {
        "\x1b[97;48;5;237m"
      },
      format!(" {} ▼", agent2_display),
      width = dropdown2_width
    ))
  )
  .unwrap();

  // Show warning if same agent selected
  if agent1_idx == agent2_idx {
    execute!(
      out,
      MoveTo(modal_x + 2, modal_y + 6),
      Print(format!(
        "\x1b[48;5;234m\x1b[91m⚠ Please select two different agents\x1b[0m"
      ))
    )
    .unwrap();
  }

  // Draw instructions
  let instructions_y = modal_y + modal_height - 5;
  execute!(
    out,
    MoveTo(modal_x + 2, instructions_y),
    Print(format!(
      "\x1b[48;5;234m\x1b[90m{}\x1b[0m",
      "─".repeat(modal_width as usize - 4)
    ))
  )
  .unwrap();

  execute!(
    out,
    MoveTo(modal_x + 2, instructions_y + 1),
    Print(format!(
      "\x1b[48;5;234m\x1b[97m Tab/←/→ \x1b[90m Switch focus\x1b[0m"
    ))
  )
  .unwrap();

  execute!(
    out,
    MoveTo(modal_x + 2, instructions_y + 2),
    Print(format!(
      "\x1b[48;5;234m\x1b[97m ↑/↓     \x1b[90m Change selection\x1b[0m"
    ))
  )
  .unwrap();

  execute!(
    out,
    MoveTo(modal_x + 2, instructions_y + 3),
    Print(format!(
      "\x1b[48;5;234m\x1b[97m Enter   \x1b[90m Confirm | \x1b[97mEsc \x1b[90m Cancel\x1b[0m"
    ))
  )
  .unwrap();

  // Draw bottom border
  execute!(
    out,
    MoveTo(modal_x, modal_y + modal_height - 1),
    Print(format!(
      "\x1b[48;5;234m\x1b[97m└{}┘\x1b[0m",
      "─".repeat(modal_width as usize - 2)
    ))
  )
  .unwrap();

  // Draw vertical borders
  for y in (modal_y + 1)..(modal_y + modal_height - 1) {
    execute!(
      out,
      MoveTo(modal_x, y),
      Print("\x1b[48;5;234m\x1b[97m│\x1b[0m")
    )
    .unwrap();
    execute!(
      out,
      MoveTo(modal_x + modal_width - 1, y),
      Print("\x1b[48;5;234m\x1b[97m│\x1b[0m")
    )
    .unwrap();
  }

  out.flush().unwrap();
}
