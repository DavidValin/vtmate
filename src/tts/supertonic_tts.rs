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
use supertonic3_tts::clone::{CloneProgress, CloneStage, load_reference};
use supertonic3_tts::device::Device;
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
  let mut engine = get_or_init_engine()?;
  let style = get_or_load_style(voice)?;
  let rt = runtime()?;

  let mut sample_rate = rt.block_on(engine.sample_rate()) as u32;

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
      // The GPU can still fail here after Device::Auto was happy: Auto only
      // covers whether the execution provider initialises, and a card that is
      // merely full fails later, on the first inference. Speech is worth more
      // than the acceleration, so drop to the CPU and say the chunk anyway.
      Err(e) if crate::util::looks_like_gpu_failure(&e.to_string()) && !FORCE_CPU.load(Ordering::SeqCst) => {
        crate::log::log(
          "warning",
          &format!(
            "[supertonic_tts] GPU synthesis failed ({}); falling back to the CPU for the rest of this run",
            e
          ),
        );
        engine = rebuild_on_cpu()?;
        sample_rate = rt.block_on(engine.sample_rate()) as u32;
        match rt.block_on(engine.synthesize_with_style(
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
                "[supertonic_tts] synthesis failed on the CPU too for chunk '{}': {}",
                chunk, e
              ),
            );
            return Err(format!("supertonic synthesis failed: {}", e).into());
          }
        }
      }
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

/// One progress update of [`clone_voice`], already reduced to what a caller
/// needs to render a progress bar: `fraction` is the overall (all stages
/// combined) 0.0..=1.0 progress; `stage`/`iteration`/`stage_total` describe
/// where in the current stage that overall progress sits.
pub struct CloneProgressInfo {
  pub stage: String,
  pub iteration: usize,
  pub stage_total: usize,
  pub fraction: f64,
}

/// Train a new supertonic3 voice from a reference recording and drop it into
/// `voice_styles_dir()`, where it is immediately usable as `--voice
/// <voice_name>` (any tts picking voices from that folder sees it right away).
///
/// `voice_name` must be free (no `<voice_name>.json` there yet, built-in
/// presets included) and made only of ASCII letters, digits and `_`;
/// `language` must be one of `SUPPORTED_LANGS`. `on_progress` is called after
/// every iteration of the mix-presets, mix-rows and refine stages (not
/// select-preset, which has no fixed size and does not move the bar).
pub fn clone_voice(
  voice_name: &str,
  language: &str,
  wav_file: &str,
  reference_text: &str,
  mut on_progress: impl FnMut(CloneProgressInfo) + Send + 'static,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
  if voice_name.is_empty() || !voice_name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
    return Err(
      format!(
        "invalid voice name '{}': only alphanumeric characters and '_' are allowed",
        voice_name
      )
      .into(),
    );
  }
  if !SUPPORTED_LANGS.contains(&language) {
    return Err(
      format!(
        "unsupported language '{}' for supertonic3 (supported: {})",
        language,
        SUPPORTED_LANGS.join(", ")
      )
      .into(),
    );
  }
  if reference_text.trim().is_empty() {
    return Err("reference text must not be empty".into());
  }
  let style_path = voice_styles_dir().join(format!("{}.json", voice_name));
  if style_path.exists() {
    return Err(
      format!(
        "voice name '{}' is already taken in supertonic3 ({})",
        voice_name,
        style_path.display()
      )
      .into(),
    );
  }

  let reference = validate_and_load_reference(wav_file)?;
  let engine = get_or_init_engine()?;
  let rt = runtime()?;

  let options = supertonic3_tts::CloneOptions {
    language: language.to_string(),
    reference_text: Some(reference_text.to_string()),
    ..Default::default()
  };

  crate::log::log(
    "info",
    &format!(
      "[supertonic_tts] cloning voice \"{}\" ({}) from {}",
      voice_name, language, wav_file
    ),
  );
  let total_iters =
    (options.mix_iterations + options.row_iterations + options.iterations).max(1) as f64;
  let mut done = 0usize;
  let progress = move |p: &CloneProgress<'_>| {
    if p.stage != CloneStage::SelectPreset {
      done += 1;
    }
    on_progress(CloneProgressInfo {
      stage: p.stage.to_string(),
      iteration: p.iteration,
      stage_total: p.total,
      fraction: (done as f64 / total_iters).min(1.0),
    });
  };

  let cloned = rt
    .block_on(engine.clone_voice_with_progress(&reference, &options, progress))
    .map_err(|e| {
      let msg = format!("[supertonic_tts] voice cloning failed: {}", e);
      crate::log::log("error", &msg);
      msg
    })?;

  std::fs::create_dir_all(voice_styles_dir())
    .map_err(|e| format!("failed to create voice styles folder: {}", e))?;
  cloned
    .save(&style_path)
    .map_err(|e| format!("failed to save cloned voice to {}: {}", style_path.display(), e))?;

  Ok(())
}

