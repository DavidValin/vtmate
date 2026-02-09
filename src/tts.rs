// ------------------------------------------------------------------
//  TTS - Text to Speech
// ------------------------------------------------------------------

use crossbeam_channel::{Receiver, Sender};
use kokoro_tiny::TtsEngine;
mod kokoro_tts;
use reqwest;
use std::io::{BufReader, Read};
use std::sync::OnceLock;
use std::sync::{
  Arc, Mutex,
  atomic::{AtomicU64, Ordering},
};
use std::thread;
use std::time::Duration;
use urlencoding;

// API
use crate::log;
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

pub fn speak(
  text: &str,
  tts: &str,
  opentts_base_url: &str,
  language: &str,
  voice: &str,
  out_sample_rate: u32, // MUST match CPAL playback SR
  tx: Sender<crate::audio::AudioChunk>,
  stop_all_rx: Receiver<()>,
  interrupt_counter: Arc<AtomicU64>,
  expected_interrupt: u64,
) -> Result<SpeakOutcome, Box<dyn std::error::Error + Send + Sync>> {
  let outcome = if tts == "opentts" {
    crate::tts::speak_via_opentts_stream(
      text,
      opentts_base_url,
      language,
      voice,
      out_sample_rate,
      tx,
      stop_all_rx,
      interrupt_counter,
      expected_interrupt,
    )
  } else {
    // NOTE: make espeak find phonemes for chinese mandarin
    let lang = if language == "zh" { "cmn" } else { language };
    crate::tts::speak_via_kokoro_stream(
      text,
      lang,
      voice,
      tx,
      stop_all_rx,
      interrupt_counter,
      expected_interrupt,
    )
  }?;
  Ok(outcome)
}

//  Kokoro Tiny TTS integration -------------------------------------
// +++++++++++++++++++++++++++++

static KOKORO_ENGINE: OnceLock<Arc<Mutex<TtsEngine>>> = OnceLock::new();

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

pub const DEFAULTKOKORO_VOICES_PER_LANGUAGE: &[(&str, &str)] = &[
  ("en", "bm_george"),
  ("es", "em_santa"),
  ("zh", "zf_xiaoni"),
  ("ja", "jm_kumo"),
  ("pt", "pf_dora"),
  ("it", "if_sara"),
  ("hi", "hf_alpha"),
  ("fr", "ff_siwis"),
];

pub fn get_all_available_languages() -> Vec<&'static str> {
  let mut langs: Vec<&str> = KOKORO_VOICES_PER_LANGUAGE
    .iter()
    .map(|(lang, _)| *lang)
    .collect();
  langs.extend(
    DEFAULT_OPENTTS_VOICES_PER_LANGUAGE
      .iter()
      .map(|(lang, _)| *lang),
  );
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
      for (lang, voice) in DEFAULT_OPENTTS_VOICES_PER_LANGUAGE.iter() {
        if *lang == language {
          return vec![*voice];
        }
      }
      Vec::new()
    }
    _ => Vec::new(),
  }
}

pub fn print_voices() {
  let langs = get_all_available_languages();
  // High Quality (Kokoro)
  println!(
    "🏆 High Quality Voices\n======================================================\n{:<8}\t{:<12}\t{:<2}\t{}",
    "TTS", "Language", "Flag", "Voices"
  );
  println!("======================================================");

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
  // Standard Quality (OpenTTS)
  println!(
    "Standard Quality Voices\n======================================================\n{:<8}\t{:<12}\t{:<2}\t{}",
    "TTS", "Language", "Flag", "Voices"
  );
  println!("======================================================");

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

pub fn speak_via_kokoro_stream(
  text: &str,
  language: &str,
  voice: &str,
  tx: Sender<crate::audio::AudioChunk>,
  stop_all_rx: Receiver<()>,
  interrupt_counter: Arc<AtomicU64>,
  expected_interrupt: u64,
) -> Result<SpeakOutcome, Box<dyn std::error::Error + Send + Sync>> {
  let engine = KOKORO_ENGINE.get_or_init(|| {
    let rt = tokio::runtime::Builder::new_current_thread()
      .enable_all()
      .build()
      .unwrap();
    let e = rt.block_on(TtsEngine::new()).unwrap();
    Arc::new(Mutex::new(e))
  });
  let mut streaming = kokoro_tts::StreamingTts::new(engine.clone());
  streaming.set_voice(voice);
  // interrupt monitoring
  let interrupt_flag = streaming.interrupt_flag.clone();
  let stop_rx = stop_all_rx.clone();
  let int_counter = interrupt_counter.clone();
  let expected = expected_interrupt;
  thread::spawn(move || {
    loop {
      if stop_rx.try_recv().is_ok() || int_counter.load(Ordering::SeqCst) != expected {
        interrupt_flag.store(true, Ordering::Relaxed);
        break;
      }
      thread::sleep(Duration::from_millis(10));
    }
  });
  let rt = tokio::runtime::Builder::new_current_thread()
    .enable_all()
    .build()?;
  let res = rt.block_on(streaming.speak_stream(text, tx.clone(), language));
  match res {
    Ok(_) => Ok(SpeakOutcome::Completed),
    Err(_) => Ok(SpeakOutcome::Interrupted),
  }
}

pub fn start_kokoro_engine() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
  let rt = tokio::runtime::Builder::new_current_thread()
    .enable_all()
    .build()?;
  let engine = rt.block_on(TtsEngine::new())?;
  KOKORO_ENGINE.set(Arc::new(Mutex::new(engine))).ok();
  Ok(())
}

