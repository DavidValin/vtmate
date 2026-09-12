// ------------------------------------------------------------------
//  UI
// ------------------------------------------------------------------

use crate::state::{GLOBAL_STATE, get_speed};
use crate::util::lang_code;
use crossbeam_channel::Receiver;
use crossterm::{
  cursor::{Hide, MoveTo, Show},
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
use std::time::{Duration, Instant};

/// How long the terminal size must hold still before the screen is redrawn:
/// one redraw at the end of a drag, not one per intermediate width.
///
/// Long enough to outlast the pauses in a slow drag. Each redraw puts a
/// full-width bar back on the last row, and a terminal that rewraps on resize
/// splits that row at the next step - the padding becomes rows that look
/// empty - so a redraw mid-drag is what makes those appear.
const RESIZE_SETTLE: Duration = Duration::from_millis(400);

/// How long the UI loop waits between passes.
const TICK: Duration = Duration::from_millis(4);

/// How long each spinner frame stays on screen. Tracked by wall clock rather
/// than by counting loop passes, so tuning TICK for resize responsiveness -
/// dropped from 10ms to this file's 4ms - cannot silently speed the spinner
/// up as a side effect the way advancing it once per pass did.
const SPINNER_FRAME: Duration = Duration::from_millis(100);

/// Width of the recording/paused tag: the longer word with two spaces either
/// side. Both tags are drawn this wide so the bar keeps still between states.
const TAG_WIDTH: usize = "recording".len() + 4;

/// Centre `text` in a field `width` columns wide. An odd remainder leaves the
/// extra space on the right.
fn centred(text: &str, width: usize) -> String {
  let text_cols = visible_columns(text);
  let pad = width.saturating_sub(text_cols);
  let left = pad / 2;
  format!(
    "{}{}{}",
    " ".repeat(left),
    text,
    " ".repeat(pad - left)
  )
}

// API
// ------------------------------------------------------------------

pub static STOP_STREAM: AtomicBool = AtomicBool::new(false);

/// Set while the interface is being torn down. The bottom bar is redrawn from
/// several places and on a timer, so without this it reappears underneath the
/// parting message, leaving a stale bar above it and a fresh one below.
pub static UI_SHUTDOWN: AtomicBool = AtomicBool::new(false);

/// Set once the loop below has noticed UI_SHUTDOWN and stopped. In
/// daemon-attach mode a background reader thread keeps feeding it new
/// messages regardless of the local terminal, so a caller about to print its
/// own final line needs to wait for this rather than assume the flag alone
/// is enough.
pub static UI_STOPPED: AtomicBool = AtomicBool::new(false);

/// Set for as long as a UI thread is running. Lets a waiting caller skip the
/// wait entirely for commands with no UI thread at all (--list-voices,
/// --daemon-status, ...).
pub static UI_RUNNING: AtomicBool = AtomicBool::new(false);

/// Set for as long as the process is actually on the alternate screen.
/// `LeaveAlternateScreen` (CSI ?1049l) restores whatever cursor position was
/// last saved by CSI ?1049h on some terminals, even when sent without a
/// matching enter in the current session - so sending it unconditionally on
/// exit can snap the cursor back to an unrelated, stale position instead of
/// leaving it where the last thing printed put it. `terminate()` checks this
/// first.
pub static ON_ALT_SCREEN: AtomicBool = AtomicBool::new(false);

/// Block until the UI thread stops, or `timeout` passes. A no-op if none was
/// ever started.
pub fn wait_for_ui_stopped(timeout: Duration) {
  if !UI_RUNNING.load(Ordering::Relaxed) {
    return;
  }
  let deadline = Instant::now() + timeout;
  while !UI_STOPPED.load(Ordering::Relaxed) && Instant::now() < deadline {
    thread::sleep(Duration::from_millis(2));
  }
}

// ANSI labels
pub const USER_LABEL: &str = "\x1b[47;30mUSER\x1b[0m";
pub const ASSIST_LABEL: &str = "\x1b[48;5;22;37mASSISTANT\x1b[0m";

/// The vtmate mark (a chevron and level bars) beside the wordmark, drawn
/// with block characters and coloured for the terminal.
pub fn get_banner() -> &'static str {
  concat!(
    "\n",
    "  \u{1b}[38;5;49m\u{2588}\u{2588}       \u{1b}[38;5;83m\u{2588}\u{1b}[38;5;83m\u{2588} \u{1b}[38;5;119m\u{2588}\u{1b}[38;5;119m\u{2588} \u{1b}[38;5;155m\u{2588}\u{1b}[38;5;155m\u{2588}     \u{1b}[0m\n",
    "  \u{1b}[38;5;49m \u{2588}\u{2588}   \u{1b}[38;5;48m\u{2588}\u{1b}[38;5;48m\u{2588} \u{1b}[38;5;83m\u{2588}\u{1b}[38;5;83m\u{2588} \u{1b}[38;5;119m\u{2588}\u{1b}[38;5;119m\u{2588} \u{1b}[38;5;155m\u{2588}\u{1b}[38;5;155m\u{2588} \u{1b}[38;5;191m\u{2588}\u{1b}[38;5;191m\u{2588}  \u{1b}[38;5;255m\u{2588}   \u{2588} \u{2580}\u{2580}\u{2588}\u{2580}\u{2580} \u{2588}\u{2584} \u{2584}\u{2588} \u{2584}\u{2580}\u{2580}\u{2580}\u{2584} \u{2580}\u{2580}\u{2588}\u{2580}\u{2580} \u{2588}\u{2580}\u{2580}\u{2580}\u{2580}\u{1b}[0m\n",
    "  \u{1b}[38;5;49m  \u{2588}\u{2588}  \u{1b}[38;5;48m\u{2588}\u{1b}[38;5;48m\u{2588} \u{1b}[38;5;83m\u{2588}\u{1b}[38;5;83m\u{2588} \u{1b}[38;5;119m\u{2588}\u{1b}[38;5;119m\u{2588} \u{1b}[38;5;155m\u{2588}\u{1b}[38;5;155m\u{2588} \u{1b}[38;5;191m\u{2588}\u{1b}[38;5;191m\u{2588}  \u{1b}[38;5;255m\u{2588}   \u{2588}   \u{2588}   \u{2588} \u{2580} \u{2588} \u{2588}\u{2584}\u{2584}\u{2584}\u{2588}   \u{2588}   \u{2588}\u{2580}\u{2580}\u{2580} \u{1b}[0m\n",
    "  \u{1b}[38;5;49m \u{2588}\u{2588}   \u{1b}[38;5;48m\u{2588}\u{1b}[38;5;48m\u{2588} \u{1b}[38;5;83m\u{2588}\u{1b}[38;5;83m\u{2588} \u{1b}[38;5;119m\u{2588}\u{1b}[38;5;119m\u{2588} \u{1b}[38;5;155m\u{2588}\u{1b}[38;5;155m\u{2588} \u{1b}[38;5;191m\u{2588}\u{1b}[38;5;191m\u{2588}  \u{1b}[38;5;255m \u{2580}\u{2584}\u{2580}    \u{2588}   \u{2588}   \u{2588} \u{2588}   \u{2588}   \u{2588}   \u{2588}\u{2584}\u{2584}\u{2584}\u{2584}\u{1b}[0m\n",
    "  \u{1b}[38;5;49m\u{2588}\u{2588}       \u{1b}[38;5;83m\u{2588}\u{1b}[38;5;83m\u{2588} \u{1b}[38;5;119m\u{2588}\u{1b}[38;5;119m\u{2588} \u{1b}[38;5;155m\u{2588}\u{1b}[38;5;155m\u{2588}     \u{1b}[0m\n"
  )
}

/// The same banner without colour, for the conversation files `-s` writes.
pub fn get_banner_plain() -> &'static str {
  concat!(
    "\n",
    "  \u{2588}\u{2588}       \u{2588}\u{2588} \u{2588}\u{2588} \u{2588}\u{2588}\n",
    "   \u{2588}\u{2588}   \u{2588}\u{2588} \u{2588}\u{2588} \u{2588}\u{2588} \u{2588}\u{2588} \u{2588}\u{2588}  \u{2588}   \u{2588} \u{2580}\u{2580}\u{2588}\u{2580}\u{2580} \u{2588}\u{2584} \u{2584}\u{2588} \u{2584}\u{2580}\u{2580}\u{2580}\u{2584} \u{2580}\u{2580}\u{2588}\u{2580}\u{2580} \u{2588}\u{2580}\u{2580}\u{2580}\u{2580}\n",
    "    \u{2588}\u{2588}  \u{2588}\u{2588} \u{2588}\u{2588} \u{2588}\u{2588} \u{2588}\u{2588} \u{2588}\u{2588}  \u{2588}   \u{2588}   \u{2588}   \u{2588} \u{2580} \u{2588} \u{2588}\u{2584}\u{2584}\u{2584}\u{2588}   \u{2588}   \u{2588}\u{2580}\u{2580}\u{2580}\n",
    "   \u{2588}\u{2588}   \u{2588}\u{2588} \u{2588}\u{2588} \u{2588}\u{2588} \u{2588}\u{2588} \u{2588}\u{2588}   \u{2580}\u{2584}\u{2580}    \u{2588}   \u{2588}   \u{2588} \u{2588}   \u{2588}   \u{2588}   \u{2588}\u{2584}\u{2584}\u{2584}\u{2584}\n",
    "  \u{2588}\u{2588}       \u{2588}\u{2588} \u{2588}\u{2588} \u{2588}\u{2588}\n"
  )
}

const CHAR_DELAY_MS: u64 = 4;

