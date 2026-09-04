// ------------------------------------------------------------------
//  Supertonic TTS (Supertonic 3, multilingual)
// ------------------------------------------------------------------
//
// Inference code adapted from the upstream helper.rs of
// https://github.com/supertone-inc/supertonic. The model files are embedded
// in the binary at build time (see build.rs / assets.rs) and extracted to
// ~/.vtmate/tts/supertonic-model/{onnx,voice_styles} on first run.

use crate::audio::AudioChunk;
use anyhow::{Context, Result, bail};
use crossbeam_channel::Sender;
use ndarray::{Array, Array3};
use ort::session::Session;
use ort::value::Value;
use rand_distr::{Distribution, Normal};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::sync::{
  Arc, Mutex, OnceLock,
  atomic::{AtomicU64, Ordering},
};
use unicode_normalization::UnicodeNormalization;

use super::{SUPERTONIC_ENGINE, SpeakOutcome};

// API
// ------------------------------------------------------------------

pub const SUPERTONIC_VOICE_STYLES: [&str; 10] =
  ["M1", "M2", "M3", "M4", "M5", "F1", "F2", "F3", "F4", "F5"];

/// Languages accepted by the Supertonic 3 model. "na" means language
/// agnostic and is accepted by the model but is not offered as a vtmate
/// language (STT needs a concrete language), see `SUPPORTED_LANGS`.
pub const AVAILABLE_LANGS: &[&str] = &[
  "en", "ko", "ja", "ar", "bg", "cs", "da", "de", "el", "es", "et", "fi", "fr", "hi", "hr",
  "hu", "id", "it", "lt", "lv", "nl", "pl", "pt", "ro", "ru", "sk", "sl", "sv", "tr", "uk",
  "vi", "na",
];

/// Languages selectable from vtmate settings for the 'supertonic' tts.
pub const SUPPORTED_LANGS: &[&str] = &[
  "en", "ko", "ja", "ar", "bg", "cs", "da", "de", "el", "es", "et", "fi", "fr", "hi", "hr",
  "hu", "id", "it", "lt", "lv", "nl", "pl", "pt", "ro", "ru", "sk", "sl", "sv", "tr", "uk",
  "vi",
];

pub fn is_valid_lang(lang: &str) -> bool {
  AVAILABLE_LANGS.contains(&lang)
}

/// Number of flow-matching denoising steps. Upstream default.
const TOTAL_STEPS: usize = 8;

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

  let sample_rate = match engine.lock() {
    Ok(e) => e.sample_rate,
    Err(_) => 44100,
  };

  // Split text into sentence-aware chunks. Shorter chunks for languages
  // whose characters carry more information per code point.
  let max_chars = if language == "ko" || language == "ja" { 120 } else { 300 };
  let chunks = chunk_text(text, Some(max_chars));

  for chunk in chunks {
    if chunk.trim().is_empty() {
      continue;
    }
    // Check for interruption
    if interrupt_counter.load(Ordering::SeqCst) != expected_interrupt {
      return Ok(SpeakOutcome::Interrupted);
    }
    let mut engine = match engine.lock() {
      Ok(e) => e,
      Err(_) => return Ok(SpeakOutcome::Interrupted),
    };
    let (wav, duration) = match engine._infer(
      &[chunk.clone()],
      &[language.to_string()],
      &style,
      TOTAL_STEPS,
      speed,
    ) {
      Ok(r) => r,
      Err(e) => {
        crate::log::log(
          "error",
          &format!("[supertonic_tts] synthesis failed for chunk '{}': {}", chunk, e),
        );
        return Err(format!("supertonic synthesis failed: {}", e).into());
      }
    };
    drop(engine);

    // The vocoder emits whole latent chunks; cut the tail down to the
    // predicted duration exactly like upstream does.
    let wav_len = ((sample_rate as f32) * duration[0]) as usize;
    let mut samples: Vec<f32> = wav[..wav_len.min(wav.len())].to_vec();
    for s in samples.iter_mut() {
      if !s.is_finite() {
        *s = 0.0;
      } else {
        *s = s.clamp(-1.0, 1.0);
      }
    }
    if samples.is_empty() {
      continue;
    }

    let audio = AudioChunk {
      data: samples,
      channels: 1,
      sample_rate: sample_rate as u32,
    };
    if tx.send(audio).is_err() {
      return Ok(SpeakOutcome::Interrupted);
    }
  }
  Ok(SpeakOutcome::Completed)
}

