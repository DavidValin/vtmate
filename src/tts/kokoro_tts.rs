// ------------------------------------------------------------------
//  kokoro tts
// ------------------------------------------------------------------

use crate::audio::AudioChunk;
use crossbeam_channel::Sender;
use kokoro_tiny::TtsEngine;
use std::sync::{
  Arc, Mutex,
  atomic::{AtomicBool, Ordering},
};
use std::thread;

// API
// ------------------------------------------------------------------
pub struct StreamingTts {
  engine: Arc<Mutex<TtsEngine>>,
  pub is_speaking: Arc<AtomicBool>,
  pub interrupt_flag: Arc<AtomicBool>,
  voice: String,
  gain: f32,
}

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
      for chunk in chunks {
        if interrupt_flag_thread.load(Ordering::Relaxed) {
          break;
        }
        if let Ok(mut e) = engine.lock() {
          if let Ok(samples) = e.synthesize_with_options(
            &chunk,
            Some(&voice),
            crate::state::get_speed(),
            gain,
            Some(&language),
          ) {
            let audio = AudioChunk {
              data: samples,
              channels: 1,
              sample_rate: 24000,
            };
            if interrupt_flag_thread.load(Ordering::Relaxed) {
              break;
            }
            if tx.send(audio).is_err() {
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

// PRIVATE
// ------------------------------------------------------------------

const MAX_CHUNK_SIZE: usize = 50;
