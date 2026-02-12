// ------------------------------------------------------------------
//  Router
// ------------------------------------------------------------------

use flate2::read::GzDecoder;
use std::collections::HashMap;
use std::sync::LazyLock;
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

  // SAFETY: called once at program startup before any threads exist
  unsafe {
    std::env::set_var("PIPER_ESPEAKNG_DATA_DIRECTORY", base.as_os_str());
  }
}

pub static _ASSET_FILES: LazyLock<HashMap<&'static str, (&'static str, &'static str)>> =
  LazyLock::new(|| {
    let mut assets = HashMap::new();
    assets.insert(
      "SST::WHISPER::TINY",
      (
        "https://huggingface.co/ggerganov/whisper.cpp/blob/main/ggml-tiny.bin",
        "~/.whisper-models/ggml-tiny.bin",
      ),
    );
    assets.insert(
      "SST::WHISPER::SMALL",
      (
        "https://huggingface.co/ggerganov/whisper.cpp/blob/main/ggml-small.bin",
        "~/.whisper-models/ggml-small.bin",
      ),
    );
    assets.insert(
      "SST::WHISPER::MEDIUM",
      (
        "https://huggingface.co/ggerganov/whisper.cpp/blob/main/ggml-medium.bin",
        "~/.whisper-models/ggml-medium.bin",
      ),
    );
    assets.insert(
      "SST::WHISPER::BASE",
      (
        "https://huggingface.co/ggerganov/whisper.cpp/blob/main/ggml-base.bin",
        "~/.whisper-models/ggml-base.bin",
      ),
    );
    assets.insert(
      "SST::WHISPER::LARGE_V1",
      (
        "https://huggingface.co/ggerganov/whisper.cpp/blob/main/ggml-large-v1.bin",
        "~/.whisper-models/ggml-large-v1.bin",
      ),
    );
    assets.insert(
      "SST::WHISPER::LARGE_V2",
      (
        "https://huggingface.co/ggerganov/whisper.cpp/blob/main/ggml-large-v2.bin",
        "~/.whisper-models/ggml-large-v2.bin",
      ),
    );
    assets.insert(
      "SST::WHISPER::LARGE_V3",
      (
        "https://huggingface.co/ggerganov/whisper.cpp/blob/main/ggml-large-v3.bin",
        "~/.whisper-models/ggml-large-v3.bin",
      ),
    );
    assets.insert(
      "SST::WHISPER::LARGE_V3_TURBO",
      (
        "https://huggingface.co/ggerganov/whisper.cpp/blob/main/ggml-large-v3-turbo.bin",
        "~/.whisper-models/ggml-large-v3-turbo.bin",
      ),
    );
    assets.insert(
      "SST::TTS::KOKORO_TINY::MODEL",
      (
        "https://github.com/8b-is/kokoro-tiny/raw/main/models/0.onnx",
        "~/.cache/.k/0.onnx",
      ),
    );
    assets.insert(
      "SST::TTS::KOKORO_TINY::VOICES",
      (
        "https://github.com/8b-is/kokoro-tiny/raw/main/models/0.bin",
        "~/.cache/.k/0.bin",
      ),
    );
    assets
  });


/// Returns all whisper asset URLs.
///
/// The function filters ASSET_FILES for keys that start with "SST::WHISPER::".
pub fn _get_assets(key_prefix: &str) -> Vec<(&'static str, (&'static str, &'static str))> {
  _ASSET_FILES
    .iter()
    .filter_map(|(k, v)| {
      if k.starts_with(key_prefix) {
        // remove the prefix to return just the asset type key
        let key = k.trim_start_matches(key_prefix);
        Some((key, *v))
      } else {
        None
      }
    })
    .collect()
}

// PRIVATE
// ------------------------------------------------------------------

/// Returns the embedded espeak-ng data archive (tar.gz) as raw bytes.
///
/// The archive file is embedded at compile time.
/// Make sure this path exists when compiling:
///   <crate>/assets/espeak-ng-data.tar.gz
fn embedded_espeak_archive() -> &'static [u8] {
  static ESPEAK_TGZ: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/assets/espeak-ng-data.tar.gz"
  ));
  ESPEAK_TGZ
}
