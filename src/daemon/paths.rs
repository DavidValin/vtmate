// ------------------------------------------------------------------
//  Daemon runtime files: pid, socket, log, startup error
// ------------------------------------------------------------------

use std::path::PathBuf;

// API
// ------------------------------------------------------------------

/// `~/.vtmate` (created if missing).
pub fn runtime_dir() -> PathBuf {
  let dir = crate::util::get_user_home_path()
    .unwrap_or_else(|| PathBuf::from("."))
    .join(".vtmate");
  let _ = std::fs::create_dir_all(&dir);
  dir
}

pub fn pid_file() -> PathBuf {
  runtime_dir().join("daemon.pid")
}

pub fn log_file() -> PathBuf {
  runtime_dir().join("daemon.log")
}

/// Written by the daemon child when it cannot start (e.g. shortcuts taken),
/// read and printed by the `--daemon` parent.
pub fn start_error_file() -> PathBuf {
  runtime_dir().join("daemon-start.err")
}

/// Unix: path of the socket file. Windows: unused (named pipe).
pub fn socket_file() -> PathBuf {
  runtime_dir().join("daemon.sock")
}

/// Local socket name the daemon listens on and clients connect to.
#[cfg(unix)]
pub fn socket_name() -> std::io::Result<interprocess::local_socket::Name<'static>> {
  use interprocess::local_socket::{GenericFilePath, ToFsName};
  socket_file().to_fs_name::<GenericFilePath>()
}

#[cfg(windows)]
pub fn socket_name() -> std::io::Result<interprocess::local_socket::Name<'static>> {
  use interprocess::local_socket::{GenericNamespaced, ToNsName};
  let user = std::env::var("USERNAME").unwrap_or_else(|_| "user".to_string());
  let user: String = user
    .chars()
    .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
    .collect();
  format!("vtmate-{}", user).to_ns_name::<GenericNamespaced>()
}

pub fn read_pid() -> Option<u32> {
  std::fs::read_to_string(pid_file())
    .ok()
    .and_then(|s| s.trim().parse::<u32>().ok())
}

pub fn write_pid() -> std::io::Result<()> {
  std::fs::write(pid_file(), format!("{}\n", std::process::id()))
}

/// Remove pid, socket and startup-error files (best effort).
pub fn remove_runtime_files() {
  let _ = std::fs::remove_file(pid_file());
  let _ = std::fs::remove_file(start_error_file());
  #[cfg(unix)]
  {
    let _ = std::fs::remove_file(socket_file());
  }
}

/// Is a process with this pid alive?
pub fn pid_alive(pid: u32) -> bool {
  #[cfg(unix)]
  {
    // SAFETY: kill with signal 0 only checks for existence / permission.
    unsafe { libc::kill(pid as i32, 0) == 0 }
  }
  #[cfg(windows)]
  {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{
      GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    const STILL_ACTIVE: u32 = 259;
    unsafe {
      let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
      if h.is_null() {
        return false;
      }
      let mut code: u32 = 0;
      let ok = GetExitCodeProcess(h, &mut code) != 0;
      CloseHandle(h);
      ok && code == STILL_ACTIVE
    }
  }
}
