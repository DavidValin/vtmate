// ------------------------------------------------------------------
//  Supertonic TTS (Supertonic 3, multilingual)
// ------------------------------------------------------------------
//
// Thin wrapper around the `supertonic3-tts` crate. The model files are
// embedded in the binary at build time (see build.rs / assets.rs) and
// extracted to ~/.vtmate/tts/supertonic-model/{onnx,voice_styles} on first
// run.

use crate::audio::AudioChunk;
use crossbeam_channel::Sender;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{
  Arc, Mutex, OnceLock,
  atomic::{AtomicU64, Ordering},
};
use supertonic3_tts::TtsEngine;
use supertonic3_tts::helper::{Style, chunk_text, load_voice_style, max_chunk_length};
use tokio::runtime::Runtime;

use super::{SUPERTONIC_ENGINE, SpeakOutcome};

// API
// ------------------------------------------------------------------

pub const SUPERTONIC_VOICE_STYLES: [&str; 10] = supertonic3_tts::VOICE_STYLES;

/// Languages selectable from vtmate settings for the 'supertonic' tts
/// (the model also accepts "na", language agnostic, but STT needs a
/// concrete language so it is not offered).
pub const SUPPORTED_LANGS: &[&str] = supertonic3_tts::SUPPORTED_LANGS;

/// Number of flow-matching denoising steps (crate "voice quality", 5..12).
/// Upstream default is 8; 5 trades a little clarity for noticeably less
/// vector-estimator time.
const VOICE_QUALITY: usize = 5;

// Speak via Supertonic
pub fn speak_via_supertonic(
  text: &str,
  voice: &str,
  speed: f32,
  language: &str,
  tx: Sender<crate::audio::AudioChunk>,
  interrupt_counter: Arc<AtomicU64>,
  expected_interrupt: u64,
) -> Result<SpeakOutcome, Box<dyn std::error::Error + Send + Sync>> {
  if text.is_empty() {
    return Ok(SpeakOutcome::Completed);
  }
  let engine = get_or_init_engine()?;
  let style = get_or_load_style(voice)?;
  let rt = runtime()?;

  let sample_rate = rt.block_on(engine.sample_rate()) as u32;

  // Split text into sentence-aware chunks so playback can start before the
  // whole phrase is synthesized and interruptions are honoured between
  // chunks.
  let chunks = chunk_text(text, Some(max_chunk_length(language)));

  for chunk in chunks {
    if chunk.trim().is_empty() {
      continue;
    }
    // Check for interruption
    if interrupt_counter.load(Ordering::SeqCst) != expected_interrupt {
      return Ok(SpeakOutcome::Interrupted);
    }
    let samples = match rt.block_on(engine.synthesize_with_style(
      &chunk,
      &style,
      speed,
      1.0,
      Some(language),
      Some(VOICE_QUALITY),
    )) {
      Ok(s) => s,
      Err(e) => {
        crate::log::log(
          "error",
          &format!(
            "[supertonic_tts] synthesis failed for chunk '{}': {}",
            chunk, e
          ),
        );
        return Err(format!("supertonic synthesis failed: {}", e).into());
      }
    };
    if samples.is_empty() {
      continue;
    }

    let audio = AudioChunk {
      data: samples,
      channels: 1,
      sample_rate,
    };
    // Re-check after synthesis: an interrupt that arrived while this chunk
    // was being generated already emptied the playback queue, so sending it
    // now would start speaking again.
    if interrupt_counter.load(Ordering::SeqCst) != expected_interrupt {
      crate::log::log(
        "debug",
        "[supertonic_tts] interrupted during synthesis: chunk discarded, not played",
      );
      return Ok(SpeakOutcome::Interrupted);
    }
    if tx.send(audio).is_err() {
      return Ok(SpeakOutcome::Interrupted);
    }
  }
  Ok(SpeakOutcome::Completed)
}

// PRIVATE
// ------------------------------------------------------------------

/// Root of the extracted model: <root>/onnx/*.onnx and <root>/voice_styles/*.json
pub fn model_root() -> PathBuf {
  if let Some(dir) = std::env::var_os("SUPERTONIC_DATA_DIRECTORY") {
    return PathBuf::from(dir);
  }
  let home = crate::util::get_user_home_path().expect("Could not determine home directory");
  home.join(".vtmate").join("tts").join("supertonic-model")
}

/// Directory holding one `<voice>.json` per voice.
pub fn voice_styles_dir() -> PathBuf {
  model_root().join("voice_styles")
}

static RUNTIME: OnceLock<Runtime> = OnceLock::new();

