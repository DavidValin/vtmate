// ------------------------------------------------------------------
//  TTS - Text to Speech
// ------------------------------------------------------------------

use crossbeam_channel::{Receiver, Sender};
use kokoro_tiny::TtsEngine;
use reqwest;
use std::io::{BufReader, Read};
use std::sync::{
  atomic::{AtomicU64, Ordering},
  Arc,
};
use urlencoding;

// API
// ------------------------------------------------------------------

// TUNABLES
// ------------------------------------------------------------------

pub const CHUNK_FRAMES: usize = 512; // Frames per chunk (per-channel interleaved)
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

//  Kokoro Tiny TTS integration -------------------------------------
// +++++++++++++++++++++++++++++

// ALL voices available per language
// ----------------------------------
// AMERICAN ENGLISH (a)
// -----------------
// Female
//   af_alloy
//   af_aoede
//   af_bella
//   af_heart
//   af_jessica
//   af_kore
//   af_nicole
//   af_nova
//   af_river
//   af_sarah
//   af_sky

// Male
//   am_adam
//   am_echo
//   am_eric
//   am_fenrir
//   am_liam
//   am_michael
//   am_onyx
//   am_puck
//   am_santa

// BRITISH ENGLISH (b)
// -----------------
// Female
//   bf_alice
//   bf_emma
//   bf_isabella
//   bf_lily

// Male
//   bm_daniel
//   bm_fable
//   bm_george
//   bm_lewis

// SPANISH (e)
// -----------------
// Female
//   ef_dora

// Male
//   em_alex
//   em_santa

// FRENCH (f)

// Female
//   ff_siwis

// HINDI (h)
// -----------------
// Female
//   hf_alpha
//   hf_beta

// Male
//   hm_omega
//   hm_psi

// ITALIAN (i)
// -----------------
// Female
//   if_sara

// Male
//   im_nicola

// PORTUGUESE – BRAZIL (p)
// -----------------
// Female
//   pf_dora

// Male
//   pm_alex
//   pm_santa

// JAPANESE (j)
// -----------------
// Female
//   jf_alpha
//   jf_gongitsune
//   jf_nezumi
//   jf_tebukuro

// Male
//   jm_kumo

// MANDARIN CHINESE (z)
// -----------------
// Female
//   zf_xiaobei
//   zf_xiaoni
//   zf_xiaoxiao
//   zf_xiaoyi

// Male
//   zm_yunjian
//   zm_yunxi
//   zm_yunxia
//   zm_yunyang

pub const DEFAULT_KOKORO_VOICES_PER_LANGUAGE: &[(&str, &str)] = &[
  ("ar", ""),
  ("bn", ""),
  ("ca", ""),
  ("cs", ""),
  ("de", ""),
  ("el", ""),
  ("en", "af_sky"),
  ("es", "em_alex"),
  ("fi", ""),
  ("fr", ""),
  ("gu", ""),
  ("hi", ""),
  ("hu", ""),
  ("it", ""),
  ("ja", ""),
  ("kn", ""),
  ("ko", ""),
  ("mr", ""),
  ("nl", ""),
  ("pa", ""),
  ("ru", ""),
  ("sv", ""),
  ("sw", ""),
  ("ta", ""),
  ("te", ""),
  ("tr", ""),
  ("zh", ""),
];

pub fn speak_via_kokoro(
  text: &str,
  language: &str, // "en", "es", "fr", "it", "pt", "ja", "zh", ...
  voice: &str,
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

  let mut engine = rt.block_on(TtsEngine::new())?;

  let samples: Vec<f32> = engine
    .synthesize(text, Some(voice), Some(language))
    .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { e.into() })?;

  // Kokoro always returns 24 kHz PCM. No resampling needed here.
  let chunk = crate::audio::AudioChunk {
    data: samples,
    channels: 1,
    sample_rate: 24_000, // kokoro-tiny output
  };

  tx.send(chunk)?;
  Ok(SpeakOutcome::Completed)
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

pub fn speak_via_opentts(
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
  crate::log::log(
    "info",
    &format!(
      "OpenTTS WAV: PCM16LE, {} ch @ {} Hz, data {} bytes (target {} Hz)",
      channels, sample_rate, data_len, target_sr
    ),
  );

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
      reader.read_exact(&mut buf[..want])?;
      remaining -= want;

      // Decode PCM16LE -> f32 (interleaved)
      let mut i = 0usize;
      while i + 1 < want {
        let s = i16::from_le_bytes([buf[i], buf[i + 1]]);
        pending.push(s as f32 / 32768.0);
        i += 2;
      }

      // Flush full chunks to playback.
      while pending.len() >= samples_per_chunk {
        if stop_all_rx.try_recv().is_ok() {
          return Ok(SpeakOutcome::Interrupted);
        }
        if interrupt_counter.load(Ordering::SeqCst) != expected_interrupt {
          return Ok(SpeakOutcome::Interrupted);
        }

        let mut data = pending.drain(..samples_per_chunk).collect::<Vec<f32>>();
        // frame-align
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
    reader.read_exact(&mut pcm)?;
    if stop_all_rx.try_recv().is_ok() {
      return Ok(SpeakOutcome::Interrupted);
    }
    if interrupt_counter.load(Ordering::SeqCst) != expected_interrupt {
      return Ok(SpeakOutcome::Interrupted);
    }
    let mut decoded = Vec::with_capacity(pcm.len() / 2);
    for i in (0..pcm.len()).step_by(2) {
      let s = i16::from_le_bytes([pcm[i], pcm[i + 1]]);
      decoded.push(s as f32 / 32768.0);
    }
    let decoded =
      crate::audio::resample_interleaved_linear(&decoded, channels, sample_rate, target_sr);
    let mut offset = 0usize;
    while offset < decoded.len() {
      if stop_all_rx.try_recv().is_ok() {
        return Ok(SpeakOutcome::Interrupted);
      }
      if interrupt_counter.load(Ordering::SeqCst) != expected_interrupt {
        return Ok(SpeakOutcome::Interrupted);
      }
      let end = (offset + samples_per_chunk).min(decoded.len());
      let mut data = decoded[offset..end].to_vec();
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

  Ok(SpeakOutcome::Completed)
}