pub fn spawn_ui_thread(
  ui_state: crate::state::UiState,
  status_line: Arc<Mutex<String>>,
  rx_ui: Receiver<String>,
  conversation_history: crate::conversation::ConversationHistory,
  no_banner: bool,
) -> thread::JoinHandle<()> {
  thread::spawn(move || {
    UI_RUNNING.store(true, Ordering::Relaxed);
    let conversation_history = conversation_history;
    let mut ui_state = ui_state;
    let mut out = io::stdout();
    execute!(out, Hide).unwrap();

    let spinner = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
    let mut bottom_bar = String::new();
    let mut buffer = History::new();
    let mut last_term_size = terminal::size().unwrap_or((80, 24));
    // Set while the terminal size is still moving; the redraw fires once it
    // has held still for RESIZE_SETTLE.
    let mut resize_settling: Option<Instant> = None;
    let mut last_spinner_advance = Instant::now();
    let mut pending_stream: Vec<String> = Vec::new();
    let mut modal_visible = false;
    let mut settings_visible = false;
    // A popup runs on the alternate screen, so neither it nor anything
    // scrolling underneath it reaches the primary screen's scrollback.
    let mut popup_on_alt = false;
    // A resize while it was open leaves the restored screen laid out for the
    // old size, so that case needs a real repaint.
    let mut resized_during_popup = false;

    crossterm::execute!(
      std::io::stdout(),
      crossterm::terminal::Clear(ClearType::All),
      MoveTo(0, 0)
    )
    .unwrap();

    if !no_banner {
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
    }

    let mut waiting_for_first_line = true;
    let mut skip_next_bottom_bar = false;

    loop {
      // Dragging a window edge produces a run of sizes; the redraw waits for
      // it to stop moving and then happens once.
      let size_now = terminal::size().unwrap_or((80, 24));
      if size_now != last_term_size {
        // First change of this drag: blank the screen and stop drawing until
        // it settles. A terminal reflows its buffer as it resizes, so anything
        // written while the size is moving is carried up into the history -
        // a bar, or the blank row left by wiping one. An empty screen carries
        // nothing, and the redraw at the end is the only one that has to
        // happen.
        if resize_settling.is_none() {
          execute!(
            out,
            ResetColor,
            Clear(ClearType::All),
            Print("\x1b[3J"),
            MoveTo(0, 0)
          )
          .unwrap();
          let _ = out.flush();
          bottom_bar.clear();
        }
        last_term_size = size_now;
        resize_settling = Some(Instant::now());
        if popup_on_alt {
          resized_during_popup = true;
        }
      } else if let Some(since) = resize_settling {
        if since.elapsed() >= RESIZE_SETTLE {
          resize_settling = None;
          // The history is wrapped to the old width, so lay it out again at
          // the new one before printing it back.
          buffer.rewrap(size_now.0);
          bottom_bar = reprint_history(&mut out, &buffer, &ui_state, &spinner, &status_line);
          if settings_visible {
            crate::settings_ui::render(&mut out, &buffer.wrapped);
          } else if modal_visible {
            render_debate_modal(&mut out, &buffer.wrapped);
          }
        }
      }

      while let Ok(msg) = rx_ui.try_recv() {
        let mut parts = msg.splitn(2, '|');
        let msg_type = parts.next().unwrap_or("");

        match msg_type {
          // The very last thing a caller wants shown before this thread ends:
          // rendered, then stopped right here, in the same match arm, so
          // nothing else queued or still arriving from a background thread
          // (daemon-attach's reader, say) can be processed after it. A
          // time-based shutdown check cannot promise that against a sender
          // with its own ongoing supply of messages - draining until one
          // runs dry, or bailing before draining what is already queued, are
          // the only two outcomes it can produce, and this needed neither.
          "final_line" => {
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
            UI_STOPPED.store(true, Ordering::Relaxed);
            return;
          }

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

          "user_interrupt_show" => {
            STOP_STREAM.store(true, Ordering::Relaxed);
            pending_stream.clear();
            waiting_for_first_line = false;

            handle_line_message(
              &mut out,
              // A geometric shape, not an emoji: no colour-emoji font to
              // install, one column in any terminal, and the red comes from
              // the escape rather than from the glyph.
              "\n\n \x1b[31m■ USER interrupted\x1b[0m",
              &mut buffer,
              &mut ui_state,
              &spinner,
              &status_line,
              &mut bottom_bar,
            );
            skip_next_bottom_bar = true;
          }

          "modal_show" => {
            if !modal_visible {
              modal_visible = true;
              open_popup_screen(&mut out, &mut popup_on_alt, &mut resized_during_popup);
            }
            render_debate_modal(&mut out, &buffer.wrapped);
          }

          "settings_show" | "settings_update" => {
            if !settings_visible {
              settings_visible = true;
              open_popup_screen(&mut out, &mut popup_on_alt, &mut resized_during_popup);
            }
            crate::settings_ui::render(&mut out, &buffer.wrapped);
          }

          "settings_hide" => {
            settings_visible = false;
            bottom_bar = close_popup_screen(
              &mut out,
              &mut popup_on_alt,
              resized_during_popup,
              &buffer,
              &ui_state,
              &spinner,
              &status_line,
            );
          }

          "modal_hide" => {
            modal_visible = false;
            bottom_bar = close_popup_screen(
              &mut out,
              &mut popup_on_alt,
              resized_during_popup,
              &buffer,
              &ui_state,
              &spinner,
              &status_line,
            );
          }

          "modal_update" => {
            if modal_visible {
              render_debate_modal(&mut out, &buffer.wrapped);
            }
          }

          "redraw_full_history" => {
            // Rebuilt in bulk (rebuild_history) and printed once
            // (reprint_history): the character-reveal path redraws the bottom
            // bar and issues ScrollUp per wrapped line, which past one
            // screenful carries a full-width bar into the real scrollback on
            // every line.
            let cols = terminal::size().unwrap_or((80, 24)).0;
            rebuild_history(&mut buffer, conversation_history.lock().unwrap().iter());
            buffer.rewrap(cols);
            bottom_bar = reprint_history(&mut out, &buffer, &ui_state, &spinner, &status_line);
          }

          _ => {}
        }

        // Anything printed underneath (an answer still streaming, a log line)
        // would run over the popup, so it goes back on top.
        if settings_visible && !msg_type.starts_with("settings") {
          crate::settings_ui::render(&mut out, &buffer.wrapped);
        }
      }

      // Checked after draining, not before: a caller printing its own final
      // line sends it and sets this in either order (see attach::finish), and
      // whatever reached the channel before this point must be drawn - a
      // bounded(1) channel cannot pile up a backlog behind it, so nothing
      // after this drain was ever going to add more before the next check.
      // Breaking here, before the tick's own bottom-bar redraw below, is what
      // stops that redraw from drawing over the message just processed.
      if UI_SHUTDOWN.load(Ordering::Relaxed) {
        UI_STOPPED.store(true, Ordering::Relaxed);
        break;
      }

      if last_spinner_advance.elapsed() >= SPINNER_FRAME {
        ui_state.spinner_index = (ui_state.spinner_index + 1) % spinner.len();
        last_spinner_advance = Instant::now();
      }

      let (_cols, term_height) = terminal::size().unwrap_or((80, 24));
      if resize_settling.is_some() {
        // Nothing is drawn while the size is moving; see the resize check.
      } else if !skip_next_bottom_bar {
        bottom_bar = render_bottom_bar(
          &mut out,
          &ui_state,
          &spinner,
          &status_line,
          term_height.saturating_sub(1),
        );
      } else {
        skip_next_bottom_bar = false;
      }
      // Short enough that a drag is noticed within a frame: until it is, the
      // terminal is reflowing the full-width bar it still has on screen, which
      // is the one jump this cannot pre-empt.
      thread::sleep(TICK);
    }
  })
}

// PRIVATE
// ------------------------------------------------------------------

// computes viewport for scroll
/// The printed history, in two forms.
///
/// `wrapped` is what is on screen, broken to the width it was printed at, and
/// is what the viewport and every redraw work in. `logical` is the same text
/// unbroken, one entry per real line, so a resize can lay it out again: a line
/// wrapped at 60 columns has forgotten where it would break at 100.
pub struct History {
  pub wrapped: Vec<String>,
  pub logical: Vec<String>,
}

impl History {
  fn new() -> Self {
    History {
      wrapped: Vec::new(),
      logical: Vec::new(),
    }
  }

  fn clear(&mut self) {
    self.wrapped.clear();
    self.logical.clear();
  }

  /// Start a new line in both forms.
  fn newline(&mut self) {
    self.wrapped.push(String::new());
    self.logical.push(String::new());
  }

  fn push_char(&mut self, ch: char) {
    if self.wrapped.is_empty() {
      self.newline();
    }
    self.wrapped.last_mut().unwrap().push(ch);
    self.logical.last_mut().unwrap().push(ch);
  }

  /// End the screen row, carrying any unfinished word onto the new one so a
  /// word is never split. Returns whether the row left behind changed and so
  /// needs drawing again.
  ///
  /// A word with no space before it fills the row alone and has nowhere to go,
  /// so it is split - the same last resort `wrap_line` takes, which keeps both
  /// breaking in the same places.
  fn wrap_at_word(&mut self) -> bool {
    let row = self.wrapped.last_mut().unwrap();
    let carry = match row.rfind(' ') {
      // Escapes never contain a space, so a split just after one cannot land
      // inside a sequence.
      Some(i) => {
        let carried = row[i + 1..].to_string();
        row.truncate(i + 1);
        carried
      }
      None => String::new(),
    };
    let moved = !carry.is_empty();
    self.wrapped.push(carry);
    moved
  }

  /// Lay the whole history out again at `cols`. Used after a resize.
  fn rewrap(&mut self, cols: u16) {
    let max_width = (cols as usize).max(1);
    let mut wrapped = Vec::with_capacity(self.wrapped.len());
    for line in &self.logical {
      wrap_line(line, max_width, &mut wrapped);
    }
    self.wrapped = wrapped;
  }
}

/// One visible character together with any escape sequences that precede it.
/// Escapes cost no columns and must travel with the character they colour, so
/// wrapping moves them as a unit.
struct Unit {
  text: String,
  cols: usize,
  is_space: bool,
}

/// Split a line into units, escapes attached to the character they precede.
fn units_of(line: &str) -> Vec<Unit> {
  let mut units = Vec::new();
  let mut pending = String::new();
  let mut chars = line.chars().peekable();
  while let Some(ch) = chars.next() {
    if ch == '\u{1b}' {
      pending.push(ch);
      // ESC '[' starts a CSI, which runs to a byte in '@'..='~'. The '[' is
      // itself in that range, so it must not be mistaken for the end.
      if chars.peek() == Some(&'[') {
        pending.push(chars.next().unwrap());
        for c in chars.by_ref() {
          pending.push(c);
          if ('@'..='~').contains(&c) {
            break;
          }
        }
      } else if let Some(c) = chars.next() {
        pending.push(c);
      }
      continue;
    }
    let cols = width_of(ch, chars.peek());
    let mut text = std::mem::take(&mut pending);
    text.push(ch);
    units.push(Unit {
      text,
      cols,
      is_space: ch == ' ',
    });
  }
  // Escapes with nothing after them (a trailing reset) ride on a zero-width
  // unit so they are not dropped.
  if !pending.is_empty() {
    units.push(Unit {
      text: pending,
      cols: 0,
      is_space: false,
    });
  }
  units
}

/// Break one logical line into screen rows of at most `max_width` visible
/// columns, appending them to `out`. An empty line still produces one row.
///
/// Breaks between words, never inside one: a word that will not fit starts the
/// next row, and only a word too long for a whole row is split.
///
/// Spaces at a break are dropped rather than carried on, so a line padded out
/// to the old width does not become rows of pure whitespace when narrowed.
fn wrap_line(line: &str, max_width: usize, out: &mut Vec<String>) {
  let max_width = max_width.max(1);
  let units = units_of(line);
  let mut row = String::new();
  let mut row_cols = 0usize;
  // Spaces seen since the last word: they join the row only if more text
  // follows on it.
  let mut gap = String::new();
  let mut gap_cols = 0usize;
  let mut word: Vec<&Unit> = Vec::new();
  let mut word_cols = 0usize;

  let flush_word = |row: &mut String,
                    row_cols: &mut usize,
                    gap: &mut String,
                    gap_cols: &mut usize,
                    word: &mut Vec<&Unit>,
                    word_cols: &mut usize,
                    out: &mut Vec<String>| {
    if word.is_empty() {
      return;
    }
    if *row_cols + *gap_cols + *word_cols <= max_width {
      row.push_str(gap);
      *row_cols += *gap_cols;
      for u in word.iter() {
        row.push_str(&u.text);
      }
      *row_cols += *word_cols;
    } else {
      // Does not fit: the gap falls at the break and the word starts a row.
      if *row_cols > 0 {
        out.push(std::mem::take(row));
        *row_cols = 0;
      }
      for u in word.iter() {
        // Only a word too long for an entire row is split.
        if *row_cols + u.cols > max_width && *row_cols > 0 {
          out.push(std::mem::take(row));
          *row_cols = 0;
        }
        row.push_str(&u.text);
        *row_cols += u.cols;
      }
    }
    gap.clear();
    *gap_cols = 0;
    word.clear();
    *word_cols = 0;
  };

  for u in &units {
    if u.is_space {
      flush_word(
        &mut row,
        &mut row_cols,
        &mut gap,
        &mut gap_cols,
        &mut word,
        &mut word_cols,
        out,
      );
      gap.push_str(&u.text);
      gap_cols += u.cols;
    } else {
      word.push(u);
      word_cols += u.cols;
    }
  }
  flush_word(
    &mut row,
    &mut row_cols,
    &mut gap,
    &mut gap_cols,
    &mut word,
    &mut word_cols,
    out,
  );
  // Trailing escapes belong on the last row; trailing padding does not.
  if !gap.trim().is_empty() && row_cols + gap_cols <= max_width {
    row.push_str(&gap);
  }
  out.push(row);
}

