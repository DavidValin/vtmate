// ------------------------------------------------------------------
//  Configuration
// ------------------------------------------------------------------

use clap::{Parser, value_parser};
use cpal::Device;
use cpal::traits::DeviceTrait;

// API
// ------------------------------------------------------------------

#[derive(Parser, Debug, Clone)]
#[command(
  author = env!("CARGO_PKG_AUTHORS"),
  version,
  long_about = concat!(
    "\n\n",
    env!("CARGO_PKG_DESCRIPTION"),
    "\n\nHomepage: ",
    env!("CARGO_PKG_HOMEPAGE")
  )
)]
pub struct Args {
  /// Enable verbose logging (prints audio device/config info and other diagnostics)
  #[arg(long, action = clap::ArgAction::SetTrue)]
  pub verbose: bool,

  /// Ollama generate endpoint URL
  #[arg(long, default_value = OLLAMA_URL_DEFAULT, env = "OLLAMA_URL")]
  pub ollama_url: String,

  /// Ollama model name
  #[arg(long, default_value = OLLAMA_MODEL_DEFAULT, env = "OLLAMA_MODEL")]
  pub ollama_model: String,

  /// Whisper model file path
  #[arg(long, default_value = WHISPER_MODEL_PATH, env = "WHISPER_MODEL_PATH")]
  pub whisper_model_path: String,

  /// OpenTTS base URL (we append &text=...)
  #[arg(long, default_value = OPENTTS_BASE_URL_DEFAULT, env = "OPENTTS_BASE_URL")]
  pub opentts_base_url: String,

  /// Language code for TTS and Whisper (e.g., en, es, de)
  #[arg(long, default_value = "en", env = "LANGUAGE")]
  pub language: String,

  /// Text-to-speech backend (e.g., kokoro, opentts)
  #[arg(
      long,
      default_value = "kokoro",
      env = "TTS",
      value_parser = clap::builder::PossibleValuesParser::new(&["kokoro", "opentts"])
  )]
  pub tts: String,

  /// Peak threshold for detecting user speech while assistant is speaking (0..1)
  #[arg(long, default_value_t = SOUND_THRESHOLD_PEAK_DEFAULT, env = "SOUND_THRESHOLD_PEAK")]
  pub sound_threshold_peak: f32,

  /// End an utterance after this much continuous silence (ms)
  #[arg(long, default_value_t = END_SILENCE_MS_DEFAULT, env = "END_SILENCE_MS")]
  pub end_silence_ms: u64,
}

// CLI parameters default values ---------------------------------------------------

const SOUND_THRESHOLD_PEAK_DEFAULT: f32 = 0.10;
pub const HANGOVER_MS_DEFAULT: u64 = 100;
const END_SILENCE_MS_DEFAULT: u64 = 850;
pub const MIN_UTTERANCE_MS_DEFAULT: u64 = 300;
pub const OLLAMA_URL_DEFAULT: &str = "http://localhost:11434/api/generate";
pub const OLLAMA_MODEL_DEFAULT: &str = "llama3.2:3b";
pub const WHISPER_MODEL_PATH: &str = "~/.whisper-models/ggml-medium-q5_0.bin";
const OPENTTS_BASE_URL_DEFAULT: &str = "http://0.0.0.0:5500/api/tts?&vocoder=high&denoiserStrength=0.005&&speakerId=&ssml=false&ssmlNumbers=true&ssmlDates=true&ssmlCurrency=true&cache=false";

impl Args {
  /// Resolve the whisper model path, expanding ~ to home directory.
  pub fn resolved_whisper_model_path(&self) -> String {
    if self.whisper_model_path.starts_with("~") {
      let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".to_string());
      let rel = self.whisper_model_path.trim_start_matches("~");
      let mut p = std::path::PathBuf::from(home);
      p.push(&rel[1..]); // remove leading /
      p.to_string_lossy().into_owned()
    } else {
      self.whisper_model_path.clone()
    }
  }
}

/// Pick an input configuration that matches the preferred sample rate as closely as possible.
///
/// This helper selects a supported stream configuration from the device, preferring
/// formats that are compatible with the desired sample rate. It returns the first
/// configuration after sorting by format, channel count, and sample‑rate distance.
pub fn pick_input_config(
  device: &Device,
  preferred_sr: u32,
) -> Result<cpal::SupportedStreamConfig, Box<dyn std::error::Error>> {
  use cpal::SampleFormat;

  let mut candidates: Vec<cpal::SupportedStreamConfig> = Vec::new();
  for range in device.supported_input_configs()? {
    let min_sr = range.min_sample_rate().0;
    let max_sr = range.max_sample_rate().0;
    let chosen_sr = preferred_sr.clamp(min_sr, max_sr);
    candidates.push(range.with_sample_rate(cpal::SampleRate(chosen_sr)));
  }

  candidates.sort_by_key(|cfg| {
    let fmt_rank = match cfg.sample_format() {
      SampleFormat::F32 => 0,
      SampleFormat::I16 => 1,
      SampleFormat::U16 => 2,
      _ => 9,
    };
    let ch_rank = match cfg.channels() {
      1 => 0,
      2 => 1,
      _ => 5,
    };
    let sr_rank = cfg.sample_rate().0.abs_diff(preferred_sr);
    (fmt_rank, ch_rank, sr_rank)
  });

  candidates
    .into_iter()
    .next()
    .ok_or_else(|| "no supported input configs".into())
}
