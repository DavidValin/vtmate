use std::{fs, io::Cursor, path::PathBuf};
use flate2::read::GzDecoder;
use tar::Archive;

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
        std::env::set_var(
            "PIPER_ESPEAKNG_DATA_DIRECTORY",
            base.as_os_str(),
        );
    }
}

/// Returns the embedded espeak-ng data archive (tar.gz) as raw bytes.
///
/// The file is embedded at compile time via `include_bytes!`.
/// The path is resolved relative to the crate's Cargo.toml.
pub fn embedded_espeak_archive() -> &'static [u8] {
    static ESPEAK_TGZ: &[u8] = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/assets/espeak-ng-data.tar.gz"
    ));
    ESPEAK_TGZ
}