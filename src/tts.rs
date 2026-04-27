// ------------------------------------------------------------------
//  TTS - Text to Speech
// ------------------------------------------------------------------

use crate::state::GLOBAL_STATE;
use crate::tts::kokoro_tts::KOKORO_VOICES_PER_LANGUAGE;
use crossbeam_channel::{Receiver, Sender};
use kokoro_micro::TtsEngine;
extern crate supersonic2_tts as supersonic2_tts_crate;
use supersonic2_tts_crate::TtsEngine as SupersonicTtsEngine;
pub mod kokoro_tts;
pub mod opentts_tts;
pub mod supersonic2_tts;

use std::sync::OnceLock;
use std::sync::{Arc, Mutex, atomic::AtomicU64};

// API
// ------------------------------------------------------------------

// TUNABLES
// ------------------------------------------------------------------

pub const CHUNK_FRAMES: usize = 1024; // Frames per chunk (per-channel interleaved)
pub const QUEUE_CAP_FRAMES: usize = 48_000 * 15; // Playback queue capacity in frames at output SR; 15 seconds worth (scaled by channels)

/// Result of attempting to synthesize/stream a TTS phrase.
/// We distinguish a clean completion from a user interruption so the
/// conversation thread can reliably print "USER interrupted" and stop
/// emitting further assistant output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpeakOutcome {
  Completed,
  Interrupted,
}

static KOKORO_ENGINE: OnceLock<Arc<Mutex<TtsEngine>>> = OnceLock::new();
static SUPSONIC_ENGINE: OnceLock<Arc<Mutex<SupersonicTtsEngine>>> = OnceLock::new();

// Supported languages for Supersonic2 TTS
static SUPSONIC_LANGS: &[&str] = &["en", "es", "fr", "ko", "pt"];

pub fn speak(
  text: &str,
  tts: &str,
  opentts_base_url: &str,
  language: &str,
  voice: &str,
  out_sample_rate: u32, // MUST match CPAL playback SR
  tx: Sender<crate::audio::AudioChunk>,
  interrupt_counter: Arc<AtomicU64>,
  expected_interrupt: u64,
) -> Result<SpeakOutcome, Box<dyn std::error::Error + Send + Sync>> {
  let outcome = if tts == "opentts" {
    opentts_tts::speak_via_opentts(
      text,
      opentts_base_url,
      language,
      voice,
      out_sample_rate,
      tx,
      interrupt_counter,
      expected_interrupt,
    )
  } else if tts == "supersonic2" {
    let speed = crate::state::get_speed();
    let gain = 1.0;
    supersonic2_tts::speak_via_supersonic2(
      text,
      voice,
      speed,
      gain,
      language,
      tx,
      interrupt_counter,
      expected_interrupt,
    )
  } else {
    let lang = if language == "zh" { "cmn" } else { language };
    kokoro_tts::speak_via_kokoro(text, lang, voice, tx, interrupt_counter, expected_interrupt)
  }?;
  Ok(outcome)
}

// tts_thread - dedicated thread for speaking phrases
pub fn tts_thread(
  out_sample_rate: u32,
  tx_play: Sender<crate::audio::AudioChunk>,
  interrupt_counter: Arc<AtomicU64>,
  rx_tts: Receiver<(String, u64, String)>,
  stop_play_tx: Sender<()>,
  tx_tts_done: Sender<()>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
  loop {
    crate::log::log("info", "🔄 TTS thread waiting for next phrase...");
    // Wait for either a new phrase or a stop signal
    crossbeam_channel::select! {
      recv(rx_tts) -> msg => {
        let (phrase, expected_interrupt, voice) = match msg {
          Ok(v) => v,
          Err(_) => break,
        };
        let state = GLOBAL_STATE.get().expect("AppState not initialized");
        // crate::log::log("info", &format!("TTS received phrase (len={}), expected_interrupt={}", phrase.len(), expected_interrupt));

        let tts_val = state.tts.lock().unwrap().clone();
        let language = state.language.lock().unwrap().clone();

        // Use OPENTTS_BASE_URL_DEFAULT when TTS is set to opentts
        let opentts_url = if tts_val == "opentts" {
          crate::config::OPENTTS_BASE_URL_DEFAULT.to_string()
        } else {
          state.baseurl.lock().unwrap().clone()
        };

        let outcome = crate::tts::speak(
          &phrase,
          &tts_val,
          &opentts_url,
          &language,
          &voice,
          out_sample_rate,
          tx_play.clone(),
          interrupt_counter.clone(),
          expected_interrupt,
        );

        match outcome {
          Ok(o) => {
            if o == crate::tts::SpeakOutcome::Interrupted {
              // Drain any remaining phrases that might be queued
              let mut drained = 0;
              loop {
                match rx_tts.try_recv() {
                  Ok(_) => {
                    drained += 1;
                    continue;
                  },
                  Err(_) => break,
                }
              }
              let _ = stop_play_tx.try_send(());
              // Signal completion before continuing
              let _ = tx_tts_done.try_send(());
              continue;
            }
            let _ = tx_tts_done.try_send(());
          }
          Err(_e) => {
            crate::log::log("error", &format!("TTS error. Can't play audio speech. Make sure OpenTTS is running: docker run --rm -p 5500:5500 synesthesiam/opentts:all"));
            // Signal completion before breaking
            let _ = tx_tts_done.try_send(());
            break;
          }
        }
      }
    }
  }

  Ok(())
}

