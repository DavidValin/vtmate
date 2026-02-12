use sha2::Digest;
use std::env;
use std::fs;
use std::path::Path;
use std::process::Command;

// Map file names to hardcoded URLs
fn find_url_for_file(file_name: &str) -> Option<String> {
  match file_name {
    "ggml-small.bin" => {
      Some("https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.bin".to_string())
    }
    "0.onnx" => {
      Some("https://github.com/DavidValin/kokoro-tiny/raw/main/models/0.onnx".to_string())
    }
    "0.bin" => Some("https://github.com/DavidValin/kokoro-tiny/raw/main/models/0.bin".to_string()),
    _ => None,
  }
}

fn main() {
  // Directory where Cargo will place build artifacts
  let out_dir = env::var("OUT_DIR").expect("OUT_DIR not set");
  let dest = Path::new(&out_dir).join("embedded");
  fs::create_dir_all(&dest).expect("Failed to create embedded dir");

  // Files to copy: (source_path, file_name)
  let files = [
    (".cache/k/0.bin", "0.bin"),
    (".cache/k/0.onnx", "0.onnx"),
    (".whisper-models/ggml-small.bin", "ggml-small.bin"),
  ];

  // Try to find the files locally, if not found, download them remotely
  // and validate checksums
  for (src_rel, name) in &files {
    let src = Path::new(&env::var("HOME").unwrap()).join(src_rel);
    let exists = src.exists();
    let mut local_checksum_ok = false;
    if !exists {
      println!("cargo:warning=File {} missing at {}", name, src.display());
    } else {
      // Compute checksum of local file
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
    }
    let should_download = !exists || !local_checksum_ok;
    if should_download {
      if let Some(url) = find_url_for_file(name) {
        println!("cargo:warning=Downloading {} from {}", name, url);
        let dest_path = dest.join(name);
        let _ = fs::create_dir_all(dest_path.parent().unwrap());
        let _ = Command::new("curl")
          .args(&["-L", "-o", dest_path.to_str().unwrap(), &url])
          .output();
      }
    } else {
      // Copy local source to dest if not already
      let dest_path = dest.join(name);
      let _ = fs::copy(&src, &dest_path).unwrap_or_else(|_| 0);
    }
  }

  // Verify all files were copied successfully and validate checksums
  for (_, name) in &files {
    let dst = dest.join(name);
    if !dst.exists() {
      continue; // skip if asset missing
    }
    // Compute SHA256
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

  println!("cargo:warning=Downloaded assets to {}", dest.display());

  // Tell Cargo to rerun if any of the source files change
  for (_, name) in &files {
    let src = Path::new(&env::var("HOME").unwrap()).join(name);
    println!("cargo:rerun-if-changed={}", src.display());
  }
}

// Expected SHA256 hashes for assets
use std::collections::HashMap;

fn init_expected_hashes() -> HashMap<&'static str, &'static str> {
  let mut m = HashMap::new();
  m.insert(
    "ggml-small.bin",
    "1be3a9b2063867b937e64e2ec7483364a79917e157fa98c5d94b5c1fffea987b",
  );
  m.insert(
    "0.bin",
    "bca610b8308e8d99f32e6fe4197e7ec01679264efed0cac9140fe9c29f1fbf7d",
  );
  m.insert(
    "0.onnx",
    "7d5df8ecf7d4b1878015a32686053fd0eebe2bc377234608764cc0ef3636a6c5",
  );
  m
}

static EXPECTED_HASHES: once_cell::sync::Lazy<HashMap<&'static str, &'static str>> =
  once_cell::sync::Lazy::new(init_expected_hashes);
