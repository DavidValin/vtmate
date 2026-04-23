use flate2::read::GzDecoder;
use hex;
use once_cell::sync::Lazy;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::Path;
use std::process::Command;
use tar::Archive;

// Recursively copy a directory.
fn copy_dir_all(src: &Path, dst: &Path) {
  for entry in fs::read_dir(src).expect("read dir failed") {
    let entry = entry.expect("entry read failed");
    let path = entry.path();
    let dest_path = dst.join(entry.file_name());
    if path.is_dir() {
      fs::create_dir_all(&dest_path).expect("mkdir failed");
      copy_dir_all(&path, &dest_path);
    } else {
      fs::copy(&path, &dest_path).expect("copy failed");
    }
  }
}

// Map file names to hard‑coded URLs.
fn find_url_for_file(file_name: &str) -> Option<String> {
  match file_name {
    "ggml-tiny.bin" => {
      Some("https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.bin".to_string())
    }
    "ggml-small.bin" => {
      Some("https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.bin".to_string())
    }
    "0.onnx" => {
      Some("https://github.com/DavidValin/kokoro-micro/raw/main/models/0.onnx".to_string())
    }
    "0.bin" => Some("https://github.com/DavidValin/kokoro-micro/raw/main/models/0.bin".to_string()),
    "supersonic2-model.tgz" => Some(
      "https://github.com/DavidValin/supersonic2-tts/releases/download/1.0.0/supersonic2-model.tgz"
        .to_string(),
    ),
    _ => None,
  }
}

fn get_home_dir() -> String {
  env::var("HOME")
    .or_else(|_| env::var("USERPROFILE"))
    .expect("Neither HOME nor USERPROFILE environment variable is set")
}

// Verify a file's SHA‑256 hash against the expected value.
fn verify_file(path: &Path, name: &str) -> Result<(), String> {
  let mut file =
    fs::File::open(path).map_err(|e| format!("unable to open {}: {}", path.display(), e))?;
  let mut hasher = Sha256::new();
  std::io::copy(&mut file, &mut hasher)
    .map_err(|e| format!("copy failed for {}: {}", path.display(), e))?;
  let hash = hex::encode(hasher.finalize());
  let expected = EXPECTED_HASHES
    .get(name)
    .ok_or_else(|| format!("unknown file {}", name))?;
  if &hash == expected {
    Ok(())
  } else {
    Err(format!(
      "Checksum mismatch for {}: expected {}, got {}",
      name, expected, hash
    ))
  }
}

// Extract the supersonic2 tarball.
fn extract_supersonic2(tgz_path: &Path) {
  let home = get_home_dir();
  let dest_dir = Path::new(&home).join(".vtmate").join("tts");
  fs::create_dir_all(&dest_dir).expect("Failed to create tts dir");
  let tar_gz = fs::File::open(tgz_path).expect("Failed to open tgz file");
  let decompressor = GzDecoder::new(tar_gz);
  let mut archive = Archive::new(decompressor);
  archive
    .unpack(&dest_dir)
    .expect("Failed to unpack supersonic2 tgz");
}

fn init_expected_hashes() -> HashMap<&'static str, &'static str> {
  let mut m = HashMap::new();
  m.insert(
    "0.bin",
    "bca610b8308e8d99f32e6fe4197e7ec01679264efed0cac9140fe9c29f1fbf7d",
  );
  m.insert(
    "0.onnx",
    "7d5df8ecf7d4b1878015a32686053fd0eebe2bc377234608764cc0ef3636a6c5",
  );
  m.insert(
    "ggml-small.bin",
    "1be3a9b2063867b937e64e2ec7483364a79917e157fa98c5d94b5c1fffea987b",
  );
  m.insert(
    "ggml-tiny.bin",
    "be07e048e1e599ad46341c8d2a135645097a538221678b7acdd1b1919c6e1b21",
  );
  m.insert(
    "supersonic2-model.tgz",
    "db410b2b6e35057e15ed3cbd1432e9a5159746dfa79c9654ac04be6c9a8c312a",
  );
  m
}

