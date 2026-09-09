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
pub mod supertonic_tts;

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
/// Replaceable, unlike the two above: when the GPU refuses mid-synthesis the
/// engine is rebuilt on the CPU in place (supertonic_tts::rebuild_on_cpu).
static SUPERTONIC_ENGINE: Mutex<Option<Arc<supertonic3_tts::TtsEngine>>> = Mutex::new(None);

// Supported languages for Supersonic2 TTS
static SUPSONIC_LANGS: &[&str] = &["en", "es", "fr", "ko", "pt"];
// Supported languages for Supertonic TTS (Supertonic 3, multilingual)
static SUPERTONIC_LANGS: &[&str] = crate::tts::supertonic_tts::SUPPORTED_LANGS;

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
  } else if tts == "supertonic" {
    let speed = crate::state::get_speed();
    supertonic_tts::speak_via_supertonic(
      text,
      voice,
      speed,
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
  tx_tts_done: Sender<u64>,
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
        // Anything queued before the last interruption belongs to a turn the
        // user has moved on from: drop it, and everything else waiting behind
        // it, instead of speaking it. Without this, moving to another phrase
        // still had to sit through the synthesis of the one left behind.
        if interrupt_counter.load(std::sync::atomic::Ordering::SeqCst) != expected_interrupt {
          crate::log::log(
            "debug",
            "TTS: phrase from a cancelled turn, not spoken",
          );
          let _ = stop_play_tx.try_send(());
          let _ = tx_tts_done.try_send(expected_interrupt);
          continue;
        }
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
        let synth_started = std::time::Instant::now();
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
        crate::log::log(
          "debug",
          &format!(
            "TTS synthesized phrase ({} chars) in {:.2}s",
            phrase.chars().count(),
            synth_started.elapsed().as_secs_f32()
          ),
        );
        match outcome {
          Ok(o) => {
            if o == crate::tts::SpeakOutcome::Interrupted {
              // Do not empty the queue here. Whatever was queued before the
              // interruption is recognised and discarded as it is taken off
              // the queue, one by one, while a phrase queued *after* it (the
              // one the user just chose) is still wanted. Emptying the queue
              // threw that one away too, and whoever was waiting for it then
              // waited for a phrase that no longer existed.
              let _ = stop_play_tx.try_send(());
              // Signal completion before continuing
              let _ = tx_tts_done.try_send(expected_interrupt);
              continue;
            }
            let _ = tx_tts_done.try_send(expected_interrupt);
          }
          Err(e) => {
            // The OpenTTS hint only makes sense for the OpenTTS engine; on a
            // local engine it sent people chasing a container that was never
            // involved, while the real cause went unnamed.
            let hint = if tts_val == "opentts" {
              " - make sure OpenTTS is running: docker run --rm -p 5500:5500 synesthesiam/opentts:all"
            } else {
              ""
            };
            crate::log::log(
              "error",
              &format!("TTS error [{}]. Can't play audio speech: {}{}", tts_val, e, hint),
            );
            // Signal completion so callers waiting on this phrase don't hang, but keep
            // the thread alive — a transient failure (e.g. OpenTTS briefly unreachable)
            // shouldn't permanently kill voice output or drop rx_tts, which would make
            // read-file mode's tx_tts.send(...).unwrap() panic on the next phrase.
            let _ = tx_tts_done.try_send(expected_interrupt);
            continue;
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
  // Include supertonic supported languages
  langs.extend(SUPERTONIC_LANGS.iter().copied());
  langs.sort();
  langs.dedup();
  langs
}

pub fn get_voices_for(tts: &str, language: &str) -> Vec<String> {
  let owned = |v: &[&str]| -> Vec<String> { v.iter().map(|s| s.to_string()).collect() };
  match tts {
    "kokoro" => {
      for (lang, voices) in KOKORO_VOICES_PER_LANGUAGE.iter() {
        if *lang == language {
          return owned(voices);
        }
      }
      Vec::new()
    }
    "opentts" => {
      for (lang, voice) in crate::tts::opentts_tts::DEFAULT_OPENTTS_VOICES_PER_LANGUAGE.iter() {
        if *lang == language {
          return vec![voice.to_string()];
        }
      }
      Vec::new()
    }
    "supersonic2" => {
      // Supersonic2 voices are supported only for specific languages
      if SUPSONIC_LANGS.contains(&language) {
        voice_styles_in(
          crate::tts::supersonic2_tts::voice_styles_dir(),
          &crate::tts::supersonic2_tts::SUPERSONIC2_VOICE_STYLES,
        )
      } else {
        Vec::new()
      }
    }
    "supertonic" => {
      // Supertonic voices are supported for all its languages
      if SUPERTONIC_LANGS.contains(&language) {
        voice_styles_in(
          crate::tts::supertonic_tts::voice_styles_dir(),
          &crate::tts::supertonic_tts::SUPERTONIC_VOICE_STYLES,
        )
      } else {
        Vec::new()
      }
    }
    _ => Vec::new(),
  }
}

/// Where an engine keeps its voice files, for engines that have one file per
/// voice on disk. `None` for engines whose voices are fixed (kokoro, opentts).
pub fn voice_styles_dir_for(tts: &str) -> Option<std::path::PathBuf> {
  match tts {
    "supertonic" => Some(crate::tts::supertonic_tts::voice_styles_dir()),
    "supersonic2" => Some(crate::tts::supersonic2_tts::voice_styles_dir()),
    _ => None,
  }
}

/// The voices an engine offers, read from its `voice_styles` directory: one
/// `<voice>.json` per voice, so dropping a file in there adds a voice and
/// `--list-voices`, the settings validator and playback all see it.
///
/// `builtin` is used when the directory cannot be read yet (models not
/// unpacked, custom install), so the shipped voices are always offered.
pub fn voice_styles_in(dir: std::path::PathBuf, builtin: &[&str]) -> Vec<String> {
  let mut names: Vec<String> = Vec::new();
  if let Ok(entries) = std::fs::read_dir(&dir) {
    for entry in entries.flatten() {
      let path = entry.path();
      // `.json`, any case: Windows and macOS filesystems are case-insensitive
      let is_json = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("json"));
      if !is_json || !path.is_file() {
        continue;
      }
      let Some(name) = path.file_stem().and_then(|s| s.to_str()) else {
        continue; // non-UTF-8 file name
      };
      // hidden files, and the "._name" companions macOS writes on some volumes
      if name.is_empty() || name.starts_with('.') {
        continue;
      }
      names.push(name.to_string());
    }
  }
  if names.is_empty() {
    return builtin.iter().map(|s| s.to_string()).collect();
  }
  names.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase()).then(a.cmp(b)));
  names.dedup();
  names
}

