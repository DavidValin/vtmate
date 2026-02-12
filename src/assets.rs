// ------------------------------------------------------------------
//  Router
// ------------------------------------------------------------------

use flate2::read::GzDecoder;
use std::{fs, io::Cursor, path::PathBuf};
use tar::Archive;

// API
// ------------------------------------------------------------------

pub fn ensure_piper_espeak_env() {
  // Respect user override
  if std::env::var_os("PIPER_ESPEAKNG_DATA_DIRECTORY").is_some() {
    return;
  }
  let home = match std::env::var("HOME") {
    Ok(h) => PathBuf::from(h),
    Err(_) => return,
  };
  let base = home.join(".ai-mate");
  let espeak_dir = base.join("espeak-ng-data");
  let marker = base.join(".espeak_extracted");
  if !(marker.exists() && espeak_dir.is_dir()) {
    let _ = fs::remove_dir_all(&base);
    if fs::create_dir_all(&base).is_ok() {
      let gz = GzDecoder::new(Cursor::new(embedded_espeak_archive()));
      let mut ar = Archive::new(gz);
      if ar.unpack(&base).is_ok() {
        let _ = fs::write(&marker, b"ok");
      }
    }
  }
  unsafe {
    std::env::set_var("PIPER_ESPEAKNG_DATA_DIRECTORY", base.as_os_str());
  }
}

pub fn ensure_assets_env() {
  // Respect user override
  if std::env::var_os("KOKORO_TTS_DATA_DIRECTORY").is_some() {
    return;
  }
  let home = match std::env::var("HOME") {
    Ok(h) => PathBuf::from(h),
    Err(_) => return,
  };
  let kokoro_assets_dir = home.join(".cache/k");
  let whisper_dir = home.join(".whisper-models");
  // Check if the expected files already exist
  let bin_path = kokoro_assets_dir.join("0.bin");
  let onnx_path = kokoro_assets_dir.join("0.onnx");
  let whisper_path = whisper_dir.join("ggml-small.bin");
  let all_exist = bin_path.exists() && onnx_path.exists() && whisper_path.exists();
  if !all_exist {
    println!("Extracting models, one moment...");
    let _ = fs::remove_dir_all(&kokoro_assets_dir);
    let _ = fs::remove_dir_all(&whisper_dir);
    if fs::create_dir_all(&kokoro_assets_dir).is_ok() && fs::create_dir_all(&whisper_dir).is_ok() {
      let _ = fs::write(bin_path, embedded_kokoro_0_bin());
      let _ = fs::write(onnx_path, embedded_kokoro_0_onnx());
      let _ = fs::write(whisper_path, embedded_whisper_small());
    }
  }

  unsafe {
    std::env::set_var("KOKORO_TTS_DATA_DIRECTORY", kokoro_assets_dir.as_os_str());
  }
}

// PRIVATE
// ------------------------------------------------------------------

/// Returns the embedded espeak-ng data archive (tar.gz) as raw bytes.
///
/// The archive file is embedded at compile time.
/// Make sure this path exists when compiling:
///   <crate>/assets/espeak-ng-data.tar.gz
fn embedded_espeak_archive() -> &'static [u8] {
  include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/assets/espeak-ng-data.tar.gz"
  ))
}

fn embedded_kokoro_0_bin() -> &'static [u8] {
  include_bytes!(concat!(env!("OUT_DIR"), "/embedded/0.bin"))
}

fn embedded_kokoro_0_onnx() -> &'static [u8] {
  include_bytes!(concat!(env!("OUT_DIR"), "/embedded/0.onnx"))
}

fn embedded_whisper_small() -> &'static [u8] {
  include_bytes!(concat!(env!("OUT_DIR"), "/embedded/ggml-small.bin"))
}