//  OpenTTS integration ---------------------------------------------
// +++++++++++++++++++++++++++++

pub const DEFAULT_OPENTTS_VOICES_PER_LANGUAGE: &[(&str, &str)] = &[
  ("ar", "festival:ara_norm_ziad_hts"),
  ("bn", "flite:cmu_indic_ben_rm"),
  ("ca", "festival:upc_ca_ona_hts"),
  ("cs", "festival:czech_machac"),
  ("de", "glow-speak:de_thorsten"),
  ("el", "glow-speak:el_rapunzelina"),
  ("en", "larynx:cmu_fem-glow_tts"),
  ("es", "larynx:karen_savage-glow_tts"),
  ("fi", "glow-speak:fi_harri_tapani_ylilammi"),
  ("fr", "larynx:gilles_le_blanc-glow_tts"),
  ("gu", "flite:cmu_indic_guj_ad"),
  ("hi", "flite:cmu_indic_hin_ab"),
  ("hu", "glow-speak:hu_diana_majlinger"),
  ("it", "larynx:riccardo_fasol-glow_tts"),
  ("ja", "coqui-tts:ja_kokoro"),
  ("kn", "flite:cmu_indic_kan_plv"),
  ("ko", "glow-speak:ko_kss"),
  ("mr", "flite:cmu_indic_mar_aup"),
  ("nl", "glow-speak:nl_rdh"),
  ("pa", "flite:cmu_indic_pan_amp"),
  ("ru", "glow-speak:ru_nikolaev"),
  ("sv", "glow-speak:sv_talesyntese"),
  ("sw", "glow-speak:sw_biblia_takatifu"),
  ("ta", "flite:cmu_indic_tam_sdr"),
  ("te", "marytts:cmu-nk-hsmm"),
  ("tr", "marytts:dfki-ot-hsmm"),
  ("zh", "coqui-tts:zh_baker"),
];

pub fn speak_via_opentts_stream(
  text: &str,
  opentts_base_url: &str,
  language: &str,
  voice: &str,
  out_sample_rate: u32, // MUST match CPAL playback SR
  tx: Sender<crate::audio::AudioChunk>,
  stop_all_rx: Receiver<()>,
  interrupt_counter: Arc<AtomicU64>,
  expected_interrupt: u64,
) -> Result<SpeakOutcome, Box<dyn std::error::Error + Send + Sync>> {
  if text.is_empty() {
    return Ok(SpeakOutcome::Completed);
  }

  let url = format!(
    "{}&voice={}&lang={}&sample_rate={}&text={}",
    opentts_base_url,
    urlencoding::encode(voice),
    urlencoding::encode(language),
    out_sample_rate,
    urlencoding::encode(text)
  );

  // crate::log::log("debug", &format!("OpenTTS URL: {}", url));

  stream_wav16le_over_http(
    &url,
    tx,
    stop_all_rx,
    out_sample_rate,
    interrupt_counter,
    expected_interrupt,
  )
}

// PRIVATE
// ------------------------------------------------------------------

