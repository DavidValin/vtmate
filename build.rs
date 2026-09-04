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

// ---------------------------------------------------------------------------
// Supertonic 3 (multilingual TTS) - fetched file by file from Hugging Face
// into $HOME/.vtmate/tts/supertonic-model and copied into OUT_DIR/embedded so
// assets.rs can include_bytes! them, the same way the supersonic2 model is.
// ---------------------------------------------------------------------------
const SUPERTONIC_HF_BASE: &str = "https://huggingface.co/Supertone/supertonic-3/resolve/main";

// (relative path inside the model dir, sha256)
const SUPERTONIC_FILES: &[(&str, &str)] = &[
  ("config.json", "4099082b107a9d4029849ac76b89eca65e03732660969c2babe5bf308c7357f2"),
  ("onnx/duration_predictor.onnx", "c3eb91414d5ff8a7a239b7fe9e34e7e2bf8a8140d8375ffb14718b1c639325db"),
  ("onnx/text_encoder.onnx", "c7befd5ea8c3119769e8a6c1486c4edc6a3bc8365c67621c881bbb774b9902ff"),
  ("onnx/tts.json", "42078d3aef1cd43ab43021f3c54f47d2d75ceb4e75f627f118890128b06a0d09"),
  ("onnx/unicode_indexer.json", "9bf7346e43883a81f8645c81224f786d43c5b57f3641f6e7671a7d6c493cb24f"),
  ("onnx/vector_estimator.onnx", "883ac868ea0275ef0e991524dc64f16b3c0376efd7c320af6b53f5b780d7c61c"),
  ("onnx/vocoder.onnx", "085de76dd8e8d5836d6ca66826601f615939218f90e519f70ee8a36ed2a4c4ba"),
  ("voice_styles/F1.json", "bbdec6ee00231c2c742ad05483df5334cab3b52fda3ba38e6a07059c4563dbc2"),
  ("voice_styles/F2.json", "7c722c6a72707b1a77f035d67f0d1351ba187738e06f7683e8c72b1df3477fc6"),
  ("voice_styles/F3.json", "12f6ef2573baa2defa1128069cb59f203e3ab67c92af77b42df8a0e3a2f7c6ab"),
  ("voice_styles/F4.json", "c2fa764c1225a76dfc3e2c73e8aa4f70d9ee48793860eb34c295fff01c2e032b"),
  ("voice_styles/F5.json", "45966e73316415626cf41a7d1c6f3b4c70dbc1ba2bee5c1978ef0ce33244fc8d"),
  ("voice_styles/M1.json", "e35604687f5d23694b8e91593a93eec0e4eca6c0b02bb8ed69139ab2ea6b0a5b"),
  ("voice_styles/M2.json", "b76cbf62bac707c710cf0ae5aba5e31eea1a6339a9734bfae33ab98499534a50"),
  ("voice_styles/M3.json", "ea1ac35ccb91b0d7ecad533a2fbd0eec10c91513d8951e3b25fbba99954e159b"),
  ("voice_styles/M4.json", "ca8eefad4fcd989c9379032ff3e50738adc547eeb5e221b82593a6d7b3bac303"),
  ("voice_styles/M5.json", "dd22b92740314321f8ae11c5e87f8dd60d060f15dd3a632b5adf77f471f77af2"),
];

// SHA-256 of a file as lowercase hex.
fn sha256_hex(path: &Path) -> Result<String, String> {
  let mut file =
    fs::File::open(path).map_err(|e| format!("unable to open {}: {}", path.display(), e))?;
  let mut hasher = Sha256::new();
  std::io::copy(&mut file, &mut hasher)
    .map_err(|e| format!("copy failed for {}: {}", path.display(), e))?;
  Ok(hex::encode(hasher.finalize()))
}

fn download_to(url: &str, dest: &Path) {
  println!("cargo:warning=Downloading {} from {}", dest.display(), url);
  fs::create_dir_all(dest.parent().unwrap()).expect("Failed to create download dir");
  let output = Command::new("curl")
    .args(&["-fL", "-o", dest.to_str().unwrap(), url])
    .output()
    .expect("Failed to run curl");
  if !output.status.success() {
    panic!("Failed to download {}: {:?}", url, output);
  }
}

