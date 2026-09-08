// ------------------------------------------------------------------
//  Desktop integration: selected text, clipboard paste, modifier state
// ------------------------------------------------------------------

#[cfg(not(target_os = "linux"))]
use enigo::{Direction, Key, Keyboard};
use enigo::{Enigo, Settings};
#[cfg(target_os = "linux")]
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

// API
// ------------------------------------------------------------------

/// When the current selection was made, as far as vtmate can tell.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum SelectionAge {
  /// There is no way to date selections here: not X11, or no XFixes. Callers
  /// fall back to comparing the text itself.
  Unknown,
  /// Nothing has been selected since vtmate started watching, so whatever the
  /// selection holds was left there earlier and is not for this message.
  Older,
  /// The X server's timestamp for the current selection.
  At(u32),
}

/// Longest selection appended to a turn / read aloud.
pub const MAX_SELECTION_CHARS: usize = 20_000;

/// How long the dictated text stays on the clipboard after the paste
/// keystroke, before the previous content is restored.
const CLIPBOARD_HOLD_MS: u64 = 1200;

/// Owned and used by the daemon controller thread only. Kept alive for the
/// whole daemon life: on Linux the clipboard content we set is served by
/// this process while the `Clipboard` lives.
pub struct Desktop {
  clipboard: Option<arboard::Clipboard>,
  /// X server timestamp of the current selection, kept up to date by the
  /// XFixes watcher; `None` when nothing is selected or it was selected
  /// before vtmate started.
  #[cfg(target_os = "linux")]
  selection_stamp: Arc<Mutex<SelectionAge>>,
  #[cfg_attr(target_os = "linux", allow(dead_code))]
  enigo: Option<Enigo>,
}

impl Desktop {
  pub fn new() -> Self {
    let clipboard = match arboard::Clipboard::new() {
      Ok(c) => Some(c),
      Err(e) => {
        crate::log::log("warning", &format!("clipboard unavailable: {}", e));
        None
      }
    };
    let enigo = match Enigo::new(&Settings::default()) {
      Ok(e) => Some(e),
      Err(e) => {
        crate::log::log("warning", &format!("input simulation unavailable: {}", e));
        None
      }
    };
    #[cfg(target_os = "linux")]
    let selection_stamp = {
      let slot = Arc::new(Mutex::new(SelectionAge::Older));
      spawn_selection_watch(slot.clone());
      slot
    };
    Self {
      clipboard,
      enigo,
      #[cfg(target_os = "linux")]
      selection_stamp,
    }
  }

  /// Identity of the current selection: the X server time at which it was
  /// made. It changes every time the user selects something, including
  /// re-selecting the same words, which is what tells a selection made for
  /// this message apart from one left over from an earlier one.
  ///
  /// `None` when it cannot be known (no X11, or an owner that does not answer
  /// the TIMESTAMP request every selection owner is supposed to answer);
  /// callers then fall back to comparing the text itself.
  pub fn selection_stamp(&self) -> SelectionAge {
    #[cfg(target_os = "linux")]
    {
      self
        .selection_stamp
        .lock()
        .map(|s| *s)
        .unwrap_or(SelectionAge::Unknown)
    }
    #[cfg(not(target_os = "linux"))]
    {
      SelectionAge::Unknown
    }
  }

  /// Whether text is selected right now. `false` means the selection was
  /// dropped (nothing owns it) and the stale text it held must not be used.
  ///
  /// Asked of the X server at the moment of the call rather than tracked in
  /// the background, so it cannot go stale. The owning window is logged, so
  /// `--verbose` shows which application still claims a selection when one
  /// is attached unexpectedly.
  ///
  /// Windows / macOS capture the selection with a copy shortcut at the
  /// moment of the request, so it is always what is highlighted right now.
  pub fn selection_present(&self) -> bool {
    #[cfg(target_os = "linux")]
    {
      match x11_primary_owner() {
        Ok(None) => false,
        Ok(Some(owner)) => {
          crate::log::log("debug", &format!("selection owned by {}", owner));
          true
        }
        Err(e) => {
          crate::log::log("debug", &format!("cannot read the selection owner: {}", e));
          true // no answer from X: fall back to reading the selection
        }
      }
    }
    #[cfg(not(target_os = "linux"))]
    {
      true
    }
  }

