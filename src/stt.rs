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
  let empty: Vec<f32> = vec![0.0; 160]; // 10 ms at 16kHz
  state.full(
    FullParams::new(SamplingStrategy::Greedy { best_of: 1 }),
    &empty,
  )?;
  Ok(())
}

pub fn whisper_transcribe_with_ctx(
  ctx: &whisper_rs::WhisperContext,
  pcm_chunks: &[i16],
  sample_rate: u32,
  language: &str,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
  let mut inter_samples = vec![Default::default(); pcm_chunks.len()];
  whisper_rs::convert_integer_to_float_audio(pcm_chunks, &mut inter_samples)
    .expect("failed to convert audio data");

  let mono_samples = {
    let min_samples = 3200; // 200 ms (whisper.cpp minimum)
    let mono = whisper_rs::convert_stereo_to_mono_audio(&inter_samples)
      .map_err(|e| format!("Failed to convert audio: {:?}", e))?;

    // let resampled = if sample_rate != 16000 {
    //   audio::resample_to(&mono, 1, sample_rate, 16000)
    // } else {
    //   mono
    // };

    // Pad to at least 100 ms if too short
    // if resampled.len() < min_samples {
    //   let mut padded = resampled.clone();
    //   padded.extend(std::iter::repeat(0.0).take(min_samples - resampled.len()));
    //   padded
    // } else {
    //   resampled
    // }
    mono
  };
  crate::log::log(
    "debug",
    &format!("Mono samples length: {}", mono_samples.len()),
  );

  crate::log::log("debug", "Creating Whisper state");
  let mut state = ctx.create_state().expect("failed to create whisper state");
  let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
  params.set_print_progress(false);
  params.set_print_special(false);
  params.set_print_timestamps(false);
  params.set_print_realtime(false);
  params.set_translate(false);
  params.set_language(Some(language));
  params.set_n_threads(1);

  let start = std::time::Instant::now();
  match state.full(params, &mono_samples[..]) {
    Ok(_) => {
      let elapsed = start.elapsed();
      crate::log::log(
        "info",
        &format!("Whisper transcription took {:.2?}", elapsed),
      );
    }
    Err(e) => {
      crate::log::log("error", &format!("Whisper full() failed: {:?}", e));
      return Err(e.into());
    }
  }

  let mut result = String::new();
  let seg_count = state.full_n_segments() as usize;
  for i in 0..seg_count {
    let seg = state
      .get_segment(i as i32)
      .ok_or_else(|| format!("Segment {} out of range", i))?;
    let seg_text = seg
      .to_str_lossy()
      .map_err(|e| format!("Failed to get segment text: {:?}", e))?;
    result.push_str(&seg_text);
    result.push(' ');
  }
  Ok(result.trim_end().to_string())
}
