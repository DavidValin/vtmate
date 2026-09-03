// ------------------------------------------------------------------
//  STT - Speech to Text
// ------------------------------------------------------------------

use crate::audio;
use std::sync::{Mutex, OnceLock};
use whisper_rs::{
  FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters, WhisperState,
};

// API
// ------------------------------------------------------------------

/// One model load and ONE inference state for the whole process.
///
/// The state (KV caches, compute buffers, backend instances) used to be created
/// and freed for every utterance. On the CUDA build that free crashed:
/// whisper.cpp 1.7.6's `whisper_free_state` -> `ggml_backend_buffer_free` jumped
/// through a corrupted buffer vtable (use-after-free inside whisper.cpp/ggml on
/// the GPU path; the CPU backend never hit it). whisper.cpp is designed to reuse
/// a state across `whisper_full` calls, so keeping a single one both matches the
/// intended usage and never runs the crashing teardown during a session. It also
/// stops the model being loaded twice at startup (the old warm-up built a second
/// context just to discard it) and keeps the GPU buffers reserved, so a sound
/// server or LLM filling VRAM mid-session cannot starve a later turn.
pub struct Whisper {
  state: Mutex<WhisperState>,
}

static WHISPER: OnceLock<Whisper> = OnceLock::new();

/// Load the model, create the state and warm it up. Idempotent; later calls
/// return the existing instance.
pub fn init(model_path: &str) -> Result<&'static Whisper, Box<dyn std::error::Error + Send + Sync>> {
  if let Some(w) = WHISPER.get() {
    return Ok(w);
  }
  let w = Whisper::load(model_path)?;
  Ok(WHISPER.get_or_init(|| w))
}

impl Whisper {
  fn load(model_path: &str) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
    if !std::path::Path::new(model_path).is_file() {
      return Err(format!("Whisper model not found: {}", model_path).into());
    }
    match Self::load_with(model_path, true) {
      Ok(w) => Ok(w),
      Err(e) => {
        crate::log::log(
          "warn",
          &format!("Whisper GPU initialisation failed ({}); falling back to CPU", e),
        );
        Self::load_with(model_path, false)
      }
    }
  }

  fn load_with(
    model_path: &str,
    use_gpu: bool,
  ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
    let mut params = WhisperContextParameters::default();
    params.use_gpu(use_gpu);
    let ctx = WhisperContext::new_with_params(model_path, params)?;
    // The state keeps the context alive; the wrapper itself is not needed.
    let mut state = ctx.create_state()?;
    // Warm-up: 1 s of silence primes the compute buffers and kernels.
    state.full(
      FullParams::new(SamplingStrategy::Greedy { best_of: 1 }),
      &vec![0.0f32; 16000],
    )?;
    Ok(Self {
      state: Mutex::new(state),
    })
  }

  pub fn transcribe(
    &self,
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

    let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
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
}