  /// The text currently selected anywhere on the desktop, trimmed; `None`
  /// when nothing is selected or the desktop is not reachable.
  pub fn read_selection(&mut self) -> Option<String> {
    let sel = self.read_selection_raw()?;
    let sel = sel.trim();
    if sel.is_empty() {
      return None;
    }
    if sel.chars().count() > MAX_SELECTION_CHARS {
      crate::log::log(
        "warning",
        &format!(
          "selection longer than {} characters, truncated",
          MAX_SELECTION_CHARS
        ),
      );
      return Some(sel.chars().take(MAX_SELECTION_CHARS).collect());
    }
    Some(sel.to_string())
  }

  /// Linux (X11): the PRIMARY selection, no keystrokes involved.
  #[cfg(target_os = "linux")]
  fn read_selection_raw(&mut self) -> Option<String> {
    use arboard::{GetExtLinux, LinuxClipboardKind};
    let cb = self.clipboard.as_mut()?;
    match cb.get().clipboard(LinuxClipboardKind::Primary).text() {
      Ok(t) => Some(t),
      Err(arboard::Error::ContentNotAvailable) => None,
      Err(e) => {
        crate::log::log("debug", &format!("primary selection: {}", e));
        None
      }
    }
  }

  /// Windows / macOS: simulate the copy shortcut, read the clipboard, put
  /// the previous text back.
  #[cfg(not(target_os = "linux"))]
  fn read_selection_raw(&mut self) -> Option<String> {
    let _ = wait_modifiers_released(Duration::from_millis(750));
    let old = self.clipboard.as_mut().and_then(|c| c.get_text().ok());
    if let Some(cb) = self.clipboard.as_mut() {
      let _ = cb.clear();
    }
    self.send_combo('c');
    let mut captured: Option<String> = None;
    let deadline = Instant::now() + Duration::from_millis(400);
    while Instant::now() < deadline {
      std::thread::sleep(Duration::from_millis(20));
      if let Some(cb) = self.clipboard.as_mut() {
        if let Ok(t) = cb.get_text() {
          if !t.is_empty() {
            captured = Some(t);
            break;
          }
        }
      }
    }
    if let (Some(cb), Some(old)) = (self.clipboard.as_mut(), old) {
      let _ = cb.set_text(old);
    }
    captured
  }

  /// Put `text` on the clipboard, paste it at the cursor, restore the
  /// previous clipboard text.
  pub fn paste_text(&mut self, text: &str) {
    let _ = wait_modifiers_released(Duration::from_millis(1000));
    let old = self.clipboard.as_mut().and_then(|c| c.get_text().ok());
    match self.clipboard.as_mut() {
      Some(cb) => {
        if let Err(e) = cb.set_text(text.to_string()) {
          crate::log::log("error", &format!("clipboard write failed: {}", e));
          return;
        }
      }
      None => {
        crate::log::log("error", "clipboard unavailable, cannot paste");
        return;
      }
    }
    // Taking ownership of the clipboard happens on another thread, and the
    // application we paste into asks whoever owns it at that moment. Pasting
    // too early hands it the text of the previous owner, so wait until the
    // clipboard really reads back as ours.
    let deadline = Instant::now() + Duration::from_millis(1500);
    let mut owned = false;
    while Instant::now() < deadline {
      if self
        .clipboard
        .as_mut()
        .and_then(|c| c.get_text().ok())
        .as_deref()
        == Some(text)
      {
        owned = true;
        break;
      }
      std::thread::sleep(Duration::from_millis(20));
    }
    if !owned {
      crate::log::log(
        "warning",
        "the clipboard did not accept the dictated text; pasting anyway",
      );
    }
    self.send_combo('v');
    // The text is fetched lazily, after the keystroke arrives: put the old
    // clipboard back only once the application has had time to ask for it.
    std::thread::sleep(Duration::from_millis(CLIPBOARD_HOLD_MS));
    if let (Some(cb), Some(old)) = (self.clipboard.as_mut(), old) {
      let _ = cb.set_text(old);
    }
  }

