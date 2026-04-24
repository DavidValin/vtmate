// ------------------------------------------------------------------
//  Router
// ------------------------------------------------------------------

use crate::util::get_user_home_path;
use flate2::read::GzDecoder;
use std::{fs, io, io::Cursor, path::Path};
use tar::Archive;

// API
// ------------------------------------------------------------------

pub fn ensure_piper_espeak_env() {
  // Respect user override
  if std::env::var_os("PIPER_ESPEAKNG_DATA_DIRECTORY").is_some() {
    return;
  }
  let home = match get_user_home_path() {
    Some(h) => h,
    None => return,
  };
  let base = home.join(".vtmate");
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
  let home = match get_user_home_path() {
    Some(h) => h,
    None => return,
  };
  let kokoro_assets_dir = home.join(".cache/k");
  let whisper_dir = home.join(".whisper-models");

  // Check if the expected files already exist
  let bin_path = kokoro_assets_dir.join("0.bin");
  let onnx_path = kokoro_assets_dir.join("0.onnx");
  let whisper_small_path = whisper_dir.join("ggml-small.bin");
  let whisper_tiny_path = whisper_dir.join("ggml-tiny.bin");

  let all_exist = bin_path.exists()
    && onnx_path.exists()
    && whisper_small_path.exists()
    && whisper_tiny_path.exists();

  // When the assets are not present at location, extract them from the binary itself
  // (they are bundled in the binary file. See: embedded_* functions in this file)
  if !all_exist {
    // extract models to disk
    let _ = fs::remove_dir_all(&kokoro_assets_dir);
    let _ = fs::remove_dir_all(&whisper_dir);
    if fs::create_dir_all(&kokoro_assets_dir).is_ok() && fs::create_dir_all(&whisper_dir).is_ok() {
      let _ = fs::write(bin_path, embedded_kokoro_0_bin());
      let _ = fs::write(onnx_path, embedded_kokoro_0_onnx());
      let _ = fs::write(whisper_small_path, embedded_whisper_small());
      let _ = fs::write(whisper_tiny_path, embedded_whisper_tiny());
      // extract supersonic2 files
      let sup_dir = home.join(".vtmate").join("tts");
      if fs::create_dir_all(&sup_dir).is_ok() {
        for rel in SUPERSONIC2_FILES {
          let path = sup_dir.join(rel);
          let _ = fs::write(path, embedded_supersonic2_file(rel));
        }
      }
    }
  }

  unsafe {
    std::env::set_var("KOKORO_TTS_DATA_DIRECTORY", kokoro_assets_dir.as_os_str());
  }
}

// PRIVATE
// ------------------------------------------------------------------

// SUPERSONIC2
// ------------------------------------------------------------------

const SUPERSONIC2_FILES: &[&str] = &[
  "onnx/vector_estimator.onnx",
  "onnx/duration_predictor.onnx",
  "onnx/tts.json",
  "onnx/text_encoder.onnx",
  "onnx/vocoder.onnx",
  "onnx/unicode_indexer.json",
  "config.json",
  "voice_styles/F4.json",
  "voice_styles/F5.json",
  "voice_styles/M1.json",
  "voice_styles/F2.json",
  "voice_styles/F3.json",
  "voice_styles/M4.json",
  "voice_styles/M5.json",
  "voice_styles/F1.json",
  "voice_styles/M2.json",
  "voice_styles/M3.json",
];

pub fn ensure_supersonic2_assets() {
  // Respect user override
  if std::env::var_os("SUPERSONIC2_DATA_DIRECTORY").is_some() {
    return;
  }
  let home = match get_user_home_path() {
    Some(h) => h,
    None => return,
  };
  let base = home.join(".vtmate");
  let sup_dir = base.join("tts/supersonic2-model");

  let mut all_exist = true;
  for rel in SUPERSONIC2_FILES {
    let path = sup_dir.join(rel);
    if !path.exists() {
      all_exist = false;
      break;
    }
  }
  if !all_exist {
    // Extract supersonic2 files from embedded binary
    let _ = fs::remove_dir_all(&sup_dir);
    if fs::create_dir_all(&sup_dir).is_ok() {
      for rel in SUPERSONIC2_FILES {
        let path = sup_dir.join(rel);
        if let Some(parent) = path.parent() {
          let _ = fs::create_dir_all(parent);
        }
        let _ = fs::write(path, embedded_supersonic2_file(rel));
      }
    }
  }

  unsafe {
    std::env::set_var("SUPERSONIC2_DATA_DIRECTORY", sup_dir.as_os_str());
  }
}

