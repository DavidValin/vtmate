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
      "https://github.com/DavidValin/supersonic2-tts/releases/download/1.0.1/supersonic2-model.tgz"
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
  m.insert(
    "duration_predictor.onnx",
    "6d556b3691165c364be91dc0bd894656b5949f5acd2750d8ec2f954010845011",
  );
  m.insert(
    "text_encoder.onnx",
    "dd5f535ed629f7df86071043e15f541ce1b2ab7f1bdbce4c7892b307bca79fa3",
  );
  m.insert(
    "tts.json",
    "ee531d9af9b80438a2ed703e22155ee6c83b12595ab22fd3bb6de94c7502fe96",
  );
  m.insert(
    "unicode_indexer.json",
    "b7662a73a0703f43b97c0f2e089f8e8325e26f5d841aca393b5a54c509c92df1",
  );
  m.insert(
    "vector_estimator.onnx",
    "105e9d66fd8756876b210a6b4aa03fc393b1eaca3a8dadcc8d9a3bc785c86a35",
  );
  m.insert(
    "vocoder.onnx",
    "19bd51f47a186069c752403518a40f7ea4c647455056d2511f7249691ecddf7c",
  );
  m.insert(
    "F1.json",
    "6106950ebeb8a5da29ea22075f605db659cd07dbc288a68292543d9129aa250f",
  );
  m.insert(
    "F2.json",
    "8b97feb16d79ac0447136796708feac5f83dbabe92a5be1168212653c38729ae",
  );
  m.insert(
    "F3.json",
    "7eda5bccb4e6eb7f228fa182462d5fcf982d77628234603599027f0734d70c29",
  );
  m.insert(
    "F4.json",
    "e056fc2bee393edc8bff761eb28f33fb461e8dad828c3b05348a010ac1b7bb79",
  );
  m.insert(
    "F5.json",
    "ce7645ad7e3c13cca04e0d62bf890ef9ac401988005ba8f5e9c9b59257bc6931",
  );
  m.insert(
    "M1.json",
    "a04c823cbda6dd1c7de131ec68fea83bbb70d7f29d61623304eb871e3b83b5a1",
  );
  m.insert(
    "M2.json",
    "7ddd07bf873a3fd67d09ef4e8293b486beb658158b47e371166198e4c6926072",
  );
  m.insert(
    "M3.json",
    "e8e77a56459e4dc8cdfeb88e6f778dc9a0adf22e1184414f4b0e82a5d1edbe72",
  );
  m.insert(
    "M4.json",
    "95322725e4d25d9ed4e7dcccbf0f3726b0e9a2471d876b7942373218dbd30174",
  );
  m.insert(
    "M5.json",
    "be52f82327da63ff18481ce2dd8060c7df432e0168d748745ef3e21b92d706a5",
  );
  m.insert(
    "config.json",
    "1caf87d5df2ed84351c04a3b9f1ce2d5656b109cfdfe0c4d1d1ffdccf0ff1a6f",
  );
  m
}

static EXPECTED_HASHES: Lazy<HashMap<&'static str, &'static str>> = Lazy::new(init_expected_hashes);

