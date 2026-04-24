// ------------------------------------------------------------------
//  supersonic2 tts
// ------------------------------------------------------------------

use crate::audio::AudioChunk;

use crossbeam_channel::{Receiver, Sender};
use std::sync::{
  Arc, Mutex,
  atomic::{AtomicBool, AtomicU64, Ordering},
};
use std::thread;
use std::time::Duration;
use tokio::runtime::Runtime;
extern crate supersonic2_tts as supersonic2_tts_crate;
use super::{SUPSONIC_ENGINE, SpeakOutcome};
use supersonic2_tts_crate::TtsEngine;

// API
// ------------------------------------------------------------------

pub const SUPERSONIC2_VOICE_STYLES: [&str; 10] =
  ["M1", "M2", "M3", "M4", "M5", "F1", "F2", "F3", "F4", "F5"];

pub struct StreamingTts {
  engine: Arc<Mutex<TtsEngine>>,
  pub is_speaking: Arc<AtomicBool>,
  pub interrupt_flag: Arc<AtomicBool>,
  voice: String,
  gain: f32,
}

// Engine initialization
pub fn start_supersonic_engine() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
  let rt = tokio::runtime::Builder::new_current_thread()
    .enable_all()
    .build()?;

  let home = crate::util::get_user_home_path().expect("Could not determine home directory");
  let onnx = home.join(".vtmate/tts/supersonic2-model/onnx");
  let base = home.join(".vtmate/tts/supersonic2-model");
  let engine = rt.block_on(TtsEngine::new(onnx, base, false))?;

  SUPSONIC_ENGINE.set(Arc::new(Mutex::new(engine))).ok();
  Ok(())
}

// Speak via Supersonic2
pub fn speak_via_supersonic2(
  text: &str,
  voice: &str,
  speed: f32,
  _gain: f32,
  language: &str,
  tx: Sender<crate::audio::AudioChunk>,
  stop_all_rx: Receiver<()>,
  interrupt_counter: Arc<AtomicU64>,
  expected_interrupt: u64,
) -> Result<SpeakOutcome, Box<dyn std::error::Error + Send + Sync>> {
  if text.is_empty() {
    return Ok(SpeakOutcome::Completed);
  }
  let rt = tokio::runtime::Builder::new_current_thread()
    .enable_all()
    .build()?;
  let engine = SUPSONIC_ENGINE.get_or_init(|| {
    let home = crate::util::get_user_home_path().expect("Could not determine home directory");
    let onnx = home.join(".vtmate/tts/supersonic2-model/onnx");
    let base = home.join(".vtmate/tts/supersonic2-model");
    let e = rt.block_on(TtsEngine::new(onnx, base, false)).unwrap();
    Arc::new(Mutex::new(e))
  });

  // Check early interrupt
  if stop_all_rx.try_recv().is_ok()
    || interrupt_counter.load(Ordering::SeqCst) != expected_interrupt
  {
    return Ok(SpeakOutcome::Interrupted);
  }

  let mut streaming = StreamingTts::new(engine.clone());
  streaming.set_voice(voice);

  // interrupt monitoring
  let interrupt_flag = streaming.interrupt_flag.clone();
  let stop_rx = stop_all_rx.clone();
  let int_counter = interrupt_counter.clone();
  let expected = expected_interrupt;

  let stop_rx_clone = stop_rx.clone();
  let int_counter_clone = int_counter.clone();

  thread::spawn(move || {
    loop {
      if stop_rx_clone.try_recv().is_ok() || int_counter_clone.load(Ordering::SeqCst) != expected {
        interrupt_flag.store(true, Ordering::Relaxed);
        break;
      }
      thread::sleep(Duration::from_millis(10));
    }
  });

  let rt2 = tokio::runtime::Builder::new_current_thread()
    .enable_all()
    .build()?;
  let res = rt2.block_on(streaming.speak_stream(text, tx.clone(), language, speed));

  match res {
    Ok(_) => Ok(SpeakOutcome::Completed),
    Err(_e) => Ok(SpeakOutcome::Interrupted),
  }
}

// PRIVATE
// ------------------------------------------------------------------

// smaller chunks reduce long synth stalls -> fewer underruns/glitches.
// (Words are variable length; 10–15 is a safer range for real-time streaming.)
const MAX_CHUNK_SIZE: usize = 30;

impl StreamingTts {
  pub fn new(engine: Arc<Mutex<TtsEngine>>) -> Self {
    Self {
      engine,
      is_speaking: Arc::new(AtomicBool::new(false)),
      interrupt_flag: Arc::new(AtomicBool::new(false)),
      voice: "".to_string(),
      gain: 1.5,
    }
  }

  pub fn set_voice(&mut self, voice: &str) {
    self.voice = voice.to_string();
  }

  fn split_into_chunks(text: &str) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut current = String::new();
    let mut count = 0;
    for word in text.split_whitespace() {
      current.push_str(word);
      current.push(' ');
      count += 1;
      if count >= MAX_CHUNK_SIZE {
        chunks.push(current.trim().to_string());
        current.clear();
        count = 0;
      }
    }
    if !current.trim().is_empty() {
      chunks.push(current.trim().to_string());
    }
    chunks
  }

  pub async fn speak_stream(
    &self,
    text: &str,
    tx: Sender<AudioChunk>,
    language: &str,
    speed: f32,
  ) -> Result<(), String> {
    if self.is_speaking.load(Ordering::Relaxed) {
      return Err("Already speaking".into());
    }
    self.is_speaking.store(true, Ordering::Relaxed);
    self.interrupt_flag.store(false, Ordering::Relaxed);

    let chunks = Self::split_into_chunks(text);
    let engine = self.engine.clone();
    let voice = self.voice.clone();
    let gain = self.gain;
    let interrupt_flag_main = self.interrupt_flag.clone();
    let interrupt_flag_thread = interrupt_flag_main.clone();

    let language = language.to_string();
    let handle = thread::spawn(move || {
      // Create a single runtime for the thread
      let rt = match Runtime::new() {
        Ok(r) => r,
        Err(_) => return,
      };
      for chunk in chunks {
        if interrupt_flag_thread.load(Ordering::Relaxed) {
          break;
        }
        if let Ok(e) = engine.lock() {
          // Run async synthesize_with_options
          match rt.block_on(e.synthesize_with_options(
            &chunk,
            Some(&voice),
            speed,
            gain,
            Some(&language),
          )) {
            Ok(mut samples) => {
              // sanitize output samples (prevents nasty noise if NaN/Inf/out-of-range)
              for s in samples.iter_mut() {
                if !s.is_finite() {
                  *s = 0.0;
                } else {
                  *s = s.clamp(-1.0, 1.0);
                }
              }
              let audio = AudioChunk {
                data: samples,
                channels: 1,
                sample_rate: 48000,
              };
              if interrupt_flag_thread.load(Ordering::Relaxed) {
                break;
              }
              if tx.send(audio).is_err() {
                break;
              }
            }
            Err(_) => {
              break;
            }
          }
        } else {
          break;
        }
      }
    });

    handle.join().ok();
    self.is_speaking.store(false, Ordering::Relaxed);
    if interrupt_flag_main.load(Ordering::Relaxed) {
      Err("Interrupted".into())
    } else {
      Ok(())
    }
  }
}