// Embedded supersonic2 functions
fn embedded_supersonic2_vector_estimator_onnx() -> &'static [u8] {
  include_bytes!(concat!(
    env!("OUT_DIR"),
    "/embedded/supersonic2-model/onnx/vector_estimator.onnx"
  ))
}
fn embedded_supersonic2_duration_predictor_onnx() -> &'static [u8] {
  include_bytes!(concat!(
    env!("OUT_DIR"),
    "/embedded/supersonic2-model/onnx/duration_predictor.onnx"
  ))
}
fn embedded_supersonic2_tts_json() -> &'static [u8] {
  include_bytes!(concat!(
    env!("OUT_DIR"),
    "/embedded/supersonic2-model/onnx/tts.json"
  ))
}
fn embedded_supersonic2_text_encoder_onnx() -> &'static [u8] {
  include_bytes!(concat!(
    env!("OUT_DIR"),
    "/embedded/supersonic2-model/onnx/text_encoder.onnx"
  ))
}
fn embedded_supersonic2_vocoder_onnx() -> &'static [u8] {
  include_bytes!(concat!(
    env!("OUT_DIR"),
    "/embedded/supersonic2-model/onnx/vocoder.onnx"
  ))
}
fn embedded_supersonic2_unicode_indexer_json() -> &'static [u8] {
  include_bytes!(concat!(
    env!("OUT_DIR"),
    "/embedded/supersonic2-model/onnx/unicode_indexer.json"
  ))
}
fn embedded_supersonic2_config_json() -> &'static [u8] {
  include_bytes!(concat!(
    env!("OUT_DIR"),
    "/embedded/supersonic2-model/config.json"
  ))
}
fn embedded_supersonic2_voice_m1_json() -> &'static [u8] {
  include_bytes!(concat!(
    env!("OUT_DIR"),
    "/embedded/supersonic2-model/voice_styles/M1.json"
  ))
}
fn embedded_supersonic2_voice_m2_json() -> &'static [u8] {
  include_bytes!(concat!(
    env!("OUT_DIR"),
    "/embedded/supersonic2-model/voice_styles/M2.json"
  ))
}
fn embedded_supersonic2_voice_m3_json() -> &'static [u8] {
  include_bytes!(concat!(
    env!("OUT_DIR"),
    "/embedded/supersonic2-model/voice_styles/M3.json"
  ))
}
fn embedded_supersonic2_voice_m4_json() -> &'static [u8] {
  include_bytes!(concat!(
    env!("OUT_DIR"),
    "/embedded/supersonic2-model/voice_styles/M4.json"
  ))
}
fn embedded_supersonic2_voice_m5_json() -> &'static [u8] {
  include_bytes!(concat!(
    env!("OUT_DIR"),
    "/embedded/supersonic2-model/voice_styles/M5.json"
  ))
}
fn embedded_supersonic2_voice_f1_json() -> &'static [u8] {
  include_bytes!(concat!(
    env!("OUT_DIR"),
    "/embedded/supersonic2-model/voice_styles/F1.json"
  ))
}
fn embedded_supersonic2_voice_f2_json() -> &'static [u8] {
  include_bytes!(concat!(
    env!("OUT_DIR"),
    "/embedded/supersonic2-model/voice_styles/F2.json"
  ))
}
fn embedded_supersonic2_voice_f3_json() -> &'static [u8] {
  include_bytes!(concat!(
    env!("OUT_DIR"),
    "/embedded/supersonic2-model/voice_styles/F3.json"
  ))
}
fn embedded_supersonic2_voice_f4_json() -> &'static [u8] {
  include_bytes!(concat!(
    env!("OUT_DIR"),
    "/embedded/supersonic2-model/voice_styles/F4.json"
  ))
}
fn embedded_supersonic2_voice_f5_json() -> &'static [u8] {
  include_bytes!(concat!(
    env!("OUT_DIR"),
    "/embedded/supersonic2-model/voice_styles/F5.json"
  ))
}

fn embedded_supersonic2_file(rel: &str) -> &'static [u8] {
  match rel {
    "onnx/vector_estimator.onnx" => embedded_supersonic2_vector_estimator_onnx(),
    "onnx/duration_predictor.onnx" => embedded_supersonic2_duration_predictor_onnx(),
    "onnx/tts.json" => embedded_supersonic2_tts_json(),
    "onnx/text_encoder.onnx" => embedded_supersonic2_text_encoder_onnx(),
    "onnx/vocoder.onnx" => embedded_supersonic2_vocoder_onnx(),
    "onnx/unicode_indexer.json" => embedded_supersonic2_unicode_indexer_json(),
    "config.json" => embedded_supersonic2_config_json(),
    "voice_styles/M1.json" => embedded_supersonic2_voice_m1_json(),
    "voice_styles/M2.json" => embedded_supersonic2_voice_m2_json(),
    "voice_styles/M3.json" => embedded_supersonic2_voice_m3_json(),
    "voice_styles/M4.json" => embedded_supersonic2_voice_m4_json(),
    "voice_styles/M5.json" => embedded_supersonic2_voice_m5_json(),
    "voice_styles/F1.json" => embedded_supersonic2_voice_f1_json(),
    "voice_styles/F2.json" => embedded_supersonic2_voice_f2_json(),
    "voice_styles/F3.json" => embedded_supersonic2_voice_f3_json(),
    "voice_styles/F4.json" => embedded_supersonic2_voice_f4_json(),
    "voice_styles/F5.json" => embedded_supersonic2_voice_f5_json(),
    _ => panic!("Unknown supersonic2 file {}", rel),
  }
}

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

fn embedded_whisper_tiny() -> &'static [u8] {
  include_bytes!(concat!(env!("OUT_DIR"), "/embedded/ggml-tiny.bin"))
}