// PRIVATE
// ------------------------------------------------------------------

/// Below this there isn't enough speech for a stable clone (the crate itself
/// hard-fails under 0.25s; this gives a clearer error before that point).
const MIN_CLONE_REFERENCE_SECONDS: f64 = 2.0;
/// `clone_voice` always uses the whole recording as the transcript window (no
/// `--ref-window` support here), and the crate recommends that window stay a
/// sentence or two, up to ~15s (`supertonic3_tts::clone::MAX_TRANSCRIPT_SECONDS`)
/// before alignment quality drops; 30s leaves comfortable slack over that for
/// an unedited recording (e.g. a few seconds of leading/trailing silence).
const MAX_CLONE_REFERENCE_SECONDS: f64 = 30.0;

/// Validate `wav_file` as a voice-cloning reference and load it: mono or
/// stereo, a plausible sample rate, and a duration within
/// `MIN_CLONE_REFERENCE_SECONDS..=MAX_CLONE_REFERENCE_SECONDS`.
///
/// Also repairs a `data` chunk that declares more bytes than the file
/// actually holds: `arecord` writes that placeholder when it can't seek back
/// to patch the header on exit (e.g. stopped with Ctrl-C), which otherwise
/// makes hound fail with "Failed to read enough bytes" partway through
/// decoding, well past every other check.
fn validate_and_load_reference(
  wav_file: &str,
) -> Result<supertonic3_tts::Audio, Box<dyn std::error::Error + Send + Sync>> {
  let path = std::path::Path::new(wav_file);
  if !path.is_file() {
    return Err(format!("wav file not found: {}", wav_file).into());
  }
  let bytes = std::fs::read(path)
    .map_err(|e| format!("failed to read wav file '{}': {}", wav_file, e))?;

  let spec = hound::WavReader::open(path)
    .map_err(|e| format!("'{}' is not a readable wav file: {}", wav_file, e))?
    .spec();
  if !(1..=2).contains(&spec.channels) {
    return Err(
      format!(
        "'{}' has {} channels; only mono or stereo reference recordings are supported for voice cloning",
        wav_file, spec.channels
      )
      .into(),
    );
  }
  if !(8_000..=192_000).contains(&spec.sample_rate) {
    return Err(
      format!(
        "'{}' has an unusual sample rate ({} Hz); expected somewhere between 8000 and 192000 Hz",
        wav_file, spec.sample_rate
      )
      .into(),
    );
  }

  let (data_offset, declared_len) = locate_data_chunk(&bytes)
    .ok_or_else(|| format!("'{}' has no readable wav data chunk", wav_file))?;
  let available = bytes.len().saturating_sub(data_offset);
  let block_align = ((spec.channels as usize) * (spec.bits_per_sample as usize / 8).max(1)).max(1);
  let mut effective_len = if declared_len == 0 || declared_len > available {
    available
  } else {
    declared_len
  };
  effective_len -= effective_len % block_align;

  let duration_secs = effective_len as f64 / block_align as f64 / spec.sample_rate as f64;
  if duration_secs < MIN_CLONE_REFERENCE_SECONDS {
    return Err(
      format!(
        "'{}' is only {:.1}s long; at least {:.0}s of speech is needed for voice cloning",
        wav_file, duration_secs, MIN_CLONE_REFERENCE_SECONDS
      )
      .into(),
    );
  }
  if duration_secs > MAX_CLONE_REFERENCE_SECONDS {
    return Err(
      format!(
        "'{}' is {:.1}s long; keep the reference under {:.0}s (a sentence or two matching the reference text works best)",
        wav_file, duration_secs, MAX_CLONE_REFERENCE_SECONDS
      )
      .into(),
    );
  }

  if declared_len == effective_len {
    return load_reference(wav_file)
      .map_err(|e| format!("failed to load reference wav '{}': {}", wav_file, e).into());
  }

  // The data chunk lied about its size (or was a streaming placeholder):
  // write a corrected copy to a temp file and load that instead.
  crate::log::log(
    "warning",
    &format!(
      "[supertonic_tts] '{}' has a broken wav header (data chunk declares {} bytes, {} available); repairing a temporary copy",
      wav_file, declared_len, available
    ),
  );
  let mut fixed = Vec::with_capacity(data_offset + effective_len);
  fixed.extend_from_slice(&bytes[..data_offset - 4]);
  fixed.extend_from_slice(&(effective_len as u32).to_le_bytes());
  fixed.extend_from_slice(&bytes[data_offset..data_offset + effective_len]);
  let riff_size = (fixed.len() - 8) as u32;
  fixed[4..8].copy_from_slice(&riff_size.to_le_bytes());

  let tmp_path =
    std::env::temp_dir().join(format!("vtmate-clone-ref-{}.wav", uuid::Uuid::new_v4()));
  std::fs::write(&tmp_path, &fixed)
    .map_err(|e| format!("failed to write repaired wav to {}: {}", tmp_path.display(), e))?;
  let result: Result<supertonic3_tts::Audio, Box<dyn std::error::Error + Send + Sync>> =
    load_reference(&tmp_path)
      .map_err(|e| format!("failed to load repaired reference wav: {}", e).into());
  let _ = std::fs::remove_file(&tmp_path);
  result
}

