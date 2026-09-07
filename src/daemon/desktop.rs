// ------------------------------------------------------------------
//  Desktop integration: selected text, clipboard paste, modifier state
// ------------------------------------------------------------------

#[cfg(not(target_os = "linux"))]
use enigo::{Direction, Key, Keyboard};
use enigo::{Enigo, Settings};
use std::time::{Duration, Instant};

// API
// ------------------------------------------------------------------

/// Longest selection appended to a turn / read aloud.
pub const MAX_SELECTION_CHARS: usize = 20_000;

/// Owned and used by the daemon controller thread only. Kept alive for the
/// whole daemon life: on Linux the clipboard content we set is served by
/// this process while the `Clipboard` lives.
pub struct Desktop {
  clipboard: Option<arboard::Clipboard>,
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
    Self { clipboard, enigo }
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
    std::thread::sleep(Duration::from_millis(40));
    self.send_combo('v');
    std::thread::sleep(Duration::from_millis(250));
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