fn stream_wav16le_over_http(
  url: &str,
  tx: Sender<crate::audio::AudioChunk>,
  stop_all_rx: Receiver<()>,
  target_sr: u32, // MUST be playback stream SR
  interrupt_counter: Arc<AtomicU64>,
  expected_interrupt: u64,
) -> Result<SpeakOutcome, Box<dyn std::error::Error + Send + Sync>> {
  let resp = reqwest::blocking::get(url)?;
  if !resp.status().is_success() {
    return Err(format!("HTTP {} from {}", resp.status(), url).into());
  }

  let mut reader = BufReader::new(resp);

  // RIFF header
  let mut riff = [0u8; 12];
  reader.read_exact(&mut riff)?;
  if &riff[0..4] != b"RIFF" || &riff[8..12] != b"WAVE" {
    return Err("not a RIFF/WAVE file".into());
  }

  let mut channels: u16 = 0;
  let mut sample_rate: u32 = 0;
  let data_len_opt: Option<u32>;

  // Parse chunks until fmt + data
  loop {
    if stop_all_rx.try_recv().is_ok() {
      return Ok(SpeakOutcome::Interrupted);
    }

    if interrupt_counter.load(Ordering::SeqCst) != expected_interrupt {
      return Ok(SpeakOutcome::Interrupted);
    }

    let mut hdr = [0u8; 8];
    reader.read_exact(&mut hdr)?;
    let id = &hdr[0..4];
    let size = u32::from_le_bytes(hdr[4..8].try_into().unwrap());

    if id == b"fmt " {
      let mut fmt = vec![0u8; size as usize];
      reader.read_exact(&mut fmt)?;
      if fmt.len() < 16 {
        return Err("fmt chunk too small".into());
      }

      let audio_format = u16::from_le_bytes([fmt[0], fmt[1]]);
      channels = u16::from_le_bytes([fmt[2], fmt[3]]);
      sample_rate = u32::from_le_bytes([fmt[4], fmt[5], fmt[6], fmt[7]]);
      let bits_per_sample = u16::from_le_bytes([fmt[14], fmt[15]]);

      if audio_format != 1 {
        return Err(format!("unsupported WAV format {}, need PCM (1)", audio_format).into());
      }
      if bits_per_sample != 16 {
        return Err(format!("unsupported bits_per_sample {}, need 16", bits_per_sample).into());
      }
    } else if id == b"data" {
      data_len_opt = Some(size);
      break;
    } else {
      let mut skip = vec![0u8; size as usize];
      reader.read_exact(&mut skip)?;
    }

    if size % 2 == 1 {
      let mut pad = [0u8; 1];
      reader.read_exact(&mut pad)?;
    }
  }

  let data_len = data_len_opt.ok_or("missing data chunk")?;
  if channels == 0 || sample_rate == 0 {
    return Err("missing WAV fmt info".into());
  }
  // crate::log::log(
  //   "debug",
  //   &format!(
  //     "OpenTTS WAV: PCM16LE, {} ch @ {} Hz, data {} bytes (target {} Hz)",
  //     channels, sample_rate, data_len, target_sr
  //   ),
  // );

  // IMPORTANT: Don't `read_exact(data_len)` in one shot.
  let samples_per_chunk = CHUNK_FRAMES * channels as usize;

  if sample_rate == target_sr {
    let mut remaining = data_len as usize;
    let mut pending: Vec<f32> = Vec::with_capacity(samples_per_chunk * 2);
    let mut buf = vec![0u8; 8192];

    while remaining > 0 {
      if stop_all_rx.try_recv().is_ok() {
        return Ok(SpeakOutcome::Interrupted);
      }
      if interrupt_counter.load(Ordering::SeqCst) != expected_interrupt {
        return Ok(SpeakOutcome::Interrupted);
      }

      let want = remaining.min(buf.len());
      let mut read_bytes = 0usize;
      while read_bytes < want {
        let n = reader.read(&mut buf[read_bytes..want])?;
        if n == 0 {
          break;
        }
        read_bytes += n;
      }
      if read_bytes < want {
        return Err(
          format!(
            "failed to fill whole buffer: expected {} bytes, got {}",
            want, read_bytes
          )
          .into(),
        );
      }
      remaining -= want;

      // Read all PCM data first
      let mut pcm = Vec::new();
      reader.read_to_end(&mut pcm)?;
      if stop_all_rx.try_recv().is_ok() {
        return Ok(SpeakOutcome::Interrupted);
      }
      if interrupt_counter.load(Ordering::SeqCst) != expected_interrupt {
        return Ok(SpeakOutcome::Interrupted);
      }
      // Decode PCM16LE -> f32
      let mut decoded: Vec<f32> = Vec::with_capacity(pcm.len() / 2);
      for i in (0..pcm.len()).step_by(2) {
        let s = i16::from_le_bytes([pcm[i], pcm[i + 1]]);
        decoded.push(s as f32 / 32768.0);
      }
      // Resample once
      let resampled = crate::audio::resample_to(&decoded, channels, sample_rate, target_sr);
      // Normalize to avoid volume drift
      let max_val = resampled.iter().map(|v| v.abs()).fold(0.0, f32::max);
      let factor = if max_val > 1.0 { 1.0 / max_val } else { 1.0 };
      let resampled: Vec<f32> = resampled.into_iter().map(|v| v * factor).collect();
      let mut offset = 0usize;
      while offset < resampled.len() {
        if stop_all_rx.try_recv().is_ok() {
          return Ok(SpeakOutcome::Interrupted);
        }
        if interrupt_counter.load(Ordering::SeqCst) != expected_interrupt {
          return Ok(SpeakOutcome::Interrupted);
        }
        let end = (offset + samples_per_chunk).min(resampled.len());
        let mut data = resampled[offset..end].to_vec();
        let aligned = data.len() - (data.len() % channels as usize);
        if aligned == 0 {
          break;
        }
        data.truncate(aligned);
        tx.send(crate::audio::AudioChunk {
          data,
          channels,
          sample_rate: target_sr,
        })?;
        offset = end;
      }
    }

    let aligned = pending.len() - (pending.len() % channels as usize);
    pending.truncate(aligned);
    if !pending.is_empty() {
      if stop_all_rx.try_recv().is_ok() {
        return Ok(SpeakOutcome::Interrupted);
      }
      if interrupt_counter.load(Ordering::SeqCst) != expected_interrupt {
        return Ok(SpeakOutcome::Interrupted);
      }
      tx.send(crate::audio::AudioChunk {
        data: pending,
        channels,
        sample_rate: target_sr,
      })?;
    }
  } else {
    let mut pcm = vec![0u8; data_len as usize];
    let mut read_bytes = 0usize;
    while read_bytes < pcm.len() {
      let n = reader.read(&mut pcm[read_bytes..])?;
      if n == 0 {
        break;
      }
      read_bytes += n;
    }
    if read_bytes < pcm.len() {
      return Err(
        format!(
          "failed to read PCM data: expected {} bytes, got {}",
          pcm.len(),
          read_bytes
        )
        .into(),
      );
    }
    if stop_all_rx.try_recv().is_ok() {
      return Ok(SpeakOutcome::Interrupted);
    }
    if interrupt_counter.load(Ordering::SeqCst) != expected_interrupt {
      return Ok(SpeakOutcome::Interrupted);
    }

    let mut decoded: Vec<f32> = Vec::with_capacity(pcm.len() / 2);
    for i in (0..pcm.len()).step_by(2) {
      let s = i16::from_le_bytes([pcm[i], pcm[i + 1]]);
      decoded.push(s as f32 / 32768.0);
    }
    let mut resampled = crate::audio::resample_to(&decoded, channels, sample_rate, target_sr);
    // normalize to fixed peak level
    let max_val = resampled.iter().map(|v| v.abs()).fold(0.0, f32::max);
    let target_peak = 0.95_f32;
    let factor = if max_val > 0.0 {
      target_peak / max_val
    } else {
      1.0
    };
    resampled = resampled.into_iter().map(|v| v * factor).collect();
    // log::log("debug", &format!("Resampled length: {}", resampled.len()));
    // send entire resampled audio as one chunk
    let aligned_len = resampled.len() - (resampled.len() % channels as usize);
    let data = if aligned_len > 0 {
      resampled[..aligned_len].to_vec()
    } else {
      Vec::new()
    };
    tx.send(crate::audio::AudioChunk {
      data,
      channels,
      sample_rate: target_sr,
    })?;
  }

  Ok(SpeakOutcome::Completed)
}
