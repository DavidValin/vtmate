// ------------------------------------------------------------------
//  STT - Speech to Text
// ------------------------------------------------------------------

use crate::audio;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext};

/// Warm‑up helper for Whisper
/// Call this once at startup to load the model and perform a no‑op
/// inference to cache the model into memory.
pub fn whisper_warmup(
  whisper_model_path: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
  if !std::path::Path::new(whisper_model_path).is_file() {
    return Err(format!("Whisper model not found: {}", whisper_model_path).into());
  }
  let ctx = WhisperContext::new_with_params(whisper_model_path, Default::default())?;
  let mut state = ctx.create_state().expect("failed to create state");
  let warmup = vec![0.0f32; 16000]; // 1.0s @ 16kHz
  state.full(
    FullParams::new(SamplingStrategy::Greedy { best_of: 1 }),
    &warmup,
  )?;
  Ok(())
}

pub fn whisper_transcribe_with_ctx(
  ctx: &WhisperContext,
  pcm_mono_f32: &[f32],
  sample_rate: u32,
  language: &str,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
  // Ensure bounded samples (optional if already normalized)
  let mono: Vec<f32> = pcm_mono_f32.iter().map(|s| s.clamp(-1.0, 1.0)).collect();

  // Resample to 16k if needed
  let mono_16k: Vec<f32> = if sample_rate != 16000 {
    audio::resample_to(&mono, 1, sample_rate, 16000)
  } else {
    mono
  };

  // Guard against too-short audio
  if mono_16k.len() < 1920 {
    return Ok(String::new());
  }

  let mut state = ctx.create_state()?;

  let mut params = FullParams::new(SamplingStrategy::BeamSearch {
    beam_size: 5,
    patience: -1.0,
  });
  params.set_print_progress(false);
  params.set_print_special(false);
  params.set_print_timestamps(false);
  params.set_print_realtime(false);
  params.set_translate(false);
  params.set_language(Some(language));

  state
    .full(params, &mono_16k)
    .map_err(|e| format!("Inference failed: {:?}", e))?;

  let mut result = String::new();
  let seg_count = state.full_n_segments();
  for i in 0..seg_count {
    let seg = state
      .get_segment(i)
      .ok_or_else(|| format!("Segment {} out of range", i))?;
    let seg_text = seg
      .to_str_lossy()
      .map_err(|e| format!("Failed to get segment text: {:?}", e))?;
    result.push_str(&seg_text);
    result.push(' ');
  }

  Ok(result.trim_end().to_string())
}