// PRIVATE
// ------------------------------------------------------------------

/// Root of the extracted model: <root>/onnx/*.onnx and <root>/voice_styles/*.json
fn model_root() -> PathBuf {
  if let Some(dir) = std::env::var_os("SUPERTONIC_DATA_DIRECTORY") {
    return PathBuf::from(dir);
  }
  let home = crate::util::get_user_home_path().expect("Could not determine home directory");
  home.join(".vtmate").join("tts").join("supertonic-model")
}

fn get_or_init_engine()
-> Result<Arc<Mutex<TextToSpeech>>, Box<dyn std::error::Error + Send + Sync>> {
  if let Some(e) = SUPERTONIC_ENGINE.get() {
    return Ok(e.clone());
  }
  let onnx_dir = model_root().join("onnx");
  let engine = load_text_to_speech(&onnx_dir).map_err(|e| {
    let msg = format!(
      "[supertonic_tts] failed to load model from {}: {}",
      onnx_dir.display(),
      e
    );
    crate::log::log("error", &msg);
    msg
  })?;
  let _ = SUPERTONIC_ENGINE.set(Arc::new(Mutex::new(engine)));
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
  let style = load_voice_style(&[style_path.to_string_lossy().to_string()]).map_err(|e| {
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

// ============================================================================
// Configuration Structures
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
  pub ae: AEConfig,
  pub ttl: TTLConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AEConfig {
  pub sample_rate: i32,
  pub base_chunk_size: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TTLConfig {
  pub chunk_compress_factor: i32,
  pub latent_dim: i32,
}

/// Load configuration from JSON file
pub fn load_cfgs<P: AsRef<Path>>(onnx_dir: P) -> Result<Config> {
  let cfg_path = onnx_dir.as_ref().join("tts.json");
  let file = File::open(&cfg_path).with_context(|| format!("open {}", cfg_path.display()))?;
  let reader = BufReader::new(file);
  let cfgs: Config = serde_json::from_reader(reader)?;
  Ok(cfgs)
}

// ============================================================================
// Voice Style Data Structure
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoiceStyleData {
  pub style_ttl: StyleComponent,
  pub style_dp: StyleComponent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StyleComponent {
  pub data: Vec<Vec<Vec<f32>>>,
  pub dims: Vec<usize>,
  #[serde(rename = "type")]
  pub dtype: String,
}

// ============================================================================
// Unicode Text Processor
// ============================================================================

pub struct UnicodeProcessor {
  indexer: Vec<i64>,
}

impl UnicodeProcessor {
  pub fn new<P: AsRef<Path>>(unicode_indexer_json_path: P) -> Result<Self> {
    let file = File::open(&unicode_indexer_json_path).with_context(|| {
      format!("open {}", unicode_indexer_json_path.as_ref().display())
    })?;
    let reader = BufReader::new(file);
    let indexer: Vec<i64> = serde_json::from_reader(reader)?;
    Ok(UnicodeProcessor { indexer })
  }

  pub fn call(
    &self,
    text_list: &[String],
    lang_list: &[String],
  ) -> Result<(Vec<Vec<i64>>, Array3<f32>)> {
    let mut processed_texts: Vec<String> = Vec::new();
    for (text, lang) in text_list.iter().zip(lang_list.iter()) {
      processed_texts.push(preprocess_text(text, lang)?);
    }

    let text_ids_lengths: Vec<usize> =
      processed_texts.iter().map(|t| t.chars().count()).collect();

    let max_len = *text_ids_lengths.iter().max().unwrap_or(&0);

    let mut text_ids = Vec::new();
    for text in &processed_texts {
      let mut row = vec![0i64; max_len];
      let unicode_vals = text_to_unicode_values(text);
      for (j, &val) in unicode_vals.iter().enumerate() {
        if val < self.indexer.len() {
          row[j] = self.indexer[val];
        } else {
          row[j] = -1;
        }
      }
      text_ids.push(row);
    }

    let text_mask = get_text_mask(&text_ids_lengths);

    Ok((text_ids, text_mask))
  }
}

fn emoji_regex() -> &'static Regex {
  static RE: OnceLock<Regex> = OnceLock::new();
  RE.get_or_init(|| {
    Regex::new(
      r"[\x{1F600}-\x{1F64F}\x{1F300}-\x{1F5FF}\x{1F680}-\x{1F6FF}\x{1F700}-\x{1F77F}\x{1F780}-\x{1F7FF}\x{1F800}-\x{1F8FF}\x{1F900}-\x{1F9FF}\x{1FA00}-\x{1FA6F}\x{1FA70}-\x{1FAFF}\x{2600}-\x{26FF}\x{2700}-\x{27BF}\x{1F1E6}-\x{1F1FF}]+",
    )
    .unwrap()
  })
}

fn whitespace_regex() -> &'static Regex {
  static RE: OnceLock<Regex> = OnceLock::new();
  RE.get_or_init(|| Regex::new(r"\s+").unwrap())
}

fn ends_with_punct_regex() -> &'static Regex {
  static RE: OnceLock<Regex> = OnceLock::new();
  RE.get_or_init(|| {
    Regex::new(r#"[.!?;:,'"\u{201C}\u{201D}\u{2018}\u{2019})\]}…。」』】〉》›»]$"#).unwrap()
  })
}

/// Text normalisation identical to upstream Supertonic 2/3: NFKD, strip
/// emojis and odd symbols, normalise quotes/dashes, tidy punctuation spacing
/// and wrap the result in `<lang>...</lang>` tags which the multilingual
/// models were trained with.
pub fn preprocess_text(text: &str, lang: &str) -> Result<String> {
  if !is_valid_lang(lang) {
    bail!("Invalid language: {}. Available: {:?}", lang, AVAILABLE_LANGS);
  }

  let mut text: String = text.nfkd().collect();

  // Remove emojis (wide Unicode range)
  text = emoji_regex().replace_all(&text, "").to_string();

  // Replace various dashes and symbols
  let replacements = [
    ("–", "-"),         // en dash
    ("‑", "-"),         // non-breaking hyphen
    ("—", "-"),         // em dash
    ("_", " "),         // underscore
    ("\u{201C}", "\""), // left double quote
    ("\u{201D}", "\""), // right double quote
    ("\u{2018}", "'"),  // left single quote
    ("\u{2019}", "'"),  // right single quote
    ("´", "'"),         // acute accent
    ("`", "'"),         // grave accent
    ("[", " "),         // left bracket
    ("]", " "),         // right bracket
    ("|", " "),         // vertical bar
    ("/", " "),         // slash
    ("#", " "),         // hash
    ("→", " "),         // right arrow
    ("←", " "),         // left arrow
  ];
  for (from, to) in &replacements {
    text = text.replace(from, to);
  }

  // Remove special symbols
  for symbol in ["♥", "☆", "♡", "©", "\\"] {
    text = text.replace(symbol, "");
  }

  // Replace known expressions
  let expr_replacements = [
    ("@", " at "),
    ("e.g.,", "for example, "),
    ("i.e.,", "that is, "),
  ];
  for (from, to) in &expr_replacements {
    text = text.replace(from, to);
  }

  // Fix spacing around punctuation
  for (from, to) in [
    (" ,", ","),
    (" .", "."),
    (" !", "!"),
    (" ?", "?"),
    (" ;", ";"),
    (" :", ":"),
    (" '", "'"),
  ] {
    text = text.replace(from, to);
  }

  // Remove duplicate quotes
  while text.contains("\"\"") {
    text = text.replace("\"\"", "\"");
  }
  while text.contains("''") {
    text = text.replace("''", "'");
  }
  while text.contains("``") {
    text = text.replace("``", "`");
  }

  // Remove extra spaces
  text = whitespace_regex().replace_all(&text, " ").to_string();
  text = text.trim().to_string();

  // If text doesn't end with punctuation, quotes, or closing brackets, add a period
  if !text.is_empty() && !ends_with_punct_regex().is_match(&text) {
    text.push('.');
  }

  // Wrap text with language tags
  Ok(format!("<{}>{}</{}>", lang, text, lang))
}

pub fn text_to_unicode_values(text: &str) -> Vec<usize> {
  text.chars().map(|c| c as usize).collect()
}

pub fn length_to_mask(lengths: &[usize], max_len: Option<usize>) -> Array3<f32> {
  let bsz = lengths.len();
  let max_len = max_len.unwrap_or_else(|| *lengths.iter().max().unwrap_or(&0));

  let mut mask = Array3::<f32>::zeros((bsz, 1, max_len));
  for (i, &len) in lengths.iter().enumerate() {
    for j in 0..len.min(max_len) {
      mask[[i, 0, j]] = 1.0;
    }
  }
  mask
}

pub fn get_text_mask(text_ids_lengths: &[usize]) -> Array3<f32> {
  let max_len = *text_ids_lengths.iter().max().unwrap_or(&0);
  length_to_mask(text_ids_lengths, Some(max_len))
}

/// Sample noisy latent from normal distribution and apply mask
pub fn sample_noisy_latent(
  duration: &[f32],
  sample_rate: i32,
  base_chunk_size: i32,
  chunk_compress: i32,
  latent_dim: i32,
) -> (Array3<f32>, Array3<f32>) {
  let bsz = duration.len();
  let max_dur = duration.iter().fold(0.0f32, |a, &b| a.max(b));

  let wav_len_max = (max_dur * sample_rate as f32) as usize;
  let wav_lengths: Vec<usize> = duration
    .iter()
    .map(|&d| (d * sample_rate as f32) as usize)
    .collect();

  let chunk_size = (base_chunk_size * chunk_compress) as usize;
  let latent_len = (wav_len_max + chunk_size - 1) / chunk_size;
  let latent_dim_val = (latent_dim * chunk_compress) as usize;

  let mut noisy_latent = Array3::<f32>::zeros((bsz, latent_dim_val, latent_len));

  let normal = Normal::new(0.0, 1.0).unwrap();
  let mut rng = rand::thread_rng();

  for b in 0..bsz {
    for d in 0..latent_dim_val {
      for t in 0..latent_len {
        noisy_latent[[b, d, t]] = normal.sample(&mut rng);
      }
    }
  }

  let latent_lengths: Vec<usize> = wav_lengths
    .iter()
    .map(|&len| (len + chunk_size - 1) / chunk_size)
    .collect();

  let latent_mask = length_to_mask(&latent_lengths, Some(latent_len));

  // Apply mask
  for b in 0..bsz {
    for d in 0..latent_dim_val {
      for t in 0..latent_len {
        noisy_latent[[b, d, t]] *= latent_mask[[b, 0, t]];
      }
    }
  }

  (noisy_latent, latent_mask)
}

// ============================================================================
// Text Chunking
// ============================================================================

const MAX_CHUNK_LENGTH: usize = 300;

const ABBREVIATIONS: &[&str] = &[
  "Dr.", "Mr.", "Mrs.", "Ms.", "Prof.", "Sr.", "Jr.", "St.", "Ave.", "Rd.", "Blvd.", "Dept.",
  "Inc.", "Ltd.", "Co.", "Corp.", "etc.", "vs.", "i.e.", "e.g.", "Ph.D.",
];

pub fn chunk_text(text: &str, max_len: Option<usize>) -> Vec<String> {
  let max_len = max_len.unwrap_or(MAX_CHUNK_LENGTH);
  let text = text.trim();

  if text.is_empty() {
    return vec![String::new()];
  }

  // Split by paragraphs
  let para_re = Regex::new(r"\n\s*\n").unwrap();
  let paragraphs: Vec<&str> = para_re.split(text).collect();
  let mut chunks: Vec<String> = Vec::new();

  for para_str in paragraphs {
    let para = para_str.trim();
    if para.is_empty() {
      continue;
    }

    if para.len() <= max_len {
      chunks.push(para.to_string());
      continue;
    }

    // Split by sentences
    let sentences = split_sentences(para);
    let mut current = String::new();
    let mut current_len = 0;

    for sentence in sentences {
      let sentence = sentence.trim();
      if sentence.is_empty() {
        continue;
      }

      let sentence_len = sentence.len();
      if sentence_len > max_len {
        // If sentence is longer than max_len, split by comma or space
        if !current.is_empty() {
          chunks.push(current.trim().to_string());
          current.clear();
          current_len = 0;
        }

        // Try splitting by comma
        let parts: Vec<&str> = sentence.split(',').collect();
        for part in parts {
          let part = part.trim();
          if part.is_empty() {
            continue;
          }

          let part_len = part.len();
          if part_len > max_len {
            // Split by space as last resort
            let words: Vec<&str> = part.split_whitespace().collect();
            let mut word_chunk = String::new();
            let mut word_chunk_len = 0;

            for word in words {
              let word_len = word.len();
              if word_chunk_len + word_len + 1 > max_len && !word_chunk.is_empty() {
                chunks.push(word_chunk.trim().to_string());
                word_chunk.clear();
                word_chunk_len = 0;
              }

              if !word_chunk.is_empty() {
                word_chunk.push(' ');
                word_chunk_len += 1;
              }
              word_chunk.push_str(word);
              word_chunk_len += word_len;
            }

            if !word_chunk.is_empty() {
              chunks.push(word_chunk.trim().to_string());
            }
          } else {
            if current_len + part_len + 1 > max_len && !current.is_empty() {
              chunks.push(current.trim().to_string());
              current.clear();
              current_len = 0;
            }

            if !current.is_empty() {
              current.push_str(", ");
              current_len += 2;
            }
            current.push_str(part);
            current_len += part_len;
          }
        }
        continue;
      }

      if current_len + sentence_len + 1 > max_len && !current.is_empty() {
        chunks.push(current.trim().to_string());
        current.clear();
        current_len = 0;
      }

      if !current.is_empty() {
        current.push(' ');
        current_len += 1;
      }
      current.push_str(sentence);
      current_len += sentence_len;
    }

    if !current.is_empty() {
      chunks.push(current.trim().to_string());
    }
  }

  if chunks.is_empty() {
    vec![String::new()]
  } else {
    chunks
  }
}

fn split_sentences(text: &str) -> Vec<String> {
  // Rust's regex doesn't support lookbehind, so we use a simpler approach
  // Split on sentence boundaries and then check if they're abbreviations
  let re = Regex::new(r"([.!?])\s+").unwrap();

  // Find all matches
  let matches: Vec<_> = re.find_iter(text).collect();
  if matches.is_empty() {
    return vec![text.to_string()];
  }

  let mut sentences: Vec<String> = Vec::new();
  let mut last_end: usize = 0;

  for m in matches {
    // Get the text before the punctuation
    let before_punc = &text[last_end..m.start()];

    // Check if this ends with an abbreviation
    let mut is_abbrev = false;
    for abbrev in ABBREVIATIONS {
      let combined = format!("{}{}", before_punc.trim(), &text[m.start()..m.start() + 1]);
      if combined.ends_with(abbrev) {
        is_abbrev = true;
        break;
      }
    }

    if !is_abbrev {
      // This is a real sentence boundary
      sentences.push(text[last_end..m.end()].to_string());
      last_end = m.end();
    }
  }

  // Add the remaining text
  if last_end < text.len() {
    sentences.push(text[last_end..].to_string());
  }

  if sentences.is_empty() {
    vec![text.to_string()]
  } else {
    sentences
  }
}

// ============================================================================
// ONNX Runtime Integration
// ============================================================================

pub struct Style {
  pub ttl: Array3<f32>,
  pub dp: Array3<f32>,
}

pub struct TextToSpeech {
  cfgs: Config,
  text_processor: UnicodeProcessor,
  dp_ort: Session,
  text_enc_ort: Session,
  vector_est_ort: Session,
  vocoder_ort: Session,
  pub sample_rate: i32,
}

impl TextToSpeech {
  pub fn new(
    cfgs: Config,
    text_processor: UnicodeProcessor,
    dp_ort: Session,
    text_enc_ort: Session,
    vector_est_ort: Session,
    vocoder_ort: Session,
  ) -> Self {
    let sample_rate = cfgs.ae.sample_rate;
    TextToSpeech {
      cfgs,
      text_processor,
      dp_ort,
      text_enc_ort,
      vector_est_ort,
      vocoder_ort,
      sample_rate,
    }
  }

  /// Run the full pipeline for a batch of texts. Returns the raw vocoder
  /// output (padded to whole latent chunks) and the predicted duration in
  /// seconds per batch item.
  fn _infer(
    &mut self,
    text_list: &[String],
    lang_list: &[String],
    style: &Style,
    total_step: usize,
    speed: f32,
  ) -> Result<(Vec<f32>, Vec<f32>)> {
    let bsz = text_list.len();

    // Process text
    let (text_ids, text_mask) = self.text_processor.call(text_list, lang_list)?;

    let text_ids_array = {
      let text_ids_shape = (bsz, text_ids[0].len());
      let mut flat = Vec::new();
      for row in &text_ids {
        flat.extend_from_slice(row);
      }
      Array::from_shape_vec(text_ids_shape, flat)?
    };

    let text_ids_value = Value::from_array(text_ids_array)?;
    let text_mask_value = Value::from_array(text_mask.clone())?;
    let style_dp_value = Value::from_array(style.dp.clone())?;

    // Predict duration
    let dp_outputs = self.dp_ort.run(ort::inputs! {
      "text_ids" => &text_ids_value,
      "style_dp" => &style_dp_value,
      "text_mask" => &text_mask_value
    })?;

    let (_, duration_data) = dp_outputs["duration"].try_extract_tensor::<f32>()?;
    let mut duration: Vec<f32> = duration_data.to_vec();

    // Apply speed factor to duration
    for dur in duration.iter_mut() {
      *dur /= speed;
    }

    // Encode text
    let style_ttl_value = Value::from_array(style.ttl.clone())?;
    let text_enc_outputs = self.text_enc_ort.run(ort::inputs! {
      "text_ids" => &text_ids_value,
      "style_ttl" => &style_ttl_value,
      "text_mask" => &text_mask_value
    })?;

    let (text_emb_shape, text_emb_data) =
      text_enc_outputs["text_emb"].try_extract_tensor::<f32>()?;
    let text_emb = Array3::from_shape_vec(
      (
        text_emb_shape[0] as usize,
        text_emb_shape[1] as usize,
        text_emb_shape[2] as usize,
      ),
      text_emb_data.to_vec(),
    )?;

    // Sample noisy latent
    let (mut xt, latent_mask) = sample_noisy_latent(
      &duration,
      self.sample_rate,
      self.cfgs.ae.base_chunk_size,
      self.cfgs.ttl.chunk_compress_factor,
      self.cfgs.ttl.latent_dim,
    );

    // Prepare constant arrays
    let total_step_array = Array::from_elem(bsz, total_step as f32);

    // Denoising loop
    for step in 0..total_step {
      let current_step_array = Array::from_elem(bsz, step as f32);

      let xt_value = Value::from_array(xt.clone())?;
      let text_emb_value = Value::from_array(text_emb.clone())?;
      let latent_mask_value = Value::from_array(latent_mask.clone())?;
      let text_mask_value2 = Value::from_array(text_mask.clone())?;
      let current_step_value = Value::from_array(current_step_array)?;
      let total_step_value = Value::from_array(total_step_array.clone())?;

      let vector_est_outputs = self.vector_est_ort.run(ort::inputs! {
        "noisy_latent" => &xt_value,
        "text_emb" => &text_emb_value,
        "style_ttl" => &style_ttl_value,
        "latent_mask" => &latent_mask_value,
        "text_mask" => &text_mask_value2,
        "current_step" => &current_step_value,
        "total_step" => &total_step_value,
      })?;

      let (denoised_shape, denoised_data) =
        vector_est_outputs["denoised_latent"].try_extract_tensor::<f32>()?;
      xt = Array3::from_shape_vec(
        (
          denoised_shape[0] as usize,
          denoised_shape[1] as usize,
          denoised_shape[2] as usize,
        ),
        denoised_data.to_vec(),
      )?;
    }

    // Generate waveform
    let final_latent_value = Value::from_array(xt)?;
    let vocoder_outputs = self.vocoder_ort.run(ort::inputs! {
      "latent" => &final_latent_value
    })?;

    let (_, wav_data) = vocoder_outputs["wav_tts"].try_extract_tensor::<f32>()?;
    let wav: Vec<f32> = wav_data.to_vec();

    Ok((wav, duration))
  }
}

// ============================================================================
// Component Loading Functions
// ============================================================================

/// Load voice style from JSON files
pub fn load_voice_style(voice_style_paths: &[String]) -> Result<Style> {
  let bsz = voice_style_paths.len();

  // Read first file to get dimensions
  let first_file =
    File::open(&voice_style_paths[0]).context("Failed to open voice style file")?;
  let first_reader = BufReader::new(first_file);
  let first_data: VoiceStyleData = serde_json::from_reader(first_reader)?;

  let ttl_dims = &first_data.style_ttl.dims;
  let dp_dims = &first_data.style_dp.dims;

  let ttl_dim1 = ttl_dims[1];
  let ttl_dim2 = ttl_dims[2];
  let dp_dim1 = dp_dims[1];
  let dp_dim2 = dp_dims[2];

  // Pre-allocate arrays with full batch size
  let ttl_size = bsz * ttl_dim1 * ttl_dim2;
  let dp_size = bsz * dp_dim1 * dp_dim2;
  let mut ttl_flat = vec![0.0f32; ttl_size];
  let mut dp_flat = vec![0.0f32; dp_size];

  // Fill in the data
  for (i, path) in voice_style_paths.iter().enumerate() {
    let file = File::open(path).context("Failed to open voice style file")?;
    let reader = BufReader::new(file);
    let data: VoiceStyleData = serde_json::from_reader(reader)?;

    // Flatten TTL data
    let ttl_offset = i * ttl_dim1 * ttl_dim2;
    let mut idx = 0;
    for batch in &data.style_ttl.data {
      for row in batch {
        for &val in row {
          ttl_flat[ttl_offset + idx] = val;
          idx += 1;
        }
      }
    }

    // Flatten DP data
    let dp_offset = i * dp_dim1 * dp_dim2;
    idx = 0;
    for batch in &data.style_dp.data {
      for row in batch {
        for &val in row {
          dp_flat[dp_offset + idx] = val;
          idx += 1;
        }
      }
    }
  }

  let ttl_style = Array3::from_shape_vec((bsz, ttl_dim1, ttl_dim2), ttl_flat)?;
  let dp_style = Array3::from_shape_vec((bsz, dp_dim1, dp_dim2), dp_flat)?;

  Ok(Style {
    ttl: ttl_style,
    dp: dp_style,
  })
}

/// Load TTS components from <onnx_dir>/{*.onnx,tts.json,unicode_indexer.json}
pub fn load_text_to_speech(onnx_dir: impl AsRef<Path>) -> Result<TextToSpeech> {
  let onnx_dir = onnx_dir.as_ref();
  let cfgs = load_cfgs(onnx_dir)?;

  let dp_path = onnx_dir.join("duration_predictor.onnx");
  let text_enc_path = onnx_dir.join("text_encoder.onnx");
  let vector_est_path = onnx_dir.join("vector_estimator.onnx");
  let vocoder_path = onnx_dir.join("vocoder.onnx");

  let dp_ort = Session::builder()?.commit_from_file(&dp_path)?;
  let text_enc_ort = Session::builder()?.commit_from_file(&text_enc_path)?;
  let vector_est_ort = Session::builder()?.commit_from_file(&vector_est_path)?;
  let vocoder_ort = Session::builder()?.commit_from_file(&vocoder_path)?;

  let unicode_indexer_path = onnx_dir.join("unicode_indexer.json");
  let text_processor = UnicodeProcessor::new(&unicode_indexer_path)?;

  crate::log::log(
    "debug",
    &format!("[supertonic_tts] model loaded from {}", onnx_dir.display()),
  );

  Ok(TextToSpeech::new(
    cfgs,
    text_processor,
    dp_ort,
    text_enc_ort,
    vector_est_ort,
    vocoder_ort,
  ))
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn preprocess_wraps_with_language_tags() {
    assert_eq!(preprocess_text("hola", "es").unwrap(), "<es>hola.</es>");
    assert_eq!(preprocess_text("Hi 😀 there", "en").unwrap(), "<en>Hi there.</en>");
    assert!(preprocess_text("x", "xx").is_err());
    assert!(preprocess_text("x", "na").is_ok());
  }

  #[test]
  fn supported_langs_exclude_language_agnostic() {
    assert!(!SUPPORTED_LANGS.contains(&"na"));
    for l in SUPPORTED_LANGS {
      assert!(AVAILABLE_LANGS.contains(l));
    }
  }

  // End-to-end synthesis against the real model. Needs the Supertonic 3 model
  // extracted at ~/.vtmate/tts/supertonic-model (build.rs puts it there), so
  // it is ignored by default:
  //   SUPERTONIC_TEST_OUT=/tmp cargo test --release -- --ignored supertonic
  #[test]
  #[ignore]
  fn synthesize_samples_to_wav() {
    let root = model_root();
    let mut tts = load_text_to_speech(root.join("onnx")).expect("load supertonic model");
    let style_path = root.join("voice_styles").join("M1.json");
    let style =
      load_voice_style(&[style_path.to_string_lossy().to_string()]).expect("load voice style");
    let out_dir = std::env::var("SUPERTONIC_TEST_OUT")
      .map(PathBuf::from)
      .unwrap_or_else(|_| std::env::temp_dir());

    let cases = [
      ("es", "Hola, ¿cómo estás? Hoy hace un día espléndido en Madrid."),
      ("en", "Hello there. The train delay was announced at 4:45 PM, and everyone sighed."),
    ];
    for (lang, text) in cases {
      let t0 = std::time::Instant::now();
      let (wav, duration) = tts
        ._infer(&[text.to_string()], &[lang.to_string()], &style, TOTAL_STEPS, 1.0)
        .expect("inference");
      eprintln!(
        "{}: duration tensor len={} values={:?} wav_len={} sr={}",
        lang,
        duration.len(),
        duration,
        wav.len(),
        tts.sample_rate
      );
      let n = ((tts.sample_rate as f32) * duration[0]) as usize;
      let samples = &wav[..n.min(wav.len())];
      assert!(duration[0] > 0.5, "duration too short for {}", lang);
      assert!(samples.iter().any(|s| s.abs() > 0.01), "silent output for {}", lang);

      let spec = hound::WavSpec {
        channels: 1,
        sample_rate: tts.sample_rate as u32,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
      };
      let path = out_dir.join(format!("supertonic_{}.wav", lang));
      let mut w = hound::WavWriter::create(&path, spec).unwrap();
      for s in samples {
        w.write_sample((s.clamp(-1.0, 1.0) * 32767.0) as i16).unwrap();
      }
      w.finalize().unwrap();
      eprintln!(
        "{}: {:.2}s of audio synthesized in {:.2}s -> {}",
        lang,
        duration[0],
        t0.elapsed().as_secs_f32(),
        path.display()
      );
    }
  }
}
