// ------------------------------------------------------------------
//  Global shortcuts (global-hotkey) and the platform event loop
// ------------------------------------------------------------------

use crate::config::DaemonSettings;
use global_hotkey::GlobalHotKeyManager;
use global_hotkey::hotkey::HotKey;
use std::str::FromStr;

// API
// ------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hotkey {
  LlmPtt,
  TtsRead,
  PastePtt,
  Reset,
}

struct Entry {
  which: Hotkey,
  setting: &'static str,
  combo: String,
  hotkey: HotKey,
}

pub struct HotkeySet {
  entries: Vec<Entry>,
}

impl HotkeySet {
  pub fn parse(d: &DaemonSettings) -> Result<Self, String> {
    let mut entries = Vec::new();
    let order = [
      Hotkey::LlmPtt,
      Hotkey::TtsRead,
      Hotkey::PastePtt,
      Hotkey::Reset,
    ];
    for (which, (setting, combo)) in order.into_iter().zip(d.combos()) {
      let hotkey = HotKey::from_str(combo).map_err(|e| {
        format!(
          "[daemon] {}: '{}' is not a valid shortcut ({})",
          setting, combo, e
        )
      })?;
      entries.push(Entry {
        which,
        setting,
        combo: combo.to_string(),
        hotkey,
      });
    }
    Ok(Self { entries })
  }

  /// Grab every combo. On failure the list of `(setting, combo)` that could
  /// not be grabbed (already taken by another application, or unsupported)
  /// is returned, after releasing the ones that did succeed.
  pub fn register_all(&self, manager: &GlobalHotKeyManager) -> Result<(), Vec<(String, String)>> {
    let mut taken = Vec::new();
    let mut ok = Vec::new();
    for e in &self.entries {
      match manager.register(e.hotkey) {
        Ok(()) => ok.push(e.hotkey),
        Err(err) => {
          crate::log::log(
            "error",
            &format!("cannot register {} = {}: {}", e.setting, e.combo, err),
          );
          taken.push((e.setting.to_string(), e.combo.clone()));
        }
      }
    }
    if taken.is_empty() {
      Ok(())
    } else {
      for hk in ok {
        let _ = manager.unregister(hk);
      }
      Err(taken)
    }
  }

  pub fn which(&self, id: u32) -> Option<Hotkey> {
    self
      .entries
      .iter()
      .find(|e| e.hotkey.id() == id)
      .map(|e| e.which)
  }

  /// `(setting, combo)` pairs for status output.
  pub fn describe(&self) -> Vec<(String, String)> {
    self
      .entries
      .iter()
      .map(|e| (e.setting.to_string(), e.combo.clone()))
      .collect()
  }
}

/// Run the platform event loop the hotkey backend needs, forever. Called on
/// the main thread after the manager was created there.
#[cfg(target_os = "macos")]
pub fn run_platform_loop() -> ! {
  #[link(name = "CoreFoundation", kind = "framework")]
  unsafe extern "C" {
    fn CFRunLoopRun();
  }
  loop {
    // SAFETY: plain call into CoreFoundation; returns only when the loop is stopped.
    unsafe { CFRunLoopRun() };
    std::thread::sleep(std::time::Duration::from_millis(50));
  }
}

#[cfg(windows)]
pub fn run_platform_loop() -> ! {
  use windows_sys::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetMessageW, MSG, TranslateMessage,
  };
  // SAFETY: standard Win32 message pump on the thread that owns the hotkey window.
  unsafe {
    let mut msg: MSG = std::mem::zeroed();
    while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
      TranslateMessage(&msg);
      DispatchMessageW(&msg);
    }
  }
  crate::util::terminate(0)
}

#[cfg(not(any(target_os = "macos", windows)))]
pub fn run_platform_loop() -> ! {
  // X11: global-hotkey runs its own listener thread; nothing to pump here.
  loop {
    std::thread::park();
  }
}