static EXPECTED_HASHES: Lazy<HashMap<&'static str, &'static str>> = Lazy::new(init_expected_hashes);

fn main() {
  let out_dir = env::var("OUT_DIR").expect("OUT_DIR not set");
  let is_release = env::var("PROFILE").unwrap_or_default() == "release";
  let dest = Path::new(&out_dir).join("embedded");
  fs::create_dir_all(&dest).expect("Failed to create embedded dir");

  let files = [
    (".cache/k/0.bin", "0.bin"),
    (".cache/k/0.onnx", "0.onnx"),
    (".whisper-models/ggml-small.bin", "ggml-small.bin"),
    (".whisper-models/ggml-tiny.bin", "ggml-tiny.bin"),
    ("", "supersonic2-model.tgz"), // tarball handled specially
  ];
  let home = get_home_dir();

  for &(src_rel, name) in &files {
    let src = Path::new(&home).join(src_rel);
    let exists = src.exists();
    if name == "supersonic2-model.tgz" {
      let dest_path = dest.join(name);
      let mut should_download = !dest_path.exists();
      if !should_download {
        // Verify tarball checksum and presence of extracted files
        if let Ok(()) = verify_file(&dest_path, name) {
          let base = Path::new(&home)
            .join(".vtmate")
            .join("tts")
            .join("supersonic2-model");
          const FILES: &[&str] = &[
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
          for f in FILES {
            if !base.join("supersonic2-model").join(f).exists() {
              should_download = true;
              break;
            }
          }
        } else {
          should_download = true;
        }
      }
      if should_download {
        if let Some(url) = find_url_for_file(name) {
          println!("cargo:warning=Downloading {} from {}", name, url);
          fs::create_dir_all(dest_path.parent().unwrap()).unwrap();
          let output = Command::new("curl")
            .args(&["-L", "-o", dest_path.to_str().unwrap(), &url])
            .output()
            .expect("Failed to run curl");
          if !output.status.success() {
            panic!("Failed to download {}: {:?}", name, output);
          }
          verify_file(&dest_path, name).expect("Checksum mismatch after download");
          extract_supersonic2(&dest_path);
        }
      } else {
        // Ensure extracted files exist
        extract_supersonic2(&dest_path);
      }
      // Copy extracted files for embedding
      let base = Path::new(&home).join(".vtmate").join("tts");
      let model_dest = dest.join("supersonic2-model");
      fs::create_dir_all(&model_dest).expect("Failed to create model dir");
      let inner = base.join("supersonic2-model");
      copy_dir_all(&inner, &model_dest);
      continue;
    }

    if !exists {
      if let Some(url) = find_url_for_file(name) {
        println!("cargo:warning=Downloading {} from {}", name, url);
        let dest_path = dest.join(name);
        fs::create_dir_all(dest_path.parent().unwrap()).unwrap();
        let output = Command::new("curl")
          .args(&["-L", "-o", dest_path.to_str().unwrap(), &url])
          .output()
          .expect("Failed to run curl");
        if !output.status.success() {
          panic!("Failed to download {}: {:?}", name, output);
        }
        if is_release {
          verify_file(&dest_path, name).expect("Checksum mismatch after download");
        }
        // copy to dest (already at dest_path)
        continue;
      } else {
        println!("cargo:warning=File {} missing and no URL found", name);
        continue;
      }
    }

    if is_release {
      match verify_file(&src, name) {
        Ok(_) => println!("cargo:warning=File {} exists and checksum OK", name),
        Err(msg) => println!("cargo:warning={}", msg),
      }
    }

    let dest_path = dest.join(name);
    fs::copy(&src, &dest_path).expect("failed to copy asset");
  }

  println!("cargo:warning=Downloaded assets to {}", dest.display());
  for &(_, name) in &files {
    let src = Path::new(&home).join(name);
    println!("cargo:rerun-if-changed={}", src.display());
  }
}