// Make sure every Supertonic 3 file is present in $HOME (downloading and
// checksum-verifying missing or corrupt ones) and copy the model into the
// embedded dir.
fn ensure_supertonic_model(home: &str, embedded_dest: &Path, is_release: bool) {
  let model_dir = Path::new(home)
    .join(".vtmate")
    .join("tts")
    .join("supertonic-model");

  for &(rel, expected) in SUPERTONIC_FILES {
    let path = model_dir.join(rel);
    let url = format!("{}/{}", SUPERTONIC_HF_BASE, rel);

    // Existing files are trusted in debug builds (fast iteration); release
    // builds verify them and re-download on mismatch.
    let mut needs_download = !path.exists();
    if !needs_download && is_release {
      match sha256_hex(&path) {
        Ok(h) if h == expected => {}
        Ok(h) => {
          println!(
            "cargo:warning=Checksum mismatch for {} (expected {}, got {}), re-downloading",
            rel, expected, h
          );
          needs_download = true;
        }
        Err(e) => panic!("{}", e),
      }
    }
    if needs_download {
      download_to(&url, &path);
      let got = sha256_hex(&path).expect("hash after download");
      if got != expected {
        panic!(
          "Checksum mismatch for supertonic file {}: expected {}, got {}",
          rel, expected, got
        );
      }
    }

    let dest_path = embedded_dest.join("supertonic-model").join(rel);
    fs::create_dir_all(dest_path.parent().unwrap()).expect("Failed to create embedded model dir");
    fs::copy(&path, &dest_path).expect("failed to copy supertonic asset");
    println!("cargo:rerun-if-changed={}", path.display());
  }
  println!("cargo:warning=Supertonic 3 model embedded from {}", model_dir.display());
}

// espeak-rs-sys builds the vendored espeak-ng with CMake, whose config
// auto-detects optional system libraries (libpcaudio for audio output,
// libsonic for fast speech rates) and compiles espeak-ng against them when
// present. espeak-rs-sys never emits link directives for those, so on a
// host that has them installed (e.g. Arch with pcaudiolib/libsonic) the
// final link fails with undefined `audio_object_*` / `sonic*` symbols.
//
// Dependency build scripts run before ours, so the CMake cache already
// exists as a sibling of our OUT_DIR. Read it and link exactly the
// libraries espeak-ng was configured with. When a lib was not found (or
// was fetched and compiled into libespeak-ng.a, as sonic is), the cache
// holds NOTFOUND / no file, and nothing is emitted.
fn link_espeak_optional_libs(out_dir: &Path) {
  // OUT_DIR = <profile>/build/vtmate-<hash>/out  ->  <profile>/build
  let Some(build_dir) = out_dir.parent().and_then(Path::parent) else {
    return;
  };
  let Ok(entries) = fs::read_dir(build_dir) else {
    return;
  };
  let mut caches: Vec<(std::time::SystemTime, std::path::PathBuf)> = entries
    .flatten()
    .filter(|e| e.file_name().to_string_lossy().starts_with("espeak-rs-sys-"))
    .map(|e| e.path().join("out").join("build").join("CMakeCache.txt"))
    .filter(|p| p.is_file())
    .filter_map(|p| {
      let mtime = fs::metadata(&p).and_then(|m| m.modified()).ok()?;
      Some((mtime, p))
    })
    .collect();
  // Prefer the most recent configuration if several stale ones linger.
  caches.sort_by(|a, b| b.0.cmp(&a.0));
  let Some((_, cache_path)) = caches.into_iter().next() else {
    return;
  };
  let Ok(cache) = fs::read_to_string(&cache_path) else {
    return;
  };
  println!("cargo:rerun-if-changed={}", cache_path.display());

  let value = |key: &str| -> Option<String> {
    cache.lines().find_map(|line| {
      let (k, v) = line.split_once('=')?;
      let (name, _ty) = k.split_once(':')?;
      (name == key).then(|| v.trim().to_string())
    })
  };

  for (use_flag, lib_key) in [
    ("USE_LIBPCAUDIO", "PCAUDIO_LIB"),
    ("USE_LIBSONIC", "SONIC_LIB"),
  ] {
    let enabled = value(use_flag).is_some_and(|v| v.eq_ignore_ascii_case("ON"));
    if !enabled {
      continue;
    }
    let Some(lib) = value(lib_key) else { continue };
    if lib.ends_with("-NOTFOUND") {
      continue;
    }
    let lib_path = Path::new(&lib);
    if !lib_path.is_file() {
      continue;
    }
    // Not `rustc-link-lib`: rustc places a bin crate's own -l flags *before*
    // the dependency rlibs, and with --as-needed the linker drops a shared
    // library it has not yet seen any undefined symbol for. A raw link-arg
    // is appended at the very end of the command line, after the rlib that
    // holds espeak-ng's objects, so the references are pending by then.
    // -L/-l rather than the full path: a library without a SONAME (libsonic)
    // would otherwise get its absolute build-host path recorded as NEEDED.
    let Some(dir) = lib_path.parent() else { continue };
    let Some(stem) = lib_path.file_stem().and_then(|s| s.to_str()) else {
      continue;
    };
    // libfoo.so / libfoo.so.0 / libfoo.a -> foo
    let name = stem.strip_prefix("lib").unwrap_or(stem);
    let name = name.split(".so").next().unwrap_or(name);
    println!("cargo:rustc-link-arg=-L{}", dir.display());
    println!("cargo:rustc-link-arg=-l{}", name);
    println!(
      "cargo:warning=espeak-ng was configured with {} ({}); linking it",
      lib_key, lib
    );
  }
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
  if cfg!(unix) {
    link_espeak_optional_libs(Path::new(&out_dir));
  }
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

  // Supertonic 3 model (multilingual TTS)
  ensure_supertonic_model(&home, &dest, is_release);

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