pub fn get_all_available_languages() -> Vec<&'static str> {
  let mut langs: Vec<&str> = KOKORO_VOICES_PER_LANGUAGE
    .iter()
    .map(|(lang, _)| *lang)
    .collect();
  langs.extend(
    crate::tts::opentts_tts::DEFAULT_OPENTTS_VOICES_PER_LANGUAGE
      .iter()
      .map(|(lang, _)| *lang),
  );
  // Include supersonic2 supported languages
  langs.extend(SUPSONIC_LANGS.iter().copied());
  langs.sort();
  langs.dedup();
  langs
}

pub fn get_voices_for(tts: &str, language: &str) -> Vec<&'static str> {
  match tts {
    "kokoro" => {
      for (lang, voices) in KOKORO_VOICES_PER_LANGUAGE.iter() {
        if *lang == language {
          return voices.to_vec();
        }
      }
      Vec::new()
    }
    "opentts" => {
      for (lang, voice) in crate::tts::opentts_tts::DEFAULT_OPENTTS_VOICES_PER_LANGUAGE.iter() {
        if *lang == language {
          return vec![*voice];
        }
      }
      Vec::new()
    }
    "supersonic2" => {
      // Supersonic2 voices are supported only for specific languages
      let supersonic_voices = crate::tts::supersonic2_tts::SUPERSONIC2_VOICE_STYLES;
      if SUPSONIC_LANGS.contains(&language) {
        supersonic_voices.to_vec()
      } else {
        Vec::new()
      }
    }
    _ => Vec::new(),
  }
}

pub fn print_voices() {
  let langs = get_all_available_languages();

  println!(
    "supersonic2 🏆 High Quality Voices\n======================================================\n{:<8}\t{:<12}\t{:<2}\t{}",
    "TTS", "Language", "Flag", "Voices"
  );
  println!("======================================================");
  // kokoro
  for lang in langs.iter() {
    let voices = get_voices_for("supersonic2", lang);
    if voices.is_empty() {
      continue;
    }
    let flag = crate::util::get_flag(lang);
    let voices_str = voices.join(", ");
    println!(
      "{:<8}\t{:<12}\t{:<2}\t{}",
      "supersonic2", lang, flag, voices_str
    );
  }
  println!();
  println!(
    "Standard Quality Voices\n======================================================\n{:<8}\t{:<12}\t{:<2}\t{}",
    "TTS", "Language", "Flag", "Voices"
  );
  println!();
  println!();

  println!(
    "kokoro 🏆 High Quality Voices\n======================================================\n{:<8}\t{:<12}\t{:<2}\t{}",
    "TTS", "Language", "Flag", "Voices"
  );
  println!("======================================================");
  // kokoro
  for lang in langs.iter() {
    let voices = get_voices_for("kokoro", lang);
    if voices.is_empty() {
      continue;
    }
    let flag = crate::util::get_flag(lang);
    let voices_str = voices.join(", ");
    println!("{:<8}\t{:<12}\t{:<2}\t{}", "kokoro", lang, flag, voices_str);
  }
  println!();
  println!();

  println!("======================================================");
  // OpenTTS
  for lang in langs.iter() {
    let voices = get_voices_for("opentts", lang);
    if voices.is_empty() {
      continue;
    }
    let flag = crate::util::get_flag(lang);
    let voices_str = voices.join(", ");
    println!(
      "{:<8}\t{:<12}\t{:<2}\t{}",
      "opentts", lang, flag, voices_str
    );
  }
}