fn main() {

  // -----------------------------
  // Optional: Link prebuilt Whisper/GGML/OpenBLAS if available
  // -----------------------------
  if let Ok(lib_dir) = env::var("WHISPER_PREBUILT_LIB") {
    println!("cargo:rerun-if-env-changed=WHISPER_PREBUILT_LIB");
    println!("cargo:rustc-link-search=native={}", lib_dir);
    println!("cargo:rustc-link-lib=static=whisper");
    println!("cargo:rustc-link-lib=static=ggml");
    println!("cargo:rustc-link-lib=static=openblas");
    println!("cargo:rustc-link-lib=pthread");

    let include_dir = Path::new(&lib_dir).join("..").join("include");
    println!("cargo:include={}", include_dir.display());
  } else {
    println!(
      "cargo:warning=WHISPER_PREBUILT_LIB not set, skipping prebuilt Whisper/GGML/OpenBLAS linking"
    );
  }

  // -----------------------------
  // Link built eSpeak NG from PowerShell build
  // -----------------------------
  if let Ok(espeak_dir) = env::var("ESPEAK_NG_DIR") {
    println!("cargo:rerun-if-env-changed=ESPEAK_NG_DIR");

    let espeak_lib_dir = Path::new(&espeak_dir).join("lib");
    println!(
      "cargo:rustc-link-search=native={}",
      espeak_lib_dir.display()
    );
    println!("cargo:rustc-link-lib=static=espeak-ng");

    let espeak_include_dir = Path::new(&espeak_dir).join("include");
    println!("cargo:include={}", espeak_include_dir.display());
  } else {
    println!("cargo:warning=ESPEAK_NG_DIR not set, skipping prebuilt eSpeak NG linking");
  }

  // -----------------------------
  // Optionally link ONNX Runtime
  // -----------------------------
  // Look for ONNX Runtime library location
  if let Ok(ort_lib_dir) = env::var("ORT_LIB_LOCATION") {
    let lib_path = Path::new(&ort_lib_dir);

    // Tell Cargo where to search for native libraries
    println!("cargo:rustc-link-search=native={}", lib_path.display());

    // Iterate over all library files in the directory
    if cfg!(windows) {
      // On Windows, link all .lib files statically
      // for entry in fs::read_dir(lib_path).expect("Failed to read ORT_LIB_LOCATION") {
      //     let entry = entry.expect("Failed to read entry in ORT_LIB_LOCATION");
      //     let path = entry.path();
      //     if let Some(ext) = path.extension() {
      //         if ext == "lib" {
      //             let stem = path.file_stem().unwrap().to_string_lossy();
      //             println!("cargo:rustc-link-lib=static={}", stem);
      //         }
      //     }
      // }
    } else if cfg!(unix) {
      // On Unix/macOS, link all .a (static) or .so/.dylib (dynamic) files
      // for entry in fs::read_dir(lib_path).expect("Failed to read ORT_LIB_LOCATION") {
      //     let entry = entry.expect("Failed to read entry in ORT_LIB_LOCATION");
      //     let path = entry.path();
      //     if let Some(ext) = path.extension() {
      //         match ext.to_str() {
      //             Some("a") => {
      //                 let stem = path.file_stem().unwrap().to_string_lossy();
      //                 println!("cargo:rustc-link-lib=static={}", stem);
      //             }
      //             _ => {}
      //         }
      //     }
      // }
    }

    // Set include path for ONNX Runtime headers
    let ort_include_dir = lib_path.join("..").join("include");
    println!("cargo:include={}", ort_include_dir.display());
  }

  let out_dir = env::var("OUT_DIR").expect("OUT_DIR not set");
  let is_release = env::var("PROFILE").unwrap_or_default() == "release";
  let dest = Path::new(&out_dir).join("embedded");
  fs::create_dir_all(&dest).expect("Failed to create embedded dir");

  let needed_files = [
    (".cache/k/0.bin", "0.bin"),
    (".cache/k/0.onnx", "0.onnx"),
    (".whisper-models/ggml-small.bin", "ggml-small.bin"),
    (".whisper-models/ggml-tiny.bin", "ggml-tiny.bin"),
  ];
  let home = get_home_dir();

  // Check if any supersonic2 files are missing; if so, download and extract the tarball
  const SUPERSONIC2_FILES: &[&str] = &[
    "onnx/duration_predictor.onnx",
    "onnx/text_encoder.onnx",
    "onnx/tts.json",
    "onnx/unicode_indexer.json",
    "onnx/vector_estimator.onnx",
    "onnx/vocoder.onnx",
    "voice_styles/F1.json",
    "voice_styles/F2.json",
    "voice_styles/F3.json",
    "voice_styles/F4.json",
    "voice_styles/F5.json",
    "voice_styles/M1.json",
    "voice_styles/M2.json",
    "voice_styles/M3.json",
    "voice_styles/M4.json",
    "voice_styles/M5.json",
    "config.json",
  ];
  let tarball_name = "supersonic2-model.tgz";
  let mut need_tgz_download = false;
  // Check each expected file; if any are missing, we need to download the tarball
  for rel in SUPERSONIC2_FILES {
    let file_path = Path::new(&home)
      .join(".vtmate")
      .join("tts")
      .join("supersonic2-model")
      .join(rel);
    if !file_path.exists() {
      need_tgz_download = true;
      break;
    }
  }
  let tarball_path = dest.join(tarball_name);
  if need_tgz_download {
    if let Some(url) = find_url_for_file(tarball_name) {
      println!("cargo:warning=Downloading {} from {}", tarball_name, url);
      fs::create_dir_all(tarball_path.parent().unwrap()).unwrap();
      let output = Command::new("curl")
        .args(&["-L", "-o", tarball_path.to_str().unwrap(), &url])
        .output()
        .expect("Failed to run curl");
      if !output.status.success() {
        panic!("Failed to download {}: {:?}", tarball_name, output);
      }
      verify_file(&tarball_path, tarball_name).expect("Checksum mismatch after download");
      extract_supersonic2(&tarball_path);
    }
  }
  // Copy extracted supersonic2 files into embedded dir
  let base = Path::new(&home).join(".vtmate").join("tts");
  let model_dest = dest.join("supersonic2-model");
  fs::create_dir_all(&model_dest).expect("Failed to create model dir");
  let inner = base.join("supersonic2-model");
  copy_dir_all(&inner, &model_dest);

  // Validate checksums of all extracted supersonic2 files (release mode only)
  if is_release {
    for rel in SUPERSONIC2_FILES {
      let path = dest.join("supersonic2-model").join(rel);
      // Use the file name component for lookup in EXPECTED_HASHES
      let name = Path::new(rel).file_name().unwrap().to_str().unwrap();
      if let Err(e) = verify_file(&path, name) {
        panic!("Checksum mismatch for {}: {}", name, e);
      } else {
        println!("cargo:warning=File {} exists and checksum OK", name);
      }
    }
  }

  for &(src_rel, name) in &needed_files {
    if name == tarball_name {
      continue;
    } // skip tarball entry if present
    let src = Path::new(&home).join(src_rel);
    let exists = src.exists();
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

  println!("cargo:warning=Assets copied to {}", dest.display());
  for &(_, name) in &needed_files {
    let src = Path::new(&home).join(name);
    println!("cargo:rerun-if-changed={}", src.display());
  }
}