/// Lay the whole history out again: clear the screen and the scrollback, then
/// print every row.
///
/// Only the resize path wants this: re-flowing lines that have already
/// scrolled past means printing them back, and the scrollback they sit in has
/// to go first or the history appears twice, once at each width.
///
/// ED 2 pushes the frame it clears into the scrollback on many terminals; the
/// ED 3 straight after takes that copy with it. The pair is safe where ED 2
/// alone is not, so they belong together, in that order.
///
/// Every row is printed with a trailing newline, the last included, so the
/// final row lands one line above the bottom bar - the layout `viewport`
/// assumes everywhere else.
/// Rebuild `buffer` from a conversation's messages: a role-label line, then
/// the message's own text, one entry per message, with direct pushes into
/// `History` and no redraw in between - the caller draws the result in one
/// pass once it is ready.
fn rebuild_history<'a>(
  buffer: &mut History,
  messages: impl Iterator<Item = &'a crate::conversation::ChatMessage>,
) {
  buffer.clear();
  for msg in messages {
    let role_label = if msg.role == "assistant" {
      "\x1b[48;5;22;37mASSISTANT\x1b[0m"
    } else {
      "\x1b[47;30mUSER\x1b[0m"
    };
    for text in [role_label, msg.content.as_str()] {
      if buffer.wrapped.is_empty() {
        buffer.newline();
      }
      for ch in text.chars() {
        if ch == '\n' {
          buffer.newline();
        } else {
          buffer.push_char(ch);
        }
      }
      buffer.newline();
    }
    // A second empty row: without it, the next role label lands directly on
    // the one row content's own newline leaves, and messages run together.
    buffer.newline();
  }
}

fn reprint_history<W: Write>(
  out: &mut W,
  history: &History,
  ui_state: &crate::state::UiState,
  spinner: &[&str],
  status_line: &Arc<Mutex<String>>,
) -> String {
  execute!(
    out,
    ResetColor,
    Clear(ClearType::All),
    Print("\x1b[3J"),
    MoveTo(0, 0)
  )
  .unwrap();
  for line in &history.wrapped {
    let _ = write!(out, "{}\r\n", line);
  }
  out.flush().unwrap();
  let (_cols, term_height) = terminal::size().unwrap_or((80, 24));
  render_bottom_bar(
    out,
    ui_state,
    spinner,
    status_line,
    term_height.saturating_sub(1),
  )
}

/// Draw the viewport rows, clearing each one as it goes, and blank any rows
/// below the end of the history.
///
/// Deliberately not `Clear(ClearType::All)`: on the primary screen most
/// terminals push the frame it clears into the scrollback instead of
/// overwriting it, depositing a copy of the screen - bottom bar included - in
/// the history. Clearing only the rows being rewritten cannot.
///
/// Split out of `repaint` so it can be tested without a running session,
/// which `render_bottom_bar` needs.
fn paint_viewport<W: Write>(out: &mut W, history: &History) {
  let (_cols, term_height) = terminal::size().unwrap_or((80, 24));
  let (_view_start, visible) = viewport(history.wrapped.len(), term_height);
  redraw_buffer(out, &history.wrapped);
  for y in history.wrapped.len().min(visible)..visible {
    execute!(
      out,
      MoveTo(0, y as u16),
      ResetColor,
      Clear(ClearType::CurrentLine)
    )
    .unwrap();
  }
  out.flush().unwrap();
}

/// Draw the viewport and the bottom bar again.
fn repaint<W: Write>(
  out: &mut W,
  history: &History,
  ui_state: &crate::state::UiState,
  spinner: &[&str],
  status_line: &Arc<Mutex<String>>,
) -> String {
  let (_cols, term_height) = terminal::size().unwrap_or((80, 24));
  paint_viewport(out, history);
  render_bottom_bar(
    out,
    ui_state,
    spinner,
    status_line,
    term_height.saturating_sub(1),
  )
}

/// Move a popup onto the alternate screen.
///
/// The popup draws with positioned writes, but the UI keeps printing
/// underneath it - a log line, an answer still streaming - and each of those
/// scrolls the primary screen, pushing rows of the popup into the scrollback.
/// The alternate screen has none, so nothing escapes and closing the popup is
/// a restore. Same reasoning as `open_clone_progress_popup`.
fn open_popup_screen<W: Write>(out: &mut W, on_alt: &mut bool, resized: &mut bool) {
  if *on_alt {
    return;
  }
  *on_alt = true;
  ON_ALT_SCREEN.store(true, Ordering::Relaxed);
  *resized = false;
  execute!(
    out,
    terminal::EnterAlternateScreen,
    Hide,
    Clear(ClearType::All),
    MoveTo(0, 0)
  )
  .unwrap();
  out.flush().unwrap();
}

/// Close a popup: leave the alternate screen, which puts the primary screen
/// and its scrollback back as they were, then repaint the viewport so anything
/// printed while it was open is shown.
fn close_popup_screen<W: Write>(
  out: &mut W,
  on_alt: &mut bool,
  resized_during_popup: bool,
  history: &History,
  ui_state: &crate::state::UiState,
  spinner: &[&str],
  status_line: &Arc<Mutex<String>>,
) -> String {
  if *on_alt {
    *on_alt = false;
    ON_ALT_SCREEN.store(false, Ordering::Relaxed);
    execute!(out, terminal::LeaveAlternateScreen, Hide).unwrap();
  }
  // Leaving the alternate screen puts the primary one back as it was, which
  // is already right unless the terminal was resized underneath it or we
  // never got there at all (an older terminal). Repainting is cheap and
  // correct in every case, so it is not worth branching on.
  let _ = resized_during_popup;
  repaint(out, history, ui_state, spinner, status_line)
}

/// Draw the row a wrap just left, when carrying a word off its end made it
/// shorter. Nothing else goes back to correct it.
fn redraw_row_left_behind<W: Write>(out: &mut W, history: &History, visible: usize, changed: bool) {
  if !changed || history.wrapped.len() < 2 {
    return;
  }
  let y = std::cmp::min(history.wrapped.len(), visible) - 1;
  if y == 0 {
    return;
  }
  execute!(
    out,
    MoveTo(0, y as u16 - 1),
    ResetColor,
    Clear(ClearType::CurrentLine),
    Print(&history.wrapped[history.wrapped.len() - 2])
  )
  .unwrap();
}

fn viewport(buffer_len: usize, term_height: u16) -> (usize, usize) {
  let visible = term_height.saturating_sub(1) as usize;
  let view_start = buffer_len.saturating_sub(visible);
  (view_start, visible)
}

// handles a complete line print
fn handle_line_message<W: Write>(
  out: &mut W,
  msg_str: &str,
  buffer: &mut History,
  ui_state: &mut crate::state::UiState,
  spinner: &[&str],
  status_line: &Arc<Mutex<String>>,
  bottom_bar: &mut String,
) {
  let (cols, term_height) = terminal::size().unwrap_or((80, 24));
  let max_width = cols as usize;

  if buffer.wrapped.is_empty() {
    buffer.newline();
  }

  for ch in msg_str.chars() {
    let is_newline_or_wrap =
      ch == '\n' || get_visible_len_for(buffer.wrapped.last().unwrap()) + 1 > max_width;

    if is_newline_or_wrap {
      // A '\n' ends the logical line; a wrap only ends the screen row, so the
      // logical line carries on and the character that overflowed joins it.
      let mut left_row_changed = false;
      if ch == '\n' {
        buffer.newline();
      } else {
        left_row_changed = buffer.wrap_at_word();
        buffer.push_char(ch);
      }
      let (_view_start, visible) = viewport(buffer.wrapped.len(), term_height);

      if buffer.wrapped.len() >= visible {
        execute!(out, ScrollUp(1)).unwrap();
      }
      redraw_row_left_behind(out, buffer, visible, left_row_changed);
      execute!(
        out,
        MoveTo(0, (std::cmp::min(buffer.wrapped.len(), visible)) as u16 - 1),
        ResetColor,
        Clear(ClearType::CurrentLine)
      )
      .unwrap();

      *bottom_bar = render_bottom_bar(
        out,
        ui_state,
        spinner,
        status_line,
        term_height.saturating_sub(1),
      );
    } else {
      buffer.push_char(ch);

      let (_view_start, visible) = viewport(buffer.wrapped.len(), term_height);
      let y_disp = if buffer.wrapped.len() >= visible {
        visible - 1
      } else {
        buffer.wrapped.len() - 1
      };

      execute!(
        out,
        MoveTo(0, y_disp as u16),
        ResetColor,
        Clear(ClearType::CurrentLine),
        Print(buffer.wrapped.last().unwrap())
      )
      .unwrap();

      out.flush().unwrap();
    }
  }

  // After message, push another empty line so next content starts fresh
  buffer.newline();

  // Update viewport and clear last line for display
  let (_view_start, visible) =
    viewport(buffer.wrapped.len(), terminal::size().unwrap_or((80, 24)).1);

  if buffer.wrapped.len() >= visible {
    execute!(out, ScrollUp(1)).unwrap();
  }

  execute!(
    out,
    MoveTo(0, (std::cmp::min(buffer.wrapped.len(), visible)) as u16 - 1),
    ResetColor,
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
    term_height.saturating_sub(1),
  );
}

