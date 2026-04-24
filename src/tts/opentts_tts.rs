// ------------------------------------------------------------------
//  OpenTTS tts
// ------------------------------------------------------------------

use crossbeam_channel::{Receiver, Sender};
use reqwest;
use std::io::{BufReader, Read};
use std::sync::{
  Arc,
  atomic::{AtomicU64, Ordering},
};
use urlencoding;

use crate::audio::{AudioChunk, resample_to};
use crate::log::log;

// API
// ------------------------------------------------------------------

pub fn speak_via_opentts(
  text: &str,
  opentts_base_url: &str,
  language: &str,
  voice: &str,
  out_sample_rate: u32,
  tx: Sender<AudioChunk>,
  stop_all_rx: Receiver<()>,
  interrupt_counter: Arc<AtomicU64>,
  expected_interrupt: u64,
) -> Result<crate::tts::SpeakOutcome, Box<dyn std::error::Error + Send + Sync>> {
  if text.is_empty() {
    return Ok(crate::tts::SpeakOutcome::Completed);
  }

  let url = format!(
    "{}&voice={}&lang={}&sample_rate={}&text={}",
    opentts_base_url,
    urlencoding::encode(voice),
    urlencoding::encode(language),
    out_sample_rate,
    urlencoding::encode(text),
  );

  stream_wav16le_over_http(
    &url,
    tx,
    stop_all_rx.clone(),
    out_sample_rate,
    interrupt_counter,
    expected_interrupt,
  )
}

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

// PRIVATE
// ------------------------------------------------------------------

fn read_exact_in_chunks<R: Read>(
  reader: &mut R,
  total: usize,
  stop_all_rx: &Receiver<()>,
  interrupt_counter: &Arc<AtomicU64>,
  expected_interrupt: u64,
) -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> {
  let mut remaining = total;
  let mut buf = vec![0u8; 8192];
  let mut out = Vec::with_capacity(total);
  while remaining > 0 {
    if stop_all_rx.try_recv().is_ok()
      || interrupt_counter.load(Ordering::SeqCst) != expected_interrupt
    {
      return Err("Interrupted while reading wav data".into());
    }
    let to_read = std::cmp::min(remaining, buf.len());
    let n = reader.read(&mut buf[..to_read])?;
    if n == 0 {
      return Err("Unexpected EOF while reading wav data".into());
    }
    out.extend_from_slice(&buf[..n]);
    remaining -= n;
  }
  Ok(out)
}