  /// Ctrl+<key>. Injected with XTEST exactly like xdotool does (events come
  /// from the XTEST slave device), which every X11 application accepts.
  #[cfg(target_os = "linux")]
  fn send_combo(&mut self, key: char) {
    if let Err(e) = x11_send_ctrl_combo(key) {
      crate::log::log("error", &format!("cannot inject ctrl+{}: {}", key, e));
    }
  }

  /// Ctrl+<key> (Cmd+<key> on macOS).
  #[cfg(not(target_os = "linux"))]
  fn send_combo(&mut self, key: char) {
    let Some(enigo) = self.enigo.as_mut() else {
      crate::log::log("error", "input simulation unavailable");
      return;
    };
    #[cfg(target_os = "macos")]
    let modifier = Key::Meta;
    #[cfg(not(target_os = "macos"))]
    let modifier = Key::Control;
    let _ = enigo.key(modifier, Direction::Press);
    let _ = enigo.key(Key::Unicode(key), Direction::Click);
    let _ = enigo.key(modifier, Direction::Release);
  }
}

/// Show a desktop notification (best effort, never blocks): `notify-send`
/// on Linux, Notification Center on macOS, a toast on Windows.
pub fn notify(title: &str, body: &str) {
  use std::process::{Command, Stdio};
  #[cfg(target_os = "linux")]
  let mut cmd = {
    let mut c = Command::new("notify-send");
    c.args(["-a", "vtmate", "-t", "3000", title, body]);
    c
  };
  #[cfg(target_os = "macos")]
  let mut cmd = {
    let mut c = Command::new("osascript");
    c.args([
      "-e",
      &format!(
        "display notification \"{}\" with title \"{}\"",
        body.replace('"', "'"),
        title.replace('"', "'")
      ),
    ]);
    c
  };
  #[cfg(target_os = "windows")]
  let mut cmd = {
    let script = format!(
      concat!(
        "[Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime] | Out-Null;",
        "$x = [Windows.UI.Notifications.ToastNotificationManager]::GetTemplateContent([Windows.UI.Notifications.ToastTemplateType]::ToastText02);",
        "$t = $x.GetElementsByTagName('text');",
        "$t.Item(0).AppendChild($x.CreateTextNode('{}')) | Out-Null;",
        "$t.Item(1).AppendChild($x.CreateTextNode('{}')) | Out-Null;",
        "$id = '{{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}}\\WindowsPowerShell\\v1.0\\powershell.exe';",
        "[Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier($id).Show([Windows.UI.Notifications.ToastNotification]::new($x))"
      ),
      title.replace('\'', "''"),
      body.replace('\'', "''")
    );
    let mut c = Command::new("powershell");
    c.args(["-NoProfile", "-NonInteractive", "-WindowStyle", "Hidden", "-Command", &script]);
    c
  };
  #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
  let mut cmd = {
    let _ = (title, body);
    return;
  };
  if let Err(e) = cmd
    .stdin(Stdio::null())
    .stdout(Stdio::null())
    .stderr(Stdio::null())
    .spawn()
  {
    crate::log::log("debug", &format!("desktop notification unavailable: {}", e));
  }
}