fn handle_stream_message<W: Write>(
  out: &mut W,
  msg_str: &str,
  buffer: &mut History,
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
  buffer: &mut History,
  ui_state: &mut crate::state::UiState,
  spinner: &[&str],
  status_line: &Arc<Mutex<String>>,
  bottom_bar: &mut String,
) {
  let (cols, term_height) = terminal::size().unwrap_or((80, 24));
  let max_width = cols as usize;

  // Colour codes take no room on screen, so pausing on them only makes the
  // reveal crawl: the animation is paced by the characters you can actually see.
  let mut in_escape = false;
  for ch in chunk.chars() {
    let visible_char = if in_escape {
      if ('@'..='~').contains(&ch) {
        in_escape = false;
      }
      false
    } else if ch == '\u{1b}' {
      in_escape = true;
      false
    } else {
      true
    };
    // Buffer a colour sequence whole: redrawing the line halfway through one
    // would send the terminal a truncated escape and make it flicker.
    if !visible_char {
      buffer.push_char(ch);
      continue;
    }
    let is_newline_or_wrap =
      ch == '\n' || get_visible_len_for(buffer.wrapped.last().unwrap()) + 1 > max_width;

    if is_newline_or_wrap {
      let (_view_start, visible) = viewport(buffer.wrapped.len(), term_height);

      if buffer.wrapped.len() >= visible {
        execute!(out, ScrollUp(1)).unwrap();
      }
      // A '\n' ends the logical line; a wrap only ends the screen row, so the
      // logical line carries on and the character that overflowed joins it.
      let mut left_row_changed = false;
      if ch == '\n' {
        buffer.newline();
      } else {
        left_row_changed = buffer.wrap_at_word();
        buffer.push_char(ch);
      }
      redraw_row_left_behind(out, buffer, visible, left_row_changed);

      execute!(
        out,
        MoveTo(0, (std::cmp::min(buffer.wrapped.len(), visible)) as u16 - 1),
        ResetColor,
        Clear(ClearType::CurrentLine)
      )
      .unwrap();

      *bottom_bar = render_bottom_bar(
        out,
        ui_state,
        spinner,
        status_line,
        term_height.saturating_sub(1),
      );
    } else {
      buffer.push_char(ch);

      let (_view_start, visible) = viewport(buffer.wrapped.len(), term_height);
      let y_disp = if buffer.wrapped.len() >= visible {
        visible - 1
      } else {
        buffer.wrapped.len() - 1
      };

      execute!(
        out,
        MoveTo(0, y_disp as u16),
        ResetColor,
        Clear(ClearType::CurrentLine),
        Print(buffer.wrapped.last().unwrap())
      )
      .unwrap();

      out.flush().unwrap();
    }

    // Checked before STOP_STREAM: a reveal here can run for whole seconds on
    // a long reply, four milliseconds per character, and none of that time
    // was ever spent noticing UI_SHUTDOWN - it lives only in the message loop
    // this function is called from, which does not get another look in until
    // the reveal finishes on its own.
    if UI_SHUTDOWN.load(Ordering::Relaxed) {
      out.flush().unwrap();
      return;
    }

    if STOP_STREAM.load(Ordering::Relaxed) {
      STOP_STREAM.store(false, Ordering::Relaxed);
      out.flush().unwrap();
      return;
    }

    if visible_char {
      thread::sleep(Duration::from_millis(CHAR_DELAY_MS));
    }
  }
}

/// Where the two halves of the bottom bar go.
struct BarLayout {
  /// Status icon and meter, cut so it cannot reach the right-hand group.
  left: String,
  /// Speed, mode, agent and the listening/paused tag, cut to the width.
  right: String,
  /// Column the right-hand group starts at, chosen so it ends on the last one.
  right_x: usize,
  /// The two joined by padding, for callers that want the line as text.
  full: String,
}

/// Place the bar's two halves in a row `cols` wide.
///
/// The right-hand group is positioned rather than padded up to, so the status
/// icon's width cannot move it: a terminal drawing emoji from a text font
/// gives that icon one column where the standard says two, which would leave
/// the group a column short of the edge. Nothing in the group itself is
/// ambiguous - digits, letters and box drawing are one column everywhere.
///
/// Both halves are cut to the room they have, so the row is never overrun
/// however long an agent name is or how narrow the terminal gets.
fn bar_layout(
  status: &str,
  meter: &str,
  speed: &str,
  combined: &str,
  tag: &str,
  cols: usize,
) -> BarLayout {
  let left = format!("{} {}", status, meter);
  let right = format!("{} {}{}", speed, combined, tag);
  // Measured after cutting, not before: a cut lands a column early when the
  // glyph that would straddle the edge is two columns wide, and a start column
  // worked out from the uncut width would leave the group one short.
  let right = fit_to_width(&right, cols);
  let right_x = cols - visible_columns(&right);
  let left = fit_to_width(&left, right_x);
  let full = format!(
    "{}{}{}",
    left,
    " ".repeat(right_x.saturating_sub(visible_columns(&left))),
    right
  );
  BarLayout {
    left,
    right,
    right_x,
    full,
  }
}

fn render_bottom_bar<W: Write>(
  out: &mut W,
  ui_state: &crate::state::UiState,
  spinner: &[&str],
  status_line: &Arc<Mutex<String>>,
  y: u16,
) -> String {
  if ui_state.quiet || UI_SHUTDOWN.load(Ordering::Relaxed) {
    return String::new();
  }
  let state = GLOBAL_STATE.get().expect("AppState not initialized");
  let agent_name = state.agent_name.lock().unwrap().clone();
  let speak = ui_state.agent_speaking.load(Ordering::Relaxed);

  let think = ui_state.thinking.load(Ordering::Relaxed);
  let play = ui_state.playing.load(Ordering::Relaxed);
  let recording_paused = state.recording_paused.load(Ordering::Relaxed);

  // What is happening wins over what is not: recording is paused for as long
  // as a push-to-talk key is unpressed, so testing it first would show the
  // pause sign through playback, speech and thinking alike.
  //
  // `speak` is voice activity from the recorder, which cannot be current while
  // capture is paused - without that guard a flag left set by the last
  // utterance keeps the microphone up through an idle push-to-talk session.
  //
  // Geometric shapes rather than emoji: a terminal without a colour emoji font
  // draws those in the foreground colour, losing the meaning the colour
  // carried, and their width depends on the font. These are one column
  // everywhere and carry their colour in the escape.
  //
  // No trailing space on any of them - the layout puts one after the icon.
  let status = if play {
    // Playing back. U+FE0E asks for the text form explicitly: the triangle is
    // the base of the play-button emoji, and a renderer that promotes it to
    // the colour font would draw it two columns wide. The other shapes here
    // are not emoji at all, so nothing can promote them.
    "\x1b[32m▶\u{fe0e}\x1b[0m".to_string()
  } else if speak && !recording_paused {
    // capturing a voice: red, blinking, like a recorder
    "\x1b[31m\x1b[5m●\x1b[0m".to_string()
  } else if think {
    // The spinner says "working" on its own; a marker beside it only reads as
    // a second one.
    format!(
      "\x1b[97m{}\x1b[0m",
      spinner[ui_state.spinner_index % spinner.len()]
    )
  } else if recording_paused {
    "\x1b[33m▮▮\x1b[0m".to_string()
  } else {
    // armed and listening, not yet hearing anything
    "\x1b[31m●\x1b[0m".to_string()
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
        "\x1b[44m\x1b[37m DEBATE \x1b[0m {} -- {}",
        agent1_name, agent2_name
      )
    } else {
      format!("\x1b[44m\x1b[37m CONVERSATION \x1b[0m")
    }
  } else {
    format!("\x1b[44m\x1b[37m CONVERSATION \x1b[0m")
  };

  // Both tags are the same width, so the bar does not shift left and right as
  // the state changes; the shorter one is centred in it. "paused" carries the
  // pause glyph in white ahead of the word, no space between them - the
  // padding is what centres the pair, not a gap inside it.
  let recording_paused_str = if recording_paused {
    // Plain ASCII: no glyph-shape ambiguity, no emoji form to guard against,
    // in every font on every terminal.
    format!("\x1b[43m\x1b[30m{}\x1b[0m", centred(">paused", TAG_WIDTH))
  } else {
    format!("\x1b[41m\x1b[37m{}\x1b[0m", centred("recording", TAG_WIDTH))
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
    "\x1b[42m\x1b[30m LIVE \x1b[0m"
  };

  let lang_guard = state.language.lock().unwrap();
  // Dimmed so it reads as a label on the agent rather than part of the name.
  let agent_display = format!("\x1b[90m{}\x1b[0m {}", lang_code(&lang_guard), agent_name);
  let combined_status = if debate_enabled {
    format!("{} {} {} ", mode, ptt, internal_status)
  } else {
    format!("{} {} {} {} ", mode, ptt, agent_display, internal_status)
  };

  let cols = crossterm::terminal::size().unwrap_or((80, 24)).0 as usize;

  // Room for the meter: the width less everything that is not it or padding.
  // The two counted columns are the spaces the layout below puts after the
  // status icon and after the speed.
  let available = cols.saturating_sub(
    visible_columns(&status)
      + 1
      + visible_columns(&speed_str)
      + 1
      + visible_columns(&combined_status)
      + visible_columns(&recording_paused_str),
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

  let layout = bar_layout(
    &status,
    &bar,
    &speed_str,
    &combined_status,
    &recording_paused_str,
    cols,
  );

  if let Ok(mut st) = status_line.lock() {
    *st = layout.full.clone();
  }

  // Auto-wrap off for the write (DECAWM, CSI ?7l/?7h). A terminal that draws a
  // glyph wider than measured would otherwise push the bar past the last
  // column, wrap it, and scroll the screen every 10ms; with auto-wrap off it
  // clips at the edge instead, costing at most a few cut characters.
  execute!(
    out,
    MoveTo(0, y),
    ResetColor,
    Clear(ClearType::CurrentLine),
    Print("\x1b[?7l"),
    Print(&layout.left),
    MoveTo(layout.right_x as u16, y),
    Print(&layout.right),
    Print("\x1b[?7h"),
    ResetColor
  )
  .unwrap();

  let full_bar = layout.full;
  out.flush().unwrap();
  full_bar
}

/// Columns `ch` occupies on screen, from the UAX #11 table terminals use.
///
/// Sequences are the caller's job: a variation selector, or the second half of
/// a flag, is worth nothing on its own and changes what the glyph before it is
/// worth, so callers look ahead (see `width_of`).
fn display_columns(ch: char) -> usize {
  use unicode_width::UnicodeWidthChar;
  ch.width().unwrap_or(0)
}

/// Columns the character at the cursor is worth, given what follows it.
/// A trailing U+FE0F asks for emoji presentation, which is two cells wide.
fn width_of(ch: char, next: Option<&char>) -> usize {
  if next == Some(&'\u{fe0f}') {
    2
  } else {
    display_columns(ch)
  }
}

/// Columns `s` occupies on screen, escape sequences costing nothing.
fn visible_columns(s: &str) -> usize {
  let mut cols = 0usize;
  let mut chars = s.chars().peekable();
  while let Some(ch) = chars.next() {
    if ch == '\u{1b}' {
      if chars.peek() == Some(&'[') {
        chars.next();
        for c in chars.by_ref() {
          if ('@'..='~').contains(&c) {
            break;
          }
        }
      } else {
        chars.next();
      }
      continue;
    }
    cols += width_of(ch, chars.peek());
  }
  cols
}

/// Cut `s` to at most `max_cols` columns, keeping escape sequences whole and
/// carrying a reset on the end so a truncated colour cannot leak.
fn fit_to_width(s: &str, max_cols: usize) -> String {
  let mut out = String::with_capacity(s.len());
  let mut cols = 0usize;
  let mut chars = s.chars().peekable();
  let mut truncated = false;
  while let Some(ch) = chars.next() {
    if ch == '\u{1b}' {
      // Copy the whole sequence: it costs no columns and must not be split.
      out.push(ch);
      if chars.peek() == Some(&'[') {
        out.push(chars.next().unwrap());
        for c in chars.by_ref() {
          out.push(c);
          if ('@'..='~').contains(&c) {
            break;
          }
        }
      } else if let Some(c) = chars.next() {
        out.push(c);
      }
      continue;
    }
    let w = width_of(ch, chars.peek());
    if cols + w > max_cols {
      truncated = true;
      break;
    }
    out.push(ch);
    cols += w;
  }
  if truncated {
    out.push_str("\u{1b}[0m");
  }
  out
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
      len += display_columns(c);
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
      ResetColor,
      Clear(ClearType::CurrentLine),
      Print(line)
    )
    .unwrap();
  }
  out.flush().unwrap();
}