fn stream_wav16le_over_http(
  url: &str,
  tx: Sender<AudioChunk>,
  stop_all_rx: Receiver<()>,
  target_sr: u32,
  interrupt_counter: Arc<AtomicU64>,
  expected_interrupt: u64,
) -> Result<crate::tts::SpeakOutcome, Box<dyn std::error::Error + Send + Sync>> {
  let resp = reqwest::blocking::get(url)?;
  if stop_all_rx.try_recv().is_ok()
    || interrupt_counter.load(Ordering::SeqCst) != expected_interrupt
  {
    return Ok(crate::tts::SpeakOutcome::Interrupted);
  }
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
      return Ok(crate::tts::SpeakOutcome::Interrupted);
    }

    if interrupt_counter.load(Ordering::SeqCst) != expected_interrupt {
      return Ok(crate::tts::SpeakOutcome::Interrupted);
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
  log(
    "info",
    &format!(
      "OpenTTS WAV: PCM16LE, {} ch @ {} Hz, data {} bytes (target {} Hz)",
      channels, sample_rate, data_len, target_sr
    ),
  );

  // IMPORTANT: Don't `read_exact(data_len)` in one shot.
  let samples_per_chunk = crate::tts::CHUNK_FRAMES * channels as usize;

  if sample_rate == target_sr {
    let mut remaining = data_len as usize;
    let mut pending: Vec<f32> = Vec::with_capacity(samples_per_chunk * 2);
    let mut buf = vec![0u8; 8192];

    while remaining > 0 {
      if stop_all_rx.try_recv().is_ok() {
        return Ok(crate::tts::SpeakOutcome::Interrupted);
      }
      if interrupt_counter.load(Ordering::SeqCst) != expected_interrupt {
        return Ok(crate::tts::SpeakOutcome::Interrupted);
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
      let pcm = match read_exact_in_chunks(
        &mut reader,
        remaining,
        &stop_all_rx,
        &interrupt_counter,
        expected_interrupt,
      ) {
        Ok(v) => v,
        Err(e) => return Err(e),
      };
      // After reading the rest, no bytes left
      remaining = 0;
      if stop_all_rx.try_recv().is_ok() {
        return Ok(crate::tts::SpeakOutcome::Interrupted);
      }
      if interrupt_counter.load(Ordering::SeqCst) != expected_interrupt {
        return Ok(crate::tts::SpeakOutcome::Interrupted);
      }
      // Decode PCM16LE -> f32
      let mut decoded: Vec<f32> = Vec::with_capacity(pcm.len() / 2);
      for i in (0..pcm.len()).step_by(2) {
        let s = i16::from_le_bytes([pcm[i], pcm[i + 1]]);
        decoded.push(s as f32 / 32768.0);
      }
      // Resample once
      let resampled = resample_to(&decoded, channels, sample_rate, target_sr);
      // Normalize to avoid volume drift
      let max_val = resampled.iter().map(|v| v.abs()).fold(0.0, f32::max);
      let factor = if max_val > 1.0 { 1.0 / max_val } else { 1.0 };
      let resampled: Vec<f32> = resampled.into_iter().map(|v| v * factor).collect();
      let mut offset = 0usize;
      while offset < resampled.len() {
        if stop_all_rx.try_recv().is_ok() {
          return Ok(crate::tts::SpeakOutcome::Interrupted);
        }
        if interrupt_counter.load(Ordering::SeqCst) != expected_interrupt {
          return Ok(crate::tts::SpeakOutcome::Interrupted);
        }
        let end = (offset + samples_per_chunk).min(resampled.len());
        let mut data = resampled[offset..end].to_vec();
        let aligned = data.len() - (data.len() % channels as usize);
        if aligned == 0 {
          break;
        }
        data.truncate(aligned);
        tx.send(AudioChunk {
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
        return Ok(crate::tts::SpeakOutcome::Interrupted);
      }
      if interrupt_counter.load(Ordering::SeqCst) != expected_interrupt {
        return Ok(crate::tts::SpeakOutcome::Interrupted);
      }
      tx.send(AudioChunk {
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
      return Ok(crate::tts::SpeakOutcome::Interrupted);
    }
    if interrupt_counter.load(Ordering::SeqCst) != expected_interrupt {
      return Ok(crate::tts::SpeakOutcome::Interrupted);
    }

    let mut decoded: Vec<f32> = Vec::with_capacity(pcm.len() / 2);
    for i in (0..pcm.len()).step_by(2) {
      let s = i16::from_le_bytes([pcm[i], pcm[i + 1]]);
      decoded.push(s as f32 / 32768.0);
    }
    let mut resampled = resample_to(&decoded, channels, sample_rate, target_sr);
    // normalize to fixed peak level
    let max_val = resampled.iter().map(|v| v.abs()).fold(0.0, f32::max);
    let target_peak = 0.95_f32;
    let factor = if max_val > 0.0 {
      target_peak / max_val
    } else {
      1.0
    };
    resampled = resampled.into_iter().map(|v| v * factor).collect();
    // send entire resampled audio as one chunk
    let aligned_len = resampled.len() - (resampled.len() % channels as usize);
    let data = if aligned_len > 0 {
      resampled[..aligned_len].to_vec()
    } else {
      Vec::new()
    };
    tx.send(AudioChunk {
      data,
      channels,
      sample_rate: target_sr,
    })?;
  }

  Ok(crate::tts::SpeakOutcome::Completed)
}
