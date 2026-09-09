// ------------------------------------------------------------------
//  kokoro tts
// ------------------------------------------------------------------

use super::{KOKORO_ENGINE, SpeakOutcome};
use crate::audio::AudioChunk;
use crossbeam_channel::Sender;
use kokoro_micro::TtsEngine;
use std::sync::{
  Arc, Mutex,
  atomic::{AtomicBool, AtomicU64, Ordering},
};
use std::thread;
use std::time::Duration;

// API
// ------------------------------------------------------------------
pub struct StreamingTts {
  engine: Arc<Mutex<TtsEngine>>,
  pub is_speaking: Arc<AtomicBool>,
  pub interrupt_flag: Arc<AtomicBool>,
  voice: String,
  gain: f32,
}

// Engine initialization
pub fn start_kokoro_engine() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
  KOKORO_ENGINE.set(Arc::new(Mutex::new(load_engine()?))).ok();
  Ok(())
}

/// Load the Kokoro model. `TtsEngine::new` is Device::Auto: the GPU when this
/// build carries a GPU execution provider (`ort-cuda`) and it comes up, the CPU
/// otherwise. Unlike the other two engines this needs no fallback of ours -
/// kokoro-micro retries on the CPU itself when inference fails on the GPU, and
/// a second layer here would only synthesize every failed chunk twice.
fn load_engine() -> Result<TtsEngine, Box<dyn std::error::Error + Send + Sync>> {
  let rt = tokio::runtime::Builder::new_current_thread()
    .enable_all()
    .build()?;
  let engine = rt.block_on(TtsEngine::new())?;
  crate::log::log(
    "info",
    &format!("[kokoro_tts] running on {}", engine.backend()),
  );
  Ok(engine)
}

// Speak via Kokoro
pub fn speak_via_kokoro(
  text: &str,
  language: &str,
  voice: &str,
  tx: Sender<crate::audio::AudioChunk>,
  interrupt_counter: Arc<AtomicU64>,
  expected_interrupt: u64,
) -> Result<SpeakOutcome, Box<dyn std::error::Error + Send + Sync>> {
  let engine = KOKORO_ENGINE.get_or_init(|| Arc::new(Mutex::new(load_engine().unwrap())));

  let mut streaming = StreamingTts::new(engine.clone());
  streaming.set_voice(voice);

  // interrupt monitoring
  let interrupt_flag = streaming.interrupt_flag.clone();
  let int_counter = interrupt_counter.clone();
  let expected = expected_interrupt;
  let stop_monitor = Arc::new(AtomicBool::new(false));
  let stop_monitor_thread = stop_monitor.clone();

  let monitor_handle = thread::spawn(move || {
    loop {
      if stop_monitor_thread.load(Ordering::Relaxed) {
        break;
      }
      if int_counter.load(Ordering::SeqCst) != expected {
        interrupt_flag.store(true, Ordering::Relaxed);
        break;
      }
      thread::sleep(Duration::from_millis(10));
    }
  });

  // Start synthesis - the monitoring thread will handle interruptions during synthesis
  let rt = tokio::runtime::Builder::new_current_thread()
    .enable_all()
    .build()?;
  let res = rt.block_on(streaming.speak_stream(text, tx.clone(), language));

  // Synthesis finished (normally or interrupted) - stop the monitor thread so it doesn't leak
  stop_monitor.store(true, Ordering::Relaxed);
  let _ = monitor_handle.join();

  match res {
    Ok(_) => Ok(SpeakOutcome::Completed),
    Err(_e) => Ok(SpeakOutcome::Interrupted),
  }
}