fn render_debate_modal<W: Write>(out: &mut W, buffer: &[String]) {
  let state = GLOBAL_STATE.get().expect("AppState not initialized");
  let agents = state.agents();
  let agents = agents.as_slice();
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
      ResetColor,
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
        "\x1b[48;5;234m\x1b[91m▲ Please select two different agents\x1b[0m"
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

/// Switches to the terminal's alternate screen before the first
/// [`render_clone_progress_popup`] call. Cloning redraws the popup on every
/// iteration (up to a couple hundred times); doing that on the primary
/// screen would repeatedly `Clear(ClearType::All)` it, and most terminals
/// push each cleared frame into scrollback rather than overwrite it in
/// place, so scrolling up would show a stack of near-duplicate popups. The
/// alternate screen has no scrollback, so redraws just replace each other.
pub fn open_clone_progress_popup() {
  ON_ALT_SCREEN.store(true, Ordering::Relaxed);
  let mut out = io::stdout();
  execute!(
    out,
    terminal::EnterAlternateScreen,
    Hide,
    Clear(ClearType::All),
    // `All` (CSI 2J) only clears the visible grid; some terminals keep a
    // scrollback for the alternate screen too (or carry over a little of
    // the primary screen's on switching), which then shows stray
    // characters when scrolled into - `Purge` (CSI 3J) clears that.
    Clear(ClearType::Purge)
  )
  .unwrap();
  out.flush().unwrap();
}

/// Modal shown while `--clone-voice` / `--refine-voice` trains a voice
/// (`title` is the caller-built inner title text, e.g. `Cloning voice
/// "myvoice"` or `Cloning voice (refining myvoice)`): same bordered,
/// dark-background popup style as [`render_debate_modal`], listing every
/// cloning stage (done / current / pending) and an overall green progress
/// bar with a step count below it. Meant to be called again on every
/// progress update (it redraws from scratch each time, there is no
/// diffing) - call [`open_clone_progress_popup`] first.
pub fn render_clone_progress_popup(
  title: &str,
  stages: &[crate::tts::supertonic3_tts::CloneStageInfo],
  done_steps: usize,
  total_steps: usize,
  fraction: f64,
) {
  let mut out = io::stdout();
  let (cols, rows) = terminal::size().unwrap_or((80, 24));
  let modal_width = std::cmp::min(56, cols.saturating_sub(4)).max(38);
  let modal_height = 7 + stages.len() as u16;
  let modal_x = cols.saturating_sub(modal_width) / 2;
  let modal_y = rows.saturating_sub(modal_height) / 2;

  execute!(out, Clear(ClearType::All), Clear(ClearType::Purge)).unwrap();

  // Modal background
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

  // Border + title
  execute!(
    out,
    MoveTo(modal_x, modal_y),
    Print(format!(
      "\x1b[48;5;234m\x1b[97m┌{}┐\x1b[0m",
      "─".repeat(modal_width as usize - 2)
    ))
  )
  .unwrap();
  let mut padded_title = format!(" {} ", title);
  if padded_title.len() as u16 > modal_width.saturating_sub(2) {
    padded_title = " Cloning voice ".to_string();
  }
  let title_x = modal_x + (modal_width - padded_title.len() as u16) / 2;
  execute!(
    out,
    MoveTo(title_x, modal_y),
    Print(format!("\x1b[48;5;234m\x1b[97;1m{}\x1b[0m", padded_title))
  )
  .unwrap();

  // One line per stage: a checkmark for done, an arrow for the current one
  // (both with its live iteration count), a dot for pending stages.
  for (i, s) in stages.iter().enumerate() {
    let (marker, marker_fg, name_fg) = if s.done {
      ("✓", "\x1b[32m", "\x1b[97m")
    } else if s.current {
      ("▸", "\x1b[96;1m", "\x1b[97;1m")
    } else {
      ("•", "\x1b[90m", "\x1b[90m")
    };
    let total_str = s.total.map(|t| t.to_string()).unwrap_or_else(|| "?".into());
    execute!(
      out,
      MoveTo(modal_x + 2, modal_y + 2 + i as u16),
      Print(format!(
        "\x1b[48;5;234m{marker_fg}{marker} {name_fg}{name:<13}\x1b[90m {iter:>4}/{total:<4}\x1b[0m",
        marker_fg = marker_fg,
        marker = marker,
        name_fg = name_fg,
        name = s.name,
        iter = s.iteration,
        total = total_str
      ))
    )
    .unwrap();
  }

  // Progress bar: green filled, dark gray empty, percentage on the right
  let bar_y = modal_y + 3 + stages.len() as u16;
  let bar_width = (modal_width as usize).saturating_sub(4 + 5);
  let fraction = fraction.clamp(0.0, 1.0);
  let filled = ((fraction * bar_width as f64).round() as usize).min(bar_width);
  execute!(
    out,
    MoveTo(modal_x + 2, bar_y),
    Print(format!(
      "\x1b[48;5;234m\x1b[32m{}\x1b[90m{}\x1b[97m {:>3}%\x1b[0m",
      "█".repeat(filled),
      "░".repeat(bar_width - filled),
      (fraction * 100.0).round() as u32
    ))
  )
  .unwrap();

  // Total steps, below the bar
  execute!(
    out,
    MoveTo(modal_x + 2, bar_y + 1),
    Print(format!(
      "\x1b[48;5;234m\x1b[90mTotal: {} / {} steps\x1b[0m",
      done_steps, total_steps
    ))
  )
  .unwrap();

  // Bottom border + sides
  execute!(
    out,
    MoveTo(modal_x, modal_y + modal_height - 1),
    Print(format!(
      "\x1b[48;5;234m\x1b[97m└{}┘\x1b[0m",
      "─".repeat(modal_width as usize - 2)
    ))
  )
  .unwrap();
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

/// Leaves the alternate screen opened by [`open_clone_progress_popup`],
/// restoring the cursor and whatever the primary screen held before
/// cloning started, ready for the caller's final success/error message.
pub fn close_clone_progress_popup() {
  ON_ALT_SCREEN.store(false, Ordering::Relaxed);
  let mut out = io::stdout();
  execute!(out, terminal::LeaveAlternateScreen, Show).unwrap();
  out.flush().unwrap();
}

#[cfg(test)]
mod tests {
  use super::*;

  fn wrapped(line: &str, max_width: usize) -> Vec<String> {
    let mut out = Vec::new();
    wrap_line(line, max_width, &mut out);
    out
  }

  /// Visible columns, the measure wrapping has to respect. Written out here
  /// rather than reusing get_visible_len_for so the test does not pass just
  /// because both sides share a bug.
  fn columns(s: &str) -> usize {
    let (mut n, mut state) = (0usize, 0u8);
    for c in s.chars() {
      match state {
        1 => {
          state = if c == '[' { 2 } else { 0 };
          continue;
        }
        2 => {
          if ('@'..='~').contains(&c) {
            state = 0;
          }
          continue;
        }
        _ => {}
      }
      if c == '\u{1b}' {
        state = 1;
        continue;
      }
      n += display_columns(c);
    }
    n
  }

  #[test]
  fn empty_line_still_takes_a_row() {
    assert_eq!(wrapped("", 10), vec![""]);
  }

  #[test]
  fn a_line_that_exactly_fits_does_not_spill() {
    assert_eq!(wrapped("abcde", 5), vec!["abcde"]);
  }

  #[test]
  fn longer_lines_break_at_the_width() {
    assert_eq!(wrapped("abcdefghij", 4), vec!["abcd", "efgh", "ij"]);
  }

  #[test]
  fn colour_codes_take_no_columns_and_are_never_split() {
    // "RED" plus " and " is 8 visible columns; the escapes must not count
    // towards that, and must not be broken across rows.
    let line = "\u{1b}[31mRED\u{1b}[0m and more";
    let rows = wrapped(line, 8);
    for row in &rows {
      assert!(
        columns(row) <= 8,
        "row {:?} is {} columns wide",
        row,
        columns(row)
      );
    }
    // The space at a break is dropped, so rows do not re-join byte for byte.
    // Every escape and every non-space glyph must survive.
    let joined = rows.concat();
    assert!(joined.contains("\u{1b}[31m") && joined.contains("\u{1b}[0m"));
    let visible = |t: &str| t.chars().filter(|c| !c.is_whitespace()).collect::<String>();
    assert_eq!(visible(&joined), visible(line));
  }

  #[test]
  fn words_are_not_split_across_rows() {
    assert_eq!(
      wrapped("the quick brown fox", 10),
      vec!["the quick", "brown fox"]
    );
  }

  #[test]
  fn a_word_that_cannot_fit_a_row_is_split_as_a_last_resort() {
    // A long URL or path would otherwise have nowhere to go.
    assert_eq!(
      wrapped("ab supercalifragilistic", 6),
      vec!["ab", "superc", "alifra", "gilist", "ic"]
    );
  }

  #[test]
  fn a_word_that_does_not_fit_the_rest_of_a_row_starts_the_next_one() {
    assert_eq!(
      wrapped("short unsplittable", 12),
      vec!["short", "unsplittable"]
    );
  }

  #[test]
  fn the_csi_introducer_does_not_end_an_escape() {
    // '[' falls inside the '@'..='~' range that ends a sequence, so a scanner
    // stopping there counts "31m" as three visible columns.
    assert_eq!(wrapped("\u{1b}[31mabcd", 4), vec!["\u{1b}[31mabcd"]);
  }

  #[test]
  fn double_width_glyphs_count_as_two() {
    assert_eq!(wrapped("🎤🎤🎤", 4), vec!["🎤🎤", "🎤"]);
  }

  #[test]
  fn a_wrap_continues_the_logical_line_but_a_newline_does_not() {
    let mut h = History::new();
    h.newline();
    for ch in "ab".chars() {
      h.push_char(ch);
    }
    h.wrap_at_word(); // the screen ran out of room
    h.push_char('c');
    assert_eq!(h.wrapped, vec!["ab", "c"], "two screen rows");
    assert_eq!(h.logical, vec!["abc"], "but one line of text");

    h.newline(); // a real line break
    h.push_char('d');
    assert_eq!(h.logical, vec!["abc", "d"]);
  }

  #[test]
  fn rewrapping_wider_reflows_lines_broken_at_the_old_width() {
    let mut h = History::new();
    h.newline();
    for ch in "The quick brown fox".chars() {
      h.push_char(ch);
      if columns(h.wrapped.last().unwrap()) == 10 {
        h.wrap_at_word();
      }
    }
    assert!(h.wrapped.len() > 1, "was broken up at 10 columns");
    h.rewrap(40);
    assert_eq!(h.wrapped, vec!["The quick brown fox"]);
  }

  #[test]
  fn rewrapping_at_the_same_width_is_stable() {
    let mut h = History::new();
    h.newline();
    for ch in "abcdefghij".chars() {
      h.push_char(ch);
    }
    h.rewrap(4);
    let once = h.wrapped.clone();
    h.rewrap(4);
    assert_eq!(h.wrapped, once);
    assert_eq!(once, vec!["abcd", "efgh", "ij"]);
  }

  #[test]
  fn a_half_streamed_line_survives_a_rewrap() {
    // A resize can land between two chunks of an answer. The half-written
    // line must come back whole, so the next chunk appends to it.
    let mut h = History::new();
    h.newline();
    for ch in "finished line".chars() {
      h.push_char(ch);
    }
    h.newline();
    for ch in "still strea".chars() {
      h.push_char(ch);
    }
    h.rewrap(100);
    assert_eq!(h.wrapped, vec!["finished line", "still strea"]);
    h.push_char('m');
    assert_eq!(h.wrapped.last().unwrap(), "still stream");
    assert_eq!(h.logical.last().unwrap(), "still stream");
  }

  /// Feed text through a History the way handle_line_message does: a '\n'
  /// ends the logical line, running out of room only ends the screen row, and
  /// a finished message leaves one empty line behind it.
  fn print_message(h: &mut History, msg: &str, cols: usize) {
    if h.wrapped.is_empty() {
      h.newline();
    }
    for ch in msg.chars() {
      if ch == '\n' {
        h.newline();
      } else if columns(h.wrapped.last().unwrap()) + 1 > cols {
        h.wrap_at_word();
        h.push_char(ch);
      } else {
        h.push_char(ch);
      }
    }
    h.newline();
  }

  fn blank_rows(h: &History) -> usize {
    h.wrapped.iter().filter(|r| r.trim().is_empty()).count()
  }

  #[test]
  fn narrowing_does_not_add_blank_rows_between_turns() {
    // Re-flowing may add rows of text, never rows of nothing.
    let mut h = History::new();
    for turn in 0..3 {
      print_message(&mut h, &format!("USER said something number {}", turn), 80);
      print_message(
        &mut h,
        &format!(
          "ASSISTANT replied at some length, turn {}, with enough text to wrap once the terminal gets narrow",
          turn
        ),
        80,
      );
    }
    let before = blank_rows(&h);
    h.rewrap(40);
    let after_narrow = blank_rows(&h);
    assert_eq!(
      before, after_narrow,
      "narrowing changed the blank-row count from {} to {}",
      before, after_narrow
    );
    h.rewrap(120);
    assert_eq!(
      before,
      blank_rows(&h),
      "widening changed the blank-row count"
    );
  }

  #[test]
  fn trailing_spaces_do_not_become_a_row_of_their_own() {
    // Padding must not spill onto a blank-looking row when narrowing.
    let mut h = History::new();
    h.newline();
    for ch in "text".chars() {
      h.push_char(ch);
    }
    for _ in 0..40 {
      h.push_char(' ');
    }
    h.rewrap(20);
    assert_eq!(
      h.wrapped.iter().filter(|r| r.trim().is_empty()).count(),
      0,
      "padding made blank rows: {:?}",
      h.wrapped
    );
  }

  #[test]
  fn the_bottom_bar_is_cut_to_the_terminal_width() {
    // One cell of overflow on the last row makes the terminal wrap and scroll.
    let bar = "\u{1b}[31m\u{23f8}\u{fe0f} 1.0x CONVERSATION PTT \u{1f1ec}\u{1f1e7} agent\u{1b}[0m";
    for width in [10usize, 20, 40] {
      let cut = fit_to_width(bar, width);
      let cols: usize = {
        let mut n = 0;
        let mut chars = cut.chars().peekable();
        while let Some(c) = chars.next() {
          if c == '\u{1b}' {
            if chars.peek() == Some(&'[') {
              chars.next();
              for c2 in chars.by_ref() {
                if ('@'..='~').contains(&c2) {
                  break;
                }
              }
            } else {
              chars.next();
            }
            continue;
          }
          n += display_columns(c);
        }
        n
      };
      assert!(
        cols <= width,
        "{} columns at width {}: {:?}",
        cols,
        width,
        cut
      );
    }
  }

  /// The bar as `render_bottom_bar` assembles it, for one combination of
  /// agent, mode and state.
  fn bar_for(agent: &str, lang: &str, ptt: bool, state: &str, cols: usize) -> BarLayout {
    let status = match state {
      "play" => "\x1b[32m\u{25b6}\x1b[0m".to_string(),
      "speak" => "\x1b[31m\x1b[5m\u{25cf}\x1b[0m".to_string(),
      "think" => "\x1b[97m\u{280b}\x1b[0m".to_string(),
      "paused" => "\x1b[33m\u{25ae}\u{25ae}\x1b[0m".to_string(),
      _ => "\x1b[31m\u{25cf}\x1b[0m".to_string(),
    };
    let meter = format!(
      "\x1b[37m{}\x1b[0m",
      "\u{2588}".repeat(if state == "paused" { 0 } else { 12 })
    );
    let speed = "[1.0x]".to_string();
    let mode = "\x1b[44m\x1b[37m CONVERSATION \x1b[0m";
    let ptt_tag = if ptt {
      "\x1b[41m\x1b[37m PTT \x1b[0m"
    } else {
      "\x1b[42m\x1b[30m LIVE \x1b[0m"
    };
    let agent_display = format!("\x1b[90m{}\x1b[0m {}", crate::util::lang_code(lang), agent);
    let internal = "\x1b[97m\u{2588}\x1b[0m".repeat(4);
    let combined = format!("{} {} {} {} ", mode, ptt_tag, agent_display, internal);
    let tag = if state == "paused" {
      format!("\x1b[43m\x1b[30m{}\x1b[0m", centred(">paused", TAG_WIDTH))
    } else {
      format!("\x1b[41m\x1b[37m{}\x1b[0m", centred("recording", TAG_WIDTH))
    };
    bar_layout(&status, &meter, &speed, &combined, &tag, cols)
  }

  #[test]
  fn the_bar_fits_and_stays_right_aligned_in_every_combination() {
    let agents = [
      "a",
      "main agent",
      "an extremely long agent name that will not fit any sane terminal width",
      "\u{65e5}\u{672c}\u{8a9e}\u{30a8}\u{30fc}\u{30b8}\u{30a7}\u{30f3}\u{30c8}", // japanese
      "\u{639}\u{645}\u{64a}\u{644}",                                             // arabic
      "\u{c5d0}\u{c774}\u{c804}\u{d2b8}",                                         // korean
    ];
    let langs = ["en", "es", "ja", "ar", "ko", "zh", "pt-BR", ""];
    let states = ["play", "speak", "think", "paused", "listen"];
    let widths = [20usize, 24, 40, 60, 80, 100, 120, 200];

    for agent in agents {
      for lang in langs {
        for ptt in [true, false] {
          for state in states {
            for cols in widths {
              let l = bar_for(agent, lang, ptt, state, cols);
              let right_w = visible_columns(&l.right);
              let left_w = visible_columns(&l.left);
              let what = format!(
                "agent={:?} lang={:?} ptt={} state={} cols={}",
                agent, lang, ptt, state, cols
              );

              // Right-aligned: the group ends on the last column.
              assert_eq!(l.right_x + right_w, cols, "not flush right: {}", what);
              // Never overruns the row, so it can never wrap.
              assert!(left_w <= l.right_x, "left runs into right: {}", what);
              assert!(
                visible_columns(&l.full) <= cols,
                "line is {} columns in a {} column row: {}",
                visible_columns(&l.full),
                cols,
                what
              );
              // And never leaves the row short on its right-hand side.
              assert_eq!(visible_columns(&l.full), cols, "short line: {}", what);
            }
          }
        }
      }
    }
  }

  #[test]
  fn both_state_tags_are_the_same_width() {
    // Different widths would shift the whole bar sideways on every change of
    // state, since the tag is what the rest is aligned against.
    let paused = centred(">paused", TAG_WIDTH);
    let recording = centred("recording", TAG_WIDTH);
    assert_eq!(visible_columns(&paused), TAG_WIDTH);
    assert_eq!(visible_columns(&recording), TAG_WIDTH);
    assert_eq!(recording, "  recording  ");
    // The pause glyph sits directly against the word, no space between them;
    // padding is what centres the pair in the field.
    assert_eq!(paused, "   >paused   ");
  }

  #[test]
  #[ignore] // swaps the process's raw stdout fd, unsafe alongside other tests
            // writing to stdout in parallel; run alone: cargo test --bin vtmate
            // detach_message -- --ignored
  fn detach_message_leaves_no_stray_bar_after_it() {
    // Reproduces attach::finish()'s real sequence against the real UI thread:
    // let it idle long enough to draw at least one ordinary bar frame, then
    // shut down exactly the way finish() does (UI_SHUTDOWN, send the line,
    // wait for UI_STOPPED), and inspect what actually reached the terminal -
    // not a model of it.
    use std::os::unix::io::AsRawFd;

    UI_SHUTDOWN.store(false, Ordering::Relaxed);
    UI_STOPPED.store(false, Ordering::Relaxed);
    UI_RUNNING.store(false, Ordering::Relaxed);

    let mut fds = [0i32; 2];
    assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
    let (read_fd, write_fd) = (fds[0], fds[1]);
    let saved_stdout = unsafe { libc::dup(std::io::stdout().as_raw_fd()) };
    assert!(saved_stdout >= 0);
    assert_eq!(unsafe { libc::dup2(write_fd, std::io::stdout().as_raw_fd()) }, 1);
    unsafe { libc::close(write_fd) };

    // Drains the pipe concurrently: the UI thread must never block on a full
    // pipe buffer while this test is busy doing something else.
    let captured = std::sync::Arc::new(std::sync::Mutex::new(Vec::<u8>::new()));
    let reader = {
      let captured = captured.clone();
      std::thread::spawn(move || {
        use std::io::Read;
        let mut file = unsafe { <std::fs::File as std::os::unix::io::FromRawFd>::from_raw_fd(read_fd) };
        let mut buf = [0u8; 4096];
        loop {
          match file.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => captured.lock().unwrap().extend_from_slice(&buf[..n]),
          }
        }
      })
    };

    let ui_state = crate::state::UiState {
      thinking: Arc::new(AtomicBool::new(false)),
      playing: Arc::new(AtomicBool::new(false)),
      agent_speaking: Arc::new(AtomicBool::new(false)),
      peak: Arc::new(Mutex::new(0.0)),
      spinner_index: 0,
      // Keeps render_bottom_bar's real early exit path off GLOBAL_STATE, which
      // this test has no session to provide - see render_bottom_bar's guard.
      quiet: false,
    };
    let status_line = Arc::new(Mutex::new(String::new()));
    let (tx_ui, rx_ui) = crossbeam_channel::bounded::<String>(1);
    let history: crate::conversation::ConversationHistory =
      std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));

    // quiet:false is what a real attach session runs with, and that path
    // needs GLOBAL_STATE; the daemon-attach state this loop reads is always
    // present in the real app, so give it the same here.
    let _ = crate::state::GLOBAL_STATE.set(std::sync::Arc::new(crate::state::AppState::new()));

    let handle = spawn_ui_thread(ui_state, status_line, rx_ui, history, true);

    // Let at least one ordinary tick draw a ~normal bar frame before shutdown.
    std::thread::sleep(Duration::from_millis(30));
    // Only what happens from here on is what the final message is
    // answerable for - spawn_ui_thread's own startup does one
    // Clear(ClearType::All) of its own, before this test ever begins, which
    // the checks below must not trip on.
    let mark = captured.lock().unwrap().len();

    // attach::finish()'s exact sequence.
    UI_SHUTDOWN.store(true, Ordering::Relaxed);
    let _ = tx_ui.send("final_line|\n\n[36m•[0m  detached, daemon still running".to_string());
    wait_for_ui_stopped(Duration::from_millis(200));

    handle.join().unwrap();
    unsafe {
      libc::dup2(saved_stdout, std::io::stdout().as_raw_fd());
      libc::close(saved_stdout);
    }
    reader.join().unwrap();

    let bytes = &captured.lock().unwrap()[mark..];
    // Nothing is cleared for this message, matching a non-daemon exit: the
    // history stays exactly as printed, so scrolling up still shows it.
    assert!(!bytes.windows(4).any(|w| w == b"[2J"), "the screen was cleared");
    assert!(
      !bytes.windows(4).any(|w| w == b"[3J"),
      "the scrollback was cleared"
    );
    let text = String::from_utf8_lossy(&bytes);
    let msg_at = text.find("detached, daemon still running").expect("message never printed");
    let after = &bytes[msg_at..];
    assert!(
      !after.windows(b"CONVERSATION".len()).any(|w| w == b"CONVERSATION"),
      "a bar frame was drawn after the final message: {:?}",
      String::from_utf8_lossy(after)
    );
  }

  #[test]
  #[ignore] // swaps the process's raw stdout fd, unsafe alongside other tests
            // writing to stdout in parallel; run alone: cargo test --bin vtmate
            // final_line_wins_over -- --ignored
  fn final_line_wins_over_a_continuing_flood_of_other_messages() {
    // A daemon can keep forwarding content for a moment after Detach is
    // sent, from a reader thread with no idea finish() is about to print its
    // own last line. That thread must never win the race.
    use std::os::unix::io::AsRawFd;

    UI_SHUTDOWN.store(false, Ordering::Relaxed);
    UI_STOPPED.store(false, Ordering::Relaxed);
    UI_RUNNING.store(false, Ordering::Relaxed);

    let mut fds = [0i32; 2];
    assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
    let (read_fd, write_fd) = (fds[0], fds[1]);
    let saved_stdout = unsafe { libc::dup(std::io::stdout().as_raw_fd()) };
    assert!(saved_stdout >= 0);
    assert_eq!(unsafe { libc::dup2(write_fd, std::io::stdout().as_raw_fd()) }, 1);
    unsafe { libc::close(write_fd) };

    let captured = std::sync::Arc::new(std::sync::Mutex::new(Vec::<u8>::new()));
    let reader = {
      let captured = captured.clone();
      std::thread::spawn(move || {
        use std::io::Read;
        let mut file = unsafe { <std::fs::File as std::os::unix::io::FromRawFd>::from_raw_fd(read_fd) };
        let mut buf = [0u8; 4096];
        loop {
          match file.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => captured.lock().unwrap().extend_from_slice(&buf[..n]),
          }
        }
      })
    };

    let ui_state = crate::state::UiState {
      thinking: Arc::new(AtomicBool::new(false)),
      playing: Arc::new(AtomicBool::new(false)),
      agent_speaking: Arc::new(AtomicBool::new(false)),
      peak: Arc::new(Mutex::new(0.0)),
      spinner_index: 0,
      quiet: false,
    };
    let status_line = Arc::new(Mutex::new(String::new()));
    let (tx_ui, rx_ui) = crossbeam_channel::bounded::<String>(1);
    let history: crate::conversation::ConversationHistory =
      std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let _ = crate::state::GLOBAL_STATE.set(std::sync::Arc::new(crate::state::AppState::new()));

    let handle = spawn_ui_thread(ui_state, status_line, rx_ui, history, true);

    // Stands in for attach::run's reader thread: keeps offering ordinary
    // content for as long as the test lets it, on its own schedule, with no
    // knowledge of when (or whether) a final line gets sent.
    let keep_flooding = Arc::new(AtomicBool::new(true));
    let flood = {
      let tx_ui = tx_ui.clone();
      let keep_flooding = keep_flooding.clone();
      std::thread::spawn(move || {
        let mut n = 0u64;
        while keep_flooding.load(Ordering::Relaxed) {
          let _ = tx_ui.try_send(format!("line|flood-message-{}", n));
          n += 1;
          std::thread::sleep(Duration::from_micros(200));
        }
      })
    };

    std::thread::sleep(Duration::from_millis(20));

    UI_SHUTDOWN.store(true, Ordering::Relaxed);
    let _ = tx_ui.send("final_line|\n\nTHE-LAST-LINE".to_string());
    wait_for_ui_stopped(Duration::from_millis(500));

    handle.join().unwrap();
    keep_flooding.store(false, Ordering::Relaxed);
    let _ = flood.join();

    unsafe {
      libc::dup2(saved_stdout, std::io::stdout().as_raw_fd());
      libc::close(saved_stdout);
    }
    reader.join().unwrap();

    let bytes = captured.lock().unwrap().clone();
    let last_line_at = bytes
      .windows(b"THE-LAST-LINE".len())
      .position(|w| w == b"THE-LAST-LINE")
      .expect("final_line never printed");
    let after = &bytes[last_line_at..];
    assert!(
      !after
        .windows(b"flood-message".len())
        .any(|w| w == b"flood-message"),
      "a flooded line was drawn after the final one: {:?}",
      String::from_utf8_lossy(after)
    );
  }


  #[test]
  fn a_final_notice_is_correct_after_a_real_attach_style_rebuild() {
    let ui_state = crate::state::UiState {
      thinking: Arc::new(AtomicBool::new(false)),
      playing: Arc::new(AtomicBool::new(false)),
      agent_speaking: Arc::new(AtomicBool::new(false)),
      peak: Arc::new(Mutex::new(0.0)),
      spinner_index: 0,
      quiet: true,
    };
    let spinner: [&str; 1] = ["*"];
    let status_line = Arc::new(Mutex::new(String::new()));
    let mut buffer = History::new();
    let mut ui_state = ui_state;
    let mut bottom_bar = String::new();
    let mut sink: Vec<u8> = Vec::new();

    // Exactly what redraw_full_history does: rebuild in bulk, then lay out at
    // the current width - the same width finish()'s own message will use.
    let messages = vec![
      crate::conversation::ChatMessage {
        role: "user".to_string(),
        content: "Okay, that's this work.".to_string(),
        agent_name: None,
      },
      crate::conversation::ChatMessage {
        role: "assistant".to_string(),
        content: "It seems like our conversation has just begun!  Could you please elaborate on what kind of work you are referring to?  I am all ears and ready to explore the subject together.".to_string(),
        agent_name: None,
      },
    ];
    rebuild_history(&mut buffer, messages.iter());
    buffer.rewrap(80);

    let rows_before = buffer.wrapped.len();

    let msg = "\n\n\u{1b}[36m\u{2022}\u{1b}[0m  detached, vtmate daemon still running (stop it with `vtmate --daemon-stop`)";
    handle_line_message(
      &mut sink,
      msg,
      &mut buffer,
      &mut ui_state,
      &spinner,
      &status_line,
      &mut bottom_bar,
    );

    let occurrences = buffer.wrapped.iter().filter(|l| l.contains("detached")).count();
    assert_eq!(occurrences, 1, "notice duplicated: {:?}", buffer.wrapped);
    // One blank row, then the notice, then a fresh blank row for the cursor -
    // nothing from the prior conversation gets touched or reprinted.
    assert!(buffer.wrapped[rows_before].is_empty(), "{:?}", buffer.wrapped);
    assert!(
      buffer.wrapped[rows_before + 1].contains("detached"),
      "{:?}",
      buffer.wrapped
    );
    assert!(
      buffer.wrapped.last().unwrap().is_empty(),
      "cursor's row should be blank: {:?}",
      buffer.wrapped
    );
    for (i, l) in buffer.wrapped[..rows_before].iter().enumerate() {
      assert!(
        !l.contains("detached"),
        "row {} of the original conversation was touched: {:?}",
        i,
        buffer.wrapped
      );
    }
  }



  #[test]
  fn on_alt_screen_tracks_entering_and_leaving_a_popup() {
    // terminate() sends LeaveAlternateScreen only when this is true, since
    // sending it with nothing having entered can restore a stale cursor
    // position on some terminals rather than being the no-op it looks like.
    ON_ALT_SCREEN.store(false, Ordering::Relaxed);
    let mut on_alt = false;
    let mut resized = false;
    let mut sink: Vec<u8> = Vec::new();

    open_popup_screen(&mut sink, &mut on_alt, &mut resized);
    assert!(ON_ALT_SCREEN.load(Ordering::Relaxed), "not set after entering");

    let ui_state = crate::state::UiState {
      thinking: Arc::new(AtomicBool::new(false)),
      playing: Arc::new(AtomicBool::new(false)),
      agent_speaking: Arc::new(AtomicBool::new(false)),
      peak: Arc::new(Mutex::new(0.0)),
      spinner_index: 0,
      quiet: true,
    };
    let spinner: [&str; 1] = ["*"];
    let status_line = Arc::new(Mutex::new(String::new()));
    let history = History::new();
    close_popup_screen(&mut sink, &mut on_alt, false, &history, &ui_state, &spinner, &status_line);
    assert!(!ON_ALT_SCREEN.load(Ordering::Relaxed), "still set after leaving");
  }

  #[test]
  fn a_final_notice_does_not_glue_onto_the_last_reply() {
    // Reproduces the reported bug directly: an assistant reply that just
    // finished streaming leaves its last row "active" - no trailing empty
    // row yet, since that only gets pushed once the whole message completes
    // via handle_line_message's own tail. A line message with no leading
    // newline of its own continues that same row instead of starting fresh;
    // finish() must send one, matching "USER interrupted" and "Session
    // restarted", which already do.
    let ui_state = crate::state::UiState {
      thinking: Arc::new(AtomicBool::new(false)),
      playing: Arc::new(AtomicBool::new(false)),
      agent_speaking: Arc::new(AtomicBool::new(false)),
      peak: Arc::new(Mutex::new(0.0)),
      spinner_index: 0,
      quiet: true, // keeps render_bottom_bar off GLOBAL_STATE
    };
    let spinner: [&str; 1] = ["*"];
    let status_line = Arc::new(Mutex::new(String::new()));
    let mut buffer = History::new();
    let mut ui_state = ui_state;
    let mut bottom_bar = String::new();
    let mut sink: Vec<u8> = Vec::new();

    // The tail of a reply that just finished streaming: pushed character by
    // character, same as stream_chunk, with no trailing newline() yet.
    for ch in "...explore the subject together.".chars() {
      buffer.push_char(ch);
    }

    handle_line_message(
      &mut sink,
      "\n\n\u{2022}  detached, daemon still running",
      &mut buffer,
      &mut ui_state,
      &spinner,
      &status_line,
      &mut bottom_bar,
    );

    let glued = buffer
      .wrapped
      .iter()
      .any(|l| l.contains("together.") && l.contains("detached"));
    assert!(!glued, "notice landed on the reply's own row: {:?}", buffer.wrapped);
    assert!(
      buffer.wrapped.iter().any(|l| l.contains("detached")),
      "notice never appeared: {:?}",
      buffer.wrapped
    );
  }

  #[test]
  fn a_mid_stream_shutdown_aborts_the_reveal_promptly() {
    // stream_chunk sleeps CHAR_DELAY_MS per visible character with no other
    // chance to notice UI_SHUTDOWN in between; a long reply must not have to
    // finish its whole reveal before shutdown takes effect.
    UI_SHUTDOWN.store(false, Ordering::Relaxed);
    let ui_state_for_thread = crate::state::UiState {
      thinking: Arc::new(AtomicBool::new(false)),
      playing: Arc::new(AtomicBool::new(false)),
      agent_speaking: Arc::new(AtomicBool::new(false)),
      peak: Arc::new(Mutex::new(0.0)),
      spinner_index: 0,
      // Short-circuits render_bottom_bar before it reaches GLOBAL_STATE,
      // which a unit test has no session to provide.
      quiet: true,
    };
    let spinner: [&str; 1] = ["*"];
    let status_line = Arc::new(Mutex::new(String::new()));
    let long_reply = "word ".repeat(200); // ~1000 chars: seconds of reveal at CHAR_DELAY_MS/char
    let mut buffer = History::new();
    buffer.newline(); // stream_chunk always follows at least one prior line
    let mut ui_state = ui_state_for_thread;
    let mut bottom_bar = String::new();
    let mut sink: Vec<u8> = Vec::new();

    let handle = std::thread::spawn(move || {
      let start = Instant::now();
      stream_chunk(
        &mut sink,
        &long_reply,
        &mut buffer,
        &mut ui_state,
        &spinner,
        &status_line,
        &mut bottom_bar,
      );
      start.elapsed()
    });

    std::thread::sleep(Duration::from_millis(20));
    UI_SHUTDOWN.store(true, Ordering::Relaxed);
    let elapsed = handle.join().unwrap();
    UI_SHUTDOWN.store(false, Ordering::Relaxed);

    // The full reveal would take ~1000 * CHAR_DELAY_MS; returning within a
    // fraction of that is the difference this fix makes.
    assert!(
      elapsed < Duration::from_millis(200),
      "stream_chunk took {:?} to notice shutdown",
      elapsed
    );
  }

  #[test]
  fn every_status_glyph_is_one_column_and_not_an_emoji() {
    // Each is drawn from a font every terminal ships, and none can be promoted
    // to a colour emoji: the only one with an emoji form carries U+FE0E.
    for (what, glyph) in [
      ("play", "\u{25b6}\u{fe0e}"),
      ("record", "\u{25cf}"),
      ("think", "\u{22ef}"),
      ("stop", "\u{25a0}"),
      ("spinner", "\u{280b}"),
    ] {
      assert_eq!(visible_columns(glyph), 1, "{}", what);
    }
    assert_eq!(visible_columns("\u{25ae}\u{25ae}"), 2, "pause");
  }

  #[test]
  fn widths_match_the_terminal_for_any_locale() {
    // The bar carries a flag for the agent's language and the agent's own
    // name, so every script has to measure the way the terminal draws it.
    for flag in [
      "\u{1f1ec}\u{1f1e7}",
      "\u{1f1ea}\u{1f1f8}",
      "\u{1f1ef}\u{1f1f5}",
      "\u{1f1e7}\u{1f1f7}",
    ] {
      assert_eq!(visible_columns(flag), 2, "flag {:?}", flag);
    }
    assert_eq!(visible_columns("\u{ff21}\u{ff22}"), 4, "fullwidth latin");
    assert_eq!(visible_columns("\u{65e5}\u{672c}\u{8a9e}"), 6, "japanese");
    assert_eq!(visible_columns("\u{d55c}\u{ad6d}"), 4, "korean");
    assert_eq!(visible_columns("\u{639}\u{631}\u{628}\u{64a}"), 4, "arabic");
    assert_eq!(visible_columns("agent"), 5, "latin");
    // Combining marks add nothing: "e" plus an acute is still one column.
    assert_eq!(visible_columns("e\u{301}"), 1, "combining acute");
  }

  #[test]
  fn a_bar_ending_in_any_flag_is_cut_to_the_same_width() {
    // Whatever the locale, the tag has to stop at the same column.
    for flag in ["\u{1f1ec}\u{1f1e7}", "\u{1f1ef}\u{1f1f5}"] {
      let bar = format!("\u{23f8}\u{fe0f} {} agent  paused  ", flag);
      let cut = fit_to_width(&bar, 20);
      assert!(
        visible_columns(&cut) <= 20,
        "{:?} measured {} columns",
        cut,
        visible_columns(&cut)
      );
    }
  }

  #[test]
  fn the_audio_meter_is_not_cut_off_the_bar() {
    // The meter is a run of U+2588; counting those as double-width would make
    // the bar measure twice its length and truncate away the meter.
    let meter = "\u{2588}".repeat(40);
    let bar = format!("\u{23f8}\u{fe0f} {} 1.0x CONVERSATION", meter);
    let cut = fit_to_width(&bar, 79);
    assert_eq!(
      cut.matches('\u{2588}').count(),
      40,
      "the whole meter must survive at 79 columns: {:?}",
      cut
    );
    assert!(
      cut.contains("1.0x"),
      "the text after the meter was cut: {:?}",
      cut
    );
  }

  #[test]
  fn block_elements_are_one_column_and_emoji_presentation_is_two() {
    assert_eq!(display_columns('\u{2588}'), 1, "block element");
    assert_eq!(display_columns('\u{2500}'), 1, "box drawing");
    assert_eq!(display_columns('\u{2192}'), 1, "arrow");
    assert_eq!(display_columns('\u{23f8}'), 1, "pause sign on its own");
    assert_eq!(display_columns('\u{fe0f}'), 0, "variation selector");
    assert_eq!(display_columns('\u{1f50a}'), 2, "speaker emoji");
    // "pause + variation selector" is two columns; fit_to_width looks ahead.
    assert_eq!(
      fit_to_width("\u{23f8}\u{fe0f}ab", 2),
      "\u{23f8}\u{fe0f}\u{1b}[0m"
    );
  }

  #[test]
  fn truncating_the_bar_closes_any_colour_it_cut() {
    let cut = fit_to_width("\u{1b}[31mred text that is long", 5);
    assert!(cut.ends_with("\u{1b}[0m"), "got {:?}", cut);
  }

  #[test]
  fn a_bar_that_already_fits_is_left_alone() {
    assert_eq!(fit_to_width("abc", 10), "abc");
  }

  #[test]
  fn repainting_never_clears_the_whole_screen() {
    // On the primary screen these push the frame they clear into the
    // scrollback, leaving copies of the bottom bar in the history.
    let mut h = History::new();
    for word in ["one", "two"] {
      h.newline();
      for ch in word.chars() {
        h.push_char(ch);
      }
    }
    let mut sink: Vec<u8> = Vec::new();
    paint_viewport(&mut sink, &h);
    let out = String::from_utf8(sink).unwrap();
    assert!(
      !out.contains("\u{1b}[2J"),
      "must not clear the whole screen"
    );
    assert!(!out.contains("\u{1b}[3J"), "must not clear the scrollback");
    assert!(out.contains("one") && out.contains("two"));
  }

  #[test]
  fn repainting_blanks_the_rows_the_history_does_not_reach() {
    // A short history must not leave a popup's lower half showing below it.
    let mut h = History::new();
    h.newline();
    for ch in "only line".chars() {
      h.push_char(ch);
    }
    let mut sink: Vec<u8> = Vec::new();
    paint_viewport(&mut sink, &h);
    let out = String::from_utf8(sink).unwrap();
    let (_, visible) = viewport(h.wrapped.len(), terminal::size().unwrap_or((80, 24)).1);
    for y in 1..visible {
      let at = format!("\u{1b}[{};1H", y + 1);
      assert!(out.contains(&at), "row {} was left untouched", y);
    }
  }

  #[test]
  fn rebuilding_a_long_history_produces_role_labels_and_content_as_separate_lines() {
    // Long enough to run past one screenful: ScrollUp only fires once a
    // rebuild needs more rows than the terminal can show at once.
    let messages: Vec<crate::conversation::ChatMessage> = (0..30)
      .map(|i| crate::conversation::ChatMessage {
        role: if i % 2 == 0 { "user".to_string() } else { "assistant".to_string() },
        content: format!("turn number {}", i),
        agent_name: None,
      })
      .collect();
    let mut h = History::new();
    rebuild_history(&mut h, messages.iter());
    h.rewrap(80);

    assert!(
      h.wrapped.iter().any(|l| l.contains("USER")),
      "no USER label in {:?}",
      h.wrapped
    );
    assert!(
      h.wrapped.iter().any(|l| l.contains("ASSISTANT")),
      "no ASSISTANT label in {:?}",
      h.wrapped
    );
    for i in 0..30 {
      let needle = format!("turn number {}", i);
      assert!(
        h.wrapped.iter().any(|l| l.contains(&needle)),
        "missing {:?}",
        needle
      );
    }
    // More rows than one typical terminal height, where ScrollUp fires for
    // every line past the first screen.
    assert!(h.wrapped.len() > 24, "only {} rows", h.wrapped.len());
  }

  #[test]
  fn rebuilt_messages_have_a_blank_row_between_them() {
    let messages: Vec<crate::conversation::ChatMessage> = vec![
      crate::conversation::ChatMessage {
        role: "user".to_string(),
        content: "hi".to_string(),
        agent_name: None,
      },
      crate::conversation::ChatMessage {
        role: "assistant".to_string(),
        content: "hello".to_string(),
        agent_name: None,
      },
    ];
    let mut h = History::new();
    rebuild_history(&mut h, messages.iter());
    let user_row = h.wrapped.iter().position(|l| l.contains("USER")).unwrap();
    let assistant_row = h
      .wrapped
      .iter()
      .position(|l| l.contains("ASSISTANT"))
      .unwrap();
    // USER, "hi", a blank row, then ASSISTANT: four rows apart, and the row
    // in between must actually be blank, not carrying the next label already.
    assert_eq!(
      assistant_row - user_row,
      3,
      "rows: {:?}",
      h.wrapped
    );
    assert!(
      h.wrapped[assistant_row - 1].trim().is_empty(),
      "row before ASSISTANT should be blank: {:?}",
      h.wrapped[assistant_row - 1]
    );
  }

  #[test]
  fn rebuilding_history_replaces_rather_than_appends() {
    let first: Vec<crate::conversation::ChatMessage> = vec![crate::conversation::ChatMessage {
      role: "user".to_string(),
      content: "first session".to_string(),
      agent_name: None,
    }];
    let second: Vec<crate::conversation::ChatMessage> = vec![crate::conversation::ChatMessage {
      role: "user".to_string(),
      content: "second session".to_string(),
      agent_name: None,
    }];
    let mut h = History::new();
    rebuild_history(&mut h, first.iter());
    rebuild_history(&mut h, second.iter());
    assert!(!h.wrapped.iter().any(|l| l.contains("first session")));
    assert!(h.wrapped.iter().any(|l| l.contains("second session")));
  }

  #[test]
  fn the_viewport_redraw_paints_the_tail_of_the_history_from_row_zero() {
    // The contract the resize and close-popup paths depend on.
    let mut h = History::new();
    for word in ["one", "two", "three"] {
      h.newline();
      for ch in word.chars() {
        h.push_char(ch);
      }
    }
    let mut sink: Vec<u8> = Vec::new();
    redraw_buffer(&mut sink, &h.wrapped);
    let out = String::from_utf8(sink).unwrap();
    for (row, word) in ["one", "two", "three"].iter().enumerate() {
      // crossterm writes MoveTo(col, row) as CSI row+1 ; col+1 H
      let at = format!("\u{1b}[{};1H", row + 1);
      let i = out
        .find(&at)
        .unwrap_or_else(|| panic!("no move to row {}", row));
      assert!(
        out[i..].contains(word),
        "row {} should carry {:?}",
        row,
        word
      );
    }
  }
}