pub fn print_voices() {
  let langs = get_all_available_languages();

  println!(
    "supertonic 🏆 High Quality Voices\n======================================================\n{:<8}\t{:<12}\t{:<2}\t{}",
    "TTS", "Language", "Flag", "Voices"
  );
  println!("======================================================");
  for lang in langs.iter() {
    let voices = get_voices_for("supertonic", lang);
    if voices.is_empty() {
      continue;
    }
    let flag = crate::util::get_flag(lang);
    let voices_str = voices.join(", ");
    println!(
      "{:<8}\t{:<12}\t{:<2}\t{}",
      "supertonic", lang, flag, voices_str
    );
  }
  print_voice_styles_hint("supertonic");
  println!();
  println!(
    "supersonic2 🏆 High Quality Voices\n======================================================\n{:<8}\t{:<12}\t{:<2}\t{}",
    "TTS", "Language", "Flag", "Voices"
  );
  println!("======================================================");
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
  print_voice_styles_hint("supersonic2");
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

/// Tell the user where an engine reads its voices from, so custom ones can be
/// added by dropping a file next to the shipped ones.
fn print_voice_styles_hint(tts: &str) {
  if let Some(dir) = voice_styles_dir_for(tts) {
    println!(
      "add your own {} voices as <name>.json in {}",
      tts,
      dir.display()
    );
  }
}