/// Who owns the PRIMARY selection right now: `None` when nothing does, so
/// nothing is selected anywhere on the desktop. The returned text names the
/// owning window, for the log.
#[cfg(target_os = "linux")]
fn x11_primary_owner() -> Result<Option<String>, String> {
  use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _};
  let (conn, _) = x11rb::connect(None).map_err(|e| e.to_string())?;
  let owner = conn
    .get_selection_owner(u32::from(AtomEnum::PRIMARY))
    .map_err(|e| e.to_string())?
    .reply()
    .map_err(|e| e.to_string())?
    .owner;
  if owner == x11rb::NONE {
    return Ok(None);
  }
  // best effort name, purely for the log
  let name = conn
    .get_property(false, owner, AtomEnum::WM_CLASS, AtomEnum::STRING, 0, 64)
    .ok()
    .and_then(|c| c.reply().ok())
    .map(|r| {
      String::from_utf8_lossy(&r.value)
        .split('\0')
        .find(|s| !s.is_empty())
        .unwrap_or("")
        .to_string()
    })
    .filter(|n| !n.is_empty())
    .unwrap_or_else(|| "an application".to_string());
  Ok(Some(format!("{} (window 0x{:x})", name, owner)))
}

/// Watch selection ownership through XFixes and keep the X server's own
/// timestamp for the current selection. The server stamps every change, so
/// this works with every application, unlike asking the owner (which many
/// answer badly or not at all).
#[cfg(target_os = "linux")]
fn spawn_selection_watch(slot: Arc<Mutex<SelectionAge>>) {
  let _ = std::thread::Builder::new()
    .name("selection-watch".into())
    .spawn(move || {
      use x11rb::connection::Connection;
      use x11rb::protocol::Event;
      use x11rb::protocol::xfixes::{ConnectionExt as _, SelectionEventMask};
      use x11rb::protocol::xproto::AtomEnum;
      let slot_for_failure = slot.clone();
      let disabled = move |why: String| {
        if let Ok(mut s) = slot_for_failure.lock() {
          *s = SelectionAge::Unknown;
        }
        crate::log::log(
          "debug",
          &format!(
            "selection timestamps unavailable ({}): falling back to comparing the text",
            why
          ),
        );
      };
      let (conn, screen_num) = match x11rb::connect(None) {
        Ok(c) => c,
        Err(e) => return disabled(e.to_string()),
      };
      let root = conn.setup().roots[screen_num].root;
      match conn.xfixes_query_version(5, 0).map(|c| c.reply()) {
        Ok(Ok(_)) => {}
        Ok(Err(e)) => return disabled(format!("XFixes: {}", e)),
        Err(e) => return disabled(format!("XFixes: {}", e)),
      }
      let mask = SelectionEventMask::SET_SELECTION_OWNER
        | SelectionEventMask::SELECTION_WINDOW_DESTROY
        | SelectionEventMask::SELECTION_CLIENT_CLOSE;
      match conn
        .xfixes_select_selection_input(root, AtomEnum::PRIMARY.into(), mask)
        .map(|c| c.check())
      {
        Ok(Ok(())) => {}
        Ok(Err(e)) => return disabled(e.to_string()),
        Err(e) => return disabled(e.to_string()),
      }
      loop {
        match conn.wait_for_event() {
          Ok(Event::XfixesSelectionNotify(ev)) => {
            if let Ok(mut s) = slot.lock() {
              // a new selection was made (or the old one dropped)
              *s = if ev.owner == x11rb::NONE {
                SelectionAge::Older
              } else {
                SelectionAge::At(ev.selection_timestamp)
              };
            }
          }
          Ok(_) => {}
          Err(e) => return disabled(e.to_string()),
        }
      }
    });
}