fn runtime() -> Result<&'static Runtime, Box<dyn std::error::Error + Send + Sync>> {
  if let Some(rt) = RUNTIME.get() {
    return Ok(rt);
  }
  let rt = tokio::runtime::Builder::new_current_thread()
    .enable_all()
    .build()?;
  let _ = RUNTIME.set(rt);
  Ok(RUNTIME.get().expect("runtime just set"))
}

fn get_or_init_engine() -> Result<Arc<TtsEngine>, Box<dyn std::error::Error + Send + Sync>> {
  if let Some(e) = SUPERTONIC_ENGINE.get() {
    return Ok(e.clone());
  }
  let base = model_root();
  let onnx_dir = base.join("onnx");
  let engine = runtime()?
    .block_on(TtsEngine::new(onnx_dir.clone(), base, false))
    .map_err(|e| {
      let msg = format!(
        "[supertonic_tts] failed to load model from {}: {}",
        onnx_dir.display(),
        e
      );
      crate::log::log("error", &msg);
      msg
    })?;
  crate::log::log(
    "info",
    &format!("[supertonic_tts] running on {}", engine.backend()),
  );
  let _ = SUPERTONIC_ENGINE.set(Arc::new(engine));
  Ok(SUPERTONIC_ENGINE.get().expect("engine just set").clone())
}

static STYLE_CACHE: OnceLock<Mutex<HashMap<String, Arc<Style>>>> = OnceLock::new();

fn get_or_load_style(voice: &str) -> Result<Arc<Style>, Box<dyn std::error::Error + Send + Sync>> {
  let cache = STYLE_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
  if let Ok(c) = cache.lock() {
    if let Some(s) = c.get(voice) {
      return Ok(s.clone());
    }
  }
  let style_path = model_root()
    .join("voice_styles")
    .join(format!("{}.json", voice));
  let style =
    load_voice_style(&[style_path.to_string_lossy().to_string()], false).map_err(|e| {
      let msg = format!(
        "[supertonic_tts] failed to load voice style {}: {}",
        style_path.display(),
        e
      );
      crate::log::log("error", &msg);
      msg
    })?;
  let style = Arc::new(style);
  if let Ok(mut c) = cache.lock() {
    c.insert(voice.to_string(), style.clone());
  }
  Ok(style)
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn supported_langs_exclude_language_agnostic() {
    assert!(!SUPPORTED_LANGS.contains(&"na"));
    for l in SUPPORTED_LANGS {
      assert!(supertonic3_tts::is_valid_lang(l));
    }
  }

  // End-to-end synthesis against the real model. Needs the Supertonic 3 model
  // extracted at ~/.vtmate/tts/supertonic-model (build.rs puts it there), so
  // it is ignored by default:
  //   SUPERTONIC_TEST_OUT=/tmp cargo test --release -- --ignored supertonic
  #[test]
  #[ignore]
  fn synthesize_samples_to_wav() {
    let engine = get_or_init_engine().expect("load supertonic model");
    let style = get_or_load_style("M1").expect("load voice style");
    let rt = runtime().expect("tokio runtime");
    let sample_rate = rt.block_on(engine.sample_rate()) as u32;
    let out_dir = std::env::var("SUPERTONIC_TEST_OUT")
      .map(PathBuf::from)
      .unwrap_or_else(|_| std::env::temp_dir());

    let cases = [
      (
        "es",
        "Hola, ¿cómo estás? Hoy hace un día espléndido en Madrid.",
      ),
      (
        "en",
        "Hello there. The train delay was announced at 4:45 PM, and everyone sighed.",
      ),
    ];
    for (lang, text) in cases {
      let t0 = std::time::Instant::now();
      let samples = rt
        .block_on(engine.synthesize_with_style(
          text,
          &style,
          1.0,
          1.0,
          Some(lang),
          Some(VOICE_QUALITY),
        ))
        .expect("inference");
      let duration = samples.len() as f32 / sample_rate as f32;
      assert!(duration > 0.5, "duration too short for {}", lang);
      assert!(
        samples.iter().any(|s| s.abs() > 0.01),
        "silent output for {}",
        lang
      );

      let spec = hound::WavSpec {
        channels: 1,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
      };
      let path = out_dir.join(format!("supertonic_{}.wav", lang));
      let mut w = hound::WavWriter::create(&path, spec).unwrap();
      for s in &samples {
        w.write_sample((s.clamp(-1.0, 1.0) * 32767.0) as i16)
          .unwrap();
      }
      w.finalize().unwrap();
      eprintln!(
        "{}: {:.2}s of audio synthesized in {:.2}s -> {}",
        lang,
        duration,
        t0.elapsed().as_secs_f32(),
        path.display()
      );
    }
  }
}