/// Byte offset of the `data` chunk's payload and its declared size (which
/// may be wrong, see [`validate_and_load_reference`]), found by walking RIFF
/// chunks from the top rather than trusting hound's parsed length.
fn locate_data_chunk(bytes: &[u8]) -> Option<(usize, usize)> {
  if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
    return None;
  }
  let mut pos = 12usize;
  while pos + 8 <= bytes.len() {
    let id = &bytes[pos..pos + 4];
    let size = u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into().ok()?) as usize;
    let body = pos + 8;
    if id == b"data" {
      return Some((body, size));
    }
    let next = body.checked_add(size)?.checked_add(size % 2)?;
    if next <= pos {
      return None;
    }
    pos = next;
  }
  None
}

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

/// Set once the GPU has failed at synthesis: every engine built afterwards
/// asks for the CPU, so a card that is out of memory is not retried per chunk.
static FORCE_CPU: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Drop the shared engine and build it again pinned to the CPU.
fn rebuild_on_cpu() -> Result<Arc<TtsEngine>, Box<dyn std::error::Error + Send + Sync>> {
  FORCE_CPU.store(true, Ordering::SeqCst);
  if let Ok(mut slot) = SUPERTONIC_ENGINE.lock() {
    *slot = None;
  }
  get_or_init_engine()
}

fn get_or_init_engine() -> Result<Arc<TtsEngine>, Box<dyn std::error::Error + Send + Sync>> {
  if let Ok(slot) = SUPERTONIC_ENGINE.lock() {
    if let Some(e) = slot.as_ref() {
      return Ok(e.clone());
    }
  }
  let base = model_root();
  let onnx_dir = base.join("onnx");
  // Device::Auto takes the GPU when its provider initialises and the CPU
  // otherwise; FORCE_CPU is the stronger statement made after a GPU failure
  // that Auto cannot see, because it happened past initialisation.
  let device = if FORCE_CPU.load(Ordering::SeqCst) {
    Device::Cpu
  } else {
    Device::Auto
  };
  // Built without the lock held: model loading takes seconds, and blocking
  // every other speaker on it is worse than the rare double build, where both
  // engines are valid and the last one stored wins.
  let engine = runtime()?
    .block_on(TtsEngine::on_device(onnx_dir.clone(), base, false, device))
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
  let engine = Arc::new(engine);
  if let Ok(mut slot) = SUPERTONIC_ENGINE.lock() {
    *slot = Some(engine.clone());
  }
  Ok(engine)
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