/// Press Control, tap `key`, release Control through the XTEST extension.
#[cfg(target_os = "linux")]
pub fn x11_send_ctrl_combo(key: char) -> Result<(), String> {
  use x11rb::connection::Connection;
  use x11rb::protocol::xproto::{ConnectionExt as _, KEY_PRESS_EVENT, KEY_RELEASE_EVENT};
  use x11rb::protocol::xtest::ConnectionExt as _;
  const XK_CONTROL_L: u32 = 0xffe3;
  let (conn, screen_num) = x11rb::connect(None).map_err(|e| e.to_string())?;
  let setup = conn.setup();
  let root = setup.roots[screen_num].root;
  let (min, max) = (setup.min_keycode, setup.max_keycode);
  let mapping = conn
    .get_keyboard_mapping(min, max - min + 1)
    .map_err(|e| e.to_string())?
    .reply()
    .map_err(|e| e.to_string())?;
  let per = (mapping.keysyms_per_keycode as usize).max(1);
  let find = |keysym: u32| -> Option<u8> {
    mapping
      .keysyms
      .chunks(per)
      .position(|syms| syms.contains(&keysym))
      .map(|i| min + i as u8)
  };
  let ctrl = find(XK_CONTROL_L).ok_or("no keycode for Control_L")?;
  let kc = find(key as u32).ok_or_else(|| format!("no keycode for '{}'", key))?;
  let fake = |kind: u8, keycode: u8| -> Result<(), String> {
    conn
      .xtest_fake_input(kind, keycode, x11rb::CURRENT_TIME, root, 0, 0, 0)
      .map_err(|e| e.to_string())?
      .check()
      .map_err(|e| e.to_string())
  };
  fake(KEY_PRESS_EVENT, ctrl)?;
  fake(KEY_PRESS_EVENT, kc)?;
  fake(KEY_RELEASE_EVENT, kc)?;
  fake(KEY_RELEASE_EVENT, ctrl)?;
  conn.flush().map_err(|e| e.to_string())
}

/// Wait until no modifier key (ctrl, alt, shift, super/cmd) is physically
/// down, so a synthetic Ctrl+C / Ctrl+V is not turned into Ctrl+Alt+C by the
/// keys the user is still releasing after the hotkey. `false` on timeout.
pub fn wait_modifiers_released(timeout: Duration) -> bool {
  let deadline = Instant::now() + timeout;
  loop {
    match modifiers_down() {
      Some(false) | None => return true,
      Some(true) => {}
    }
    if Instant::now() >= deadline {
      crate::log::log("debug", "modifier keys still held, continuing anyway");
      return false;
    }
    std::thread::sleep(Duration::from_millis(15));
  }
}

/// `Some(true)` when a modifier is down, `None` when it cannot be queried.
#[cfg(target_os = "linux")]
fn modifiers_down() -> Option<bool> {
  use x11rb::connection::Connection;
  use x11rb::protocol::xproto::{ConnectionExt, KeyButMask};
  let (conn, screen_num) = x11rb::connect(None).ok()?;
  let root = conn.setup().roots.get(screen_num)?.root;
  let reply = conn.query_pointer(root).ok()?.reply().ok()?;
  let mask = reply.mask;
  let held = mask.contains(KeyButMask::SHIFT)
    || mask.contains(KeyButMask::CONTROL)
    || mask.contains(KeyButMask::MOD1)
    || mask.contains(KeyButMask::MOD4);
  Some(held)
}

#[cfg(target_os = "windows")]
fn modifiers_down() -> Option<bool> {
  use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
  };
  let down = |vk: u16| unsafe { (GetAsyncKeyState(vk as i32) as u16 & 0x8000) != 0 };
  Some(down(VK_CONTROL) || down(VK_MENU) || down(VK_SHIFT) || down(VK_LWIN) || down(VK_RWIN))
}

#[cfg(target_os = "macos")]
fn modifiers_down() -> Option<bool> {
  use objc2_app_kit::{NSEvent, NSEventModifierFlags};
  let flags = NSEvent::modifierFlags_class();
  let held = flags.contains(NSEventModifierFlags::Control)
    || flags.contains(NSEventModifierFlags::Option)
    || flags.contains(NSEventModifierFlags::Shift)
    || flags.contains(NSEventModifierFlags::Command);
  Some(held)
}

#[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
fn modifiers_down() -> Option<bool> {
  None
}