pub const KOKORO_VOICES_PER_LANGUAGE: &[(&str, &[&str])] = &[
  // English language
  // ----------------------------------------
  (
    "en",
    &[
      // American english - female
      "af_alloy",
      "af_aoede",
      "af_bella",
      "af_heart",
      "af_jessica",
      "af_kore",
      "af_nicole",
      "af_nova",
      "af_river",
      "af_sarah",
      "af_sky",
      // American english - male
      "am_adam",
      "am_echo",
      "am_eric",
      "am_fenrir",
      "am_liam",
      "am_michael",
      "am_onyx",
      "am_puck",
      "am_santa",
      // British english - female
      "bf_alice",
      "bf_emma",
      "bf_isabella",
      "bf_lily",
      // British english - male
      "bm_daniel",
      "bm_fable",
      "bm_george",
      "bm_lewis",
    ],
  ),
  // Spanish language
  // ----------------------------------------
  (
    "es",
    &[
      // Spanish - female
      "ef_dora", // Spanish - male
      "em_alex", "em_santa",
    ],
  ),
  // Mandarin chinese language
  // ----------------------------------------
  (
    "zh",
    &[
      // Mandarin chinese - female
      "zf_xiaobei",
      "zf_xiaoni",
      "zf_xiaoxiao",
      "zf_xiaoyi",
      // Mandarin chinese - male
      "zm_yunjian",
      "zm_yunxi",
      "zm_yunxia",
      "zm_yunyang",
    ],
  ),
  // Japanese language
  // ----------------------------------------
  (
    "ja",
    &[
      // Japanese - female
      "jf_alpha",
      "jf_gongitsune",
      "jf_nezumi",
      "jf_tebukuro",
      // Japanese - male
      "jm_kumo",
    ],
  ),
  // Portuguese / Brazil language
  // ----------------------------------------
  (
    "pt",
    &[
      // Portuguese - female
      "pf_dora", // Portuguese - male
      "pm_alex", "pm_santa",
    ],
  ),
  // Italian language
  // ----------------------------------------
  (
    "it",
    &[
      // Italian - female
      "if_sara",
      // Italian - male
      "im_nicola",
    ],
  ),
  // Hindi language
  // ----------------------------------------
  (
    "hi",
    &[
      // Hindi - female
      "hf_alpha", "hf_beta", // Hindi - male
      "hm_omega", "hm_psi",
    ],
  ),
  // French language
  // ----------------------------------------
  (
    "fr",
    &[
      // French - female
      "ff_siwis",
    ],
  ),
];

pub const _DEFAULT_KOKORO_VOICES_PER_LANGUAGE: &[(&str, &str)] = &[
  ("en", "bf_emma"),
  ("es", "em_santa"),
  ("zh", "zf_xiaoni"),
  ("ja", "jm_kumo"),
  ("pt", "pf_dora"),
  ("it", "if_sara"),
  ("hi", "hf_alpha"),
  ("fr", "ff_siwis"),
];

// PRIVATE
// ------------------------------------------------------------------

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

    // The whole phrase goes to kokoro-micro in one piece. It splits on real
    // sentence and clause boundaries in any script and packs to the model's
    // context length, so cutting the text up here first only served to reset
    // prosody mid-sentence. `util::split_text_for_tts` has already broken the
    // reply into phrases upstream, which is what keeps playback streaming and
    // bounds how long an interrupt takes to land.
    let phrase = text.to_string();
    let engine = self.engine.clone();
    let voice = self.voice.clone();
    let gain = self.gain;
    let interrupt_flag_main = self.interrupt_flag.clone();
    let interrupt_flag_thread = interrupt_flag_main.clone();

    let language = language.to_string();
    let handle = thread::spawn(move || {
      if interrupt_flag_thread.load(Ordering::Relaxed) {
        return;
      }
      let Ok(e) = engine.lock() else {
        return;
      };
      if let Ok(mut samples) = e.synthesize_with_options(
        &phrase,
        Some(&voice),
        crate::state::get_speed(),
        gain,
        Some(&language),
      ) {
        // sanitize output samples (prevents nasty noise if NaN/Inf/out-of-range)
        for s in &mut samples {
          if !s.is_finite() {
            *s = 0.0;
          } else {
            *s = s.clamp(-1.0, 1.0);
          }
        }
        let audio = AudioChunk {
          data: samples,
          channels: 1,
          sample_rate: 24000,
        };
        if !interrupt_flag_thread.load(Ordering::Relaxed) {
          let _ = tx.send(audio);
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
