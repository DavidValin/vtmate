use once_cell::sync::Lazy;
use sha2::Digest;
use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::Path;
use std::process::Command;

// Map file names to hardcoded URLs
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
    _ => None,
  }
}

// Use HOME on Unix, USERPROFILE on Windows
fn get_home_dir() -> String {
    // Use KOKORO_CACHE if set, otherwise fallback to HOME or USERPROFILE
    env::var("KOKORO_CACHE")
        .or_else(|_| env::var("HOME"))
        .or_else(|_| env::var("USERPROFILE"))
        .expect("Neither KOKORO_CACHE, HOME nor USERPROFILE environment variable is set")
}

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
      println!("cargo:warning=WHISPER_PREBUILT_LIB not set, skipping prebuilt Whisper/GGML/OpenBLAS linking");
  }

  // -----------------------------
  // Link built eSpeak NG from PowerShell build
  // -----------------------------
  if let Ok(espeak_dir) = env::var("ESPEAK_NG_DIR") {
      println!("cargo:rerun-if-env-changed=ESPEAK_NG_DIR");

      let espeak_lib_dir = Path::new(&espeak_dir).join("lib");
      println!("cargo:rustc-link-search=native={}", espeak_lib_dir.display());
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

  let files = [
    (".cache/k/0.bin", "0.bin"),
    (".cache/k/0.onnx", "0.onnx"),
    (".whisper-models/ggml-small.bin", "ggml-small.bin"),
    (".whisper-models/ggml-tiny.bin", "ggml-tiny.bin"),
  ];

  let home = get_home_dir();

  for (src_rel, name) in &files {
    let src = Path::new(&home).join(src_rel);
    let exists = src.exists();
    let mut local_checksum_ok = false;

    if exists {
      if is_release {
        let mut file = fs::File::open(&src).expect("unable to open asset");
        let mut hasher = sha2::Sha256::new();
        std::io::copy(&mut file, &mut hasher).expect("copy failed");
        let hash = hex::encode(hasher.finalize());
        let expected = EXPECTED_HASHES.get(name).expect("unknown file");
        if &hash == expected {
          println!("cargo:warning=File {} exists and checksum OK", name);
          local_checksum_ok = true;
        } else {
          println!(
            "cargo:warning=Checksum mismatch for {}: expected {}, got {}",
            name, expected, hash
          );
        }
      } else {
        local_checksum_ok = true;
      }
    } else {
      println!("cargo:warning=File {} missing at {}", name, src.display());
    }

    let should_download = !exists || !local_checksum_ok;

    if should_download {
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
      }
    } else {
      let dest_path = dest.join(name);
      let _ = fs::copy(&src, &dest_path).unwrap_or_else(|_| 0);
    }
  }

  // Validate checksums only in release
  if is_release {
    for (_, name) in &files {
      let dst = dest.join(name);
      if dst.exists() {
        let mut file = fs::File::open(&dst).expect("unable to open asset");
        let mut hasher = sha2::Sha256::new();
        std::io::copy(&mut file, &mut hasher).expect("copy failed");
        let hash = hex::encode(hasher.finalize());
        let expected = EXPECTED_HASHES.get(name).expect("unknown file");
        if &hash != expected {
          panic!(
            "Checksum mismatch for {}: expected {}, got {}",
            name, expected, hash
          );
        }
      }
    }
  }

  println!("cargo:info=Downloaded assets to {}", dest.display());

  for (_, name) in &files {
    let src = Path::new(&home).join(name);
    println!("cargo:rerun-if-changed={}", src.display());
  }
}

// Expected SHA256 hashes for assets
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
  m
}

static EXPECTED_HASHES: Lazy<HashMap<&'static str, &'static str>> = Lazy::new(init_expected_hashes);
