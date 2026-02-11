// ------------------------------------------------------------------
//  Record
// ------------------------------------------------------------------

use cpal::traits::{DeviceTrait, StreamTrait};
use crossbeam_channel::{Receiver, Sender};
use std::sync::OnceLock;
use std::sync::{
  Arc, Mutex,
  atomic::{AtomicBool, AtomicU64, Ordering},
};
use std::thread;
use std::time::{Duration, Instant};

// API
// ------------------------------------------------------------------

pub fn record_thread(
  start_instant: &'static OnceLock<Instant>,
  device: cpal::Device,
  supported: cpal::SupportedStreamConfig,
  config: cpal::StreamConfig,
  tx: Sender<crate::audio::AudioChunk>,     // mic -> router
  tx_utt: Sender<crate::audio::AudioChunk>, // utterance -> conversation
  vad_thresh: f32,
  end_silence_ms: u64,
  playback_active: Arc<AtomicBool>,
  gate_until_ms: Arc<AtomicU64>,
  stop_play_tx: Sender<()>,
  interrupt_counter: Arc<AtomicU64>,
  stop_all_rx: Receiver<()>,
  peak: Arc<Mutex<f32>>,
  ui: crate::state::UiState,
  volume: Arc<Mutex<f32>>,
  recording_paused: Arc<AtomicBool>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
  use cpal::SampleFormat;

  let channels = config.channels;
  let sample_rate = config.sample_rate.0;
  let sample_format = supported.sample_format();

  let min_utt_ms =
    crate::util::env_u64("MIN_UTTERANCE_MS", crate::config::MIN_UTTERANCE_MS_DEFAULT);
  let hangover_ms = crate::util::env_u64("HANGOVER_MS", crate::config::HANGOVER_MS_DEFAULT);

  // chunker for mic->router
  let accum: Arc<Mutex<Vec<f32>>> = Arc::new(Mutex::new(Vec::with_capacity(
    crate::tts::CHUNK_FRAMES * channels as usize,
  )));

  // utterance capture state
  let utt_buf: Arc<Mutex<Vec<f32>>> = Arc::new(Mutex::new(Vec::new()));
  let in_speech = Arc::new(AtomicBool::new(false));
  let last_voice_ms = Arc::new(AtomicU64::new(0));

  // debounced stop signal
  let stop_sent = Arc::new(AtomicBool::new(false));

  let err_fn = |e| crate::log::log("error", &format!("input stream error: {}", e));

  let stream = match sample_format {
    SampleFormat::F32 => build_input_f32(
      start_instant,
      &device,
      &config,
      channels,
      sample_rate,
      tx.clone(),
      tx_utt.clone(),
      vad_thresh,
      end_silence_ms,
      min_utt_ms,
      hangover_ms,
      playback_active.clone(),
      gate_until_ms.clone(),
      stop_play_tx.clone(),
      interrupt_counter.clone(),
      accum.clone(),
      utt_buf.clone(),
      in_speech.clone(),
      last_voice_ms.clone(),
      stop_sent.clone(),
      stop_all_rx.clone(),
      peak.clone(),
      ui,
      volume.clone(),
      recording_paused.clone(),
      err_fn,
    )?,
    SampleFormat::I16 => build_input_i16(
      start_instant,
      &device,
      &config,
      channels,
      sample_rate,
      tx.clone(),
      tx_utt.clone(),
      vad_thresh,
      end_silence_ms,
      min_utt_ms,
      hangover_ms,
      playback_active.clone(),
      gate_until_ms.clone(),
      stop_play_tx.clone(),
      interrupt_counter.clone(),
      accum.clone(),
      utt_buf.clone(),
      in_speech.clone(),
      last_voice_ms.clone(),
      stop_sent.clone(),
      stop_all_rx.clone(),
      peak.clone(),
      ui,
      volume.clone(),
      recording_paused.clone(),
      err_fn,
    )?,
    SampleFormat::U16 => build_input_u16(
      start_instant,
      &device,
      &config,
      channels,
      sample_rate,
      tx.clone(),
      tx_utt.clone(),
      vad_thresh,
      end_silence_ms,
      min_utt_ms,
      hangover_ms,
      playback_active.clone(),
      gate_until_ms.clone(),
      stop_play_tx.clone(),
      interrupt_counter.clone(),
      accum.clone(),
      utt_buf.clone(),
      in_speech.clone(),
      last_voice_ms.clone(),
      stop_sent.clone(),
      stop_all_rx.clone(),
      peak.clone(),
      ui,
      volume.clone(),
      recording_paused.clone(),
      err_fn,
    )?,
    other => return Err(format!("unsupported input format: {other:?}").into()),
  };

  stream.play()?;

  while stop_all_rx.try_recv().is_err() {
    thread::sleep(Duration::from_millis(50));
  }

  drop(stream);
  Ok(())
}

// PRIVATE
// ------------------------------------------------------------------

fn build_input_f32(
  start_instant: &'static OnceLock<Instant>,
  device: &cpal::Device,
  config: &cpal::StreamConfig,
  channels: u16,
  sample_rate: u32,
  tx: Sender<crate::audio::AudioChunk>,
  tx_utt: Sender<crate::audio::AudioChunk>,
  vad_thresh: f32,
  end_silence_ms: u64,
  min_utt_ms: u64,
  hangover_ms: u64,
  playback_active: Arc<AtomicBool>,
  gate_until_ms: Arc<AtomicU64>,
  stop_play_tx: Sender<()>,
  interrupt_counter: Arc<AtomicU64>,
  accum: Arc<Mutex<Vec<f32>>>,
  utt_buf: Arc<Mutex<Vec<f32>>>,
  in_speech: Arc<AtomicBool>,
  last_voice_ms: Arc<AtomicU64>,
  stop_sent: Arc<AtomicBool>,
  stop_all_rx: Receiver<()>,
  peak: Arc<Mutex<f32>>,
  ui: crate::state::UiState,
  volume: Arc<Mutex<f32>>,
  recording_paused: Arc<AtomicBool>,
  mut err_fn: impl FnMut(cpal::StreamError) + Send + 'static,
) -> Result<cpal::Stream, cpal::BuildStreamError> {
  device.build_input_stream(
    config,
    move |data: &[f32], _| {
      if recording_paused.load(Ordering::Relaxed) {
        return;
      }
      let local_peak = peak_abs(data);

      if let Ok(mut p) = peak.lock() {
        *p = local_peak;
      }
      if stop_all_rx.try_recv().is_ok() {
        return;
      }

      // use previously computed peak for threshold check
      if local_peak >= vad_thresh {
        last_voice_ms.store(crate::util::now_ms(start_instant), Ordering::Relaxed);
        ui.speaking.store(true, Ordering::Relaxed);

        if !in_speech.swap(true, Ordering::Relaxed) {
          let mut b = utt_buf.lock().unwrap();
          b.clear();
          crate::log::log("info", &format!("Audio detected (peak: {:.3})", local_peak));
        }
        {
          let mut b = utt_buf.lock().unwrap();
          b.extend_from_slice(data);
        }

        if playback_active.load(Ordering::Relaxed) && !stop_sent.load(Ordering::Relaxed) {
          let _ = stop_play_tx.try_send(());
          // Signal conversation + TTS cancellation (user spoke over playback)
          interrupt_counter.fetch_add(1, Ordering::SeqCst);
          stop_sent.store(true, Ordering::Relaxed);
          gate_until_ms.store(
            crate::util::now_ms(start_instant).saturating_add(hangover_ms),
            Ordering::Relaxed,
          );
          // silence audio
          let mut vol = volume.lock().unwrap();
          *vol = 0.0;
          playback_active.store(false, Ordering::Relaxed);
          stop_sent.store(false, Ordering::Relaxed);
        }
      } else if in_speech.load(Ordering::Relaxed) {
        {
          let mut b = utt_buf.lock().unwrap();
          b.extend_from_slice(data);
        }
        let last = last_voice_ms.load(Ordering::Relaxed);
        if last > 0 && crate::util::now_ms(start_instant).saturating_sub(last) >= end_silence_ms {
          crate::log::log("info", "Silence detected");
          ui.speaking.store(false, Ordering::Relaxed);
          in_speech.store(false, Ordering::Relaxed);
          stop_sent.store(false, Ordering::Relaxed);
          let mut b = utt_buf.lock().unwrap();
          if !b.is_empty() {
            let audio = std::mem::take(&mut *b);
            let denom = (sample_rate as u64).saturating_mul(channels as u64).max(1);
            let dur_ms = (audio.len() as u64).saturating_mul(1000) / denom;
            crate::log::log(
              "info",
              &format!(
                "Speech ended after (~{}ms) of silence; samples={})",
                dur_ms,
                audio.len()
              ),
            );
            if dur_ms >= min_utt_ms {
              let _ = tx_utt.send(crate::audio::AudioChunk {
                data: audio,
                channels,
                sample_rate,
              });
            } else {
              crate::log::log(
                "warning",
                &format!(
                  "[{}ms] utterance too short ({}ms < {}ms), dropped",
                  crate::util::now_ms(start_instant),
                  dur_ms,
                  min_utt_ms
                ),
              );
            }
          }
        }
      } else {
        stop_sent.store(false, Ordering::Relaxed);
      }

      let gate_active = playback_active.load(Ordering::Relaxed)
        || crate::util::now_ms(start_instant) < gate_until_ms.load(Ordering::Relaxed);

      if gate_active {
        let zeros = vec![0.0f32; data.len()];
        chunk_and_send(&zeros, channels, sample_rate, &tx, &accum);
      } else {
        chunk_and_send(data, channels, sample_rate, &tx, &accum);
      }
    },
    move |e| err_fn(e),
    None,
  )
}

fn build_input_i16(
  start_instant: &'static OnceLock<Instant>,
  device: &cpal::Device,
  config: &cpal::StreamConfig,
  channels: u16,
  sample_rate: u32,
  tx: Sender<crate::audio::AudioChunk>,
  tx_utt: Sender<crate::audio::AudioChunk>,
  vad_thresh: f32,
  end_silence_ms: u64,
  min_utt_ms: u64,
  hangover_ms: u64,
  playback_active: Arc<AtomicBool>,
  gate_until_ms: Arc<AtomicU64>,
  stop_play_tx: Sender<()>,
  interrupt_counter: Arc<AtomicU64>,
  accum: Arc<Mutex<Vec<f32>>>,
  utt_buf: Arc<Mutex<Vec<f32>>>,
  in_speech: Arc<AtomicBool>,
  last_voice_ms: Arc<AtomicU64>,
  stop_sent: Arc<AtomicBool>,
  stop_all_rx: Receiver<()>,
  peak: Arc<Mutex<f32>>,
  ui: crate::state::UiState,
  volume: Arc<Mutex<f32>>,
  recording_paused: Arc<AtomicBool>,
  mut err_fn: impl FnMut(cpal::StreamError) + Send + 'static,
) -> Result<cpal::Stream, cpal::BuildStreamError> {
  device.build_input_stream(
    config,
    move |data: &[i16], _| {
      if stop_all_rx.try_recv().is_ok() {
        return;
      }
      if recording_paused.load(Ordering::Relaxed) {
        return;
      }

      let mut tmp = Vec::with_capacity(data.len());
      for &s in data {
        tmp.push(s as f32 / i16::MAX as f32);
      }

      let local_peak = peak_abs(&tmp);
      if let Ok(mut p) = peak.lock() {
        *p = local_peak;
      }

      if local_peak >= vad_thresh {
        last_voice_ms.store(crate::util::now_ms(start_instant), Ordering::Relaxed);
        ui.speaking.store(true, Ordering::Relaxed);

        if !in_speech.swap(true, Ordering::Relaxed) {
          let mut b = utt_buf.lock().unwrap();
          b.clear();
          crate::log::log("info", &format!("Audio detected (peak: {:.3})", local_peak));
        }
        {
          let mut b = utt_buf.lock().unwrap();
          b.extend_from_slice(&tmp);
        }

        if playback_active.load(Ordering::Relaxed) && !stop_sent.load(Ordering::Relaxed) {
          let _ = stop_play_tx.try_send(());
          interrupt_counter.fetch_add(1, Ordering::SeqCst);
          stop_sent.store(true, Ordering::Relaxed);
          gate_until_ms.store(
            crate::util::now_ms(start_instant).saturating_add(hangover_ms),
            Ordering::Relaxed,
          );
          // silence audio
          let mut vol = volume.lock().unwrap();
          *vol = 0.0;
          playback_active.store(false, Ordering::Relaxed);
          stop_sent.store(false, Ordering::Relaxed);
        }
      } else if in_speech.load(Ordering::Relaxed) {
        {
          let mut b = utt_buf.lock().unwrap();
          b.extend_from_slice(&tmp);
        }
        let last = last_voice_ms.load(Ordering::Relaxed);
        if last > 0 && crate::util::now_ms(start_instant).saturating_sub(last) >= end_silence_ms {
          crate::log::log("info", "Silence detected");
          ui.speaking.store(false, Ordering::Relaxed);
          in_speech.store(false, Ordering::Relaxed);
          stop_sent.store(false, Ordering::Relaxed);
          let mut b = utt_buf.lock().unwrap();
          if !b.is_empty() {
            let audio = std::mem::take(&mut *b);
            let denom = (sample_rate as u64).saturating_mul(channels as u64).max(1);
            let dur_ms = (audio.len() as u64).saturating_mul(1000) / denom;
            crate::log::log(
              "info",
              &format!(
                "Speech ended after (~{}ms) of silence; samples={})",
                dur_ms,
                audio.len()
              ),
            );
            if dur_ms >= min_utt_ms {
              let _ = tx_utt.send(crate::audio::AudioChunk {
                data: audio,
                channels,
                sample_rate,
              });
            }
          }
        }
      } else {
        stop_sent.store(false, Ordering::Relaxed);
      }

      let gate_active = playback_active.load(Ordering::Relaxed)
        || crate::util::now_ms(start_instant) < gate_until_ms.load(Ordering::Relaxed);

      if gate_active {
        let zeros = vec![0.0f32; tmp.len()];
        chunk_and_send(&zeros, channels, sample_rate, &tx, &accum);
      } else {
        chunk_and_send(&tmp, channels, sample_rate, &tx, &accum);
      }
    },
    move |e| err_fn(e),
    None,
  )
}

fn build_input_u16(
  start_instant: &'static OnceLock<Instant>,
  device: &cpal::Device,
  config: &cpal::StreamConfig,
  channels: u16,
  sample_rate: u32,
  tx: Sender<crate::audio::AudioChunk>,
  tx_utt: Sender<crate::audio::AudioChunk>,
  vad_thresh: f32,
  end_silence_ms: u64,
  min_utt_ms: u64,
  hangover_ms: u64,
  playback_active: Arc<AtomicBool>,
  gate_until_ms: Arc<AtomicU64>,
  stop_play_tx: Sender<()>,
  interrupt_counter: Arc<AtomicU64>,
  accum: Arc<Mutex<Vec<f32>>>,
  utt_buf: Arc<Mutex<Vec<f32>>>,
  in_speech: Arc<AtomicBool>,
  last_voice_ms: Arc<AtomicU64>,
  stop_sent: Arc<AtomicBool>,
  stop_all_rx: Receiver<()>,
  peak: Arc<Mutex<f32>>,
  ui: crate::state::UiState,
  volume: Arc<Mutex<f32>>,
  recording_paused: Arc<AtomicBool>,
  mut err_fn: impl FnMut(cpal::StreamError) + Send + 'static,
) -> Result<cpal::Stream, cpal::BuildStreamError> {
  device.build_input_stream(
    config,
    move |data: &[u16], _| {
      if recording_paused.load(Ordering::Relaxed) {
        return;
      }
      let local_peak = peak_abs(
        &data
          .iter()
          .map(|&s| (s as f32 / u16::MAX as f32) * 2.0 - 1.0)
          .collect::<Vec<f32>>(),
      );
      if let Ok(mut p) = peak.lock() {
        *p = local_peak;
      }

      if stop_all_rx.try_recv().is_ok() {
        return;
      }

      let tmp = data
        .iter()
        .map(|&s| (s as f32 / u16::MAX as f32) * 2.0 - 1.0)
        .collect::<Vec<f32>>();

      if local_peak >= vad_thresh {
        last_voice_ms.store(crate::util::now_ms(start_instant), Ordering::Relaxed);
        ui.speaking.store(true, Ordering::Relaxed);
        last_voice_ms.store(crate::util::now_ms(start_instant), Ordering::Relaxed);
        ui.speaking.store(true, Ordering::Relaxed);

        if !in_speech.swap(true, Ordering::Relaxed) {
          let mut b = utt_buf.lock().unwrap();
          b.clear();
          crate::log::log("info", &format!("Audio detected (peak: {:.3})", local_peak));
        }
        {
          let mut b = utt_buf.lock().unwrap();
          b.extend_from_slice(&tmp);
        }

        if playback_active.load(Ordering::Relaxed) && !stop_sent.load(Ordering::Relaxed) {
          let _ = stop_play_tx.try_send(());
          interrupt_counter.fetch_add(1, Ordering::SeqCst);
          stop_sent.store(true, Ordering::Relaxed);
          gate_until_ms.store(
            crate::util::now_ms(start_instant).saturating_add(hangover_ms),
            Ordering::Relaxed,
          );
          // silence audio
          let mut vol = volume.lock().unwrap();
          *vol = 0.0;
          playback_active.store(false, Ordering::Relaxed);
          stop_sent.store(false, Ordering::Relaxed);
        }
      } else if in_speech.load(Ordering::Relaxed) {
        {
          let mut b = utt_buf.lock().unwrap();
          b.extend_from_slice(&tmp);
        }
        let last = last_voice_ms.load(Ordering::Relaxed);
        if last > 0 && crate::util::now_ms(start_instant).saturating_sub(last) >= end_silence_ms {
          crate::log::log("info", "Silence detected");
          in_speech.store(false, Ordering::Relaxed);
          stop_sent.store(false, Ordering::Relaxed);

          let mut b = utt_buf.lock().unwrap();
          if !b.is_empty() {
            let audio = std::mem::take(&mut *b);
            let denom = (sample_rate as u64).saturating_mul(channels as u64).max(1);
            let dur_ms = (audio.len() as u64).saturating_mul(1000) / denom;
            crate::log::log(
              "info",
              &format!(
                "Speech ended after (~{}ms) of silence; samples={})",
                dur_ms,
                audio.len()
              ),
            );
            if dur_ms >= min_utt_ms {
              let _ = tx_utt.send(crate::audio::AudioChunk {
                data: audio,
                channels,
                sample_rate,
              });
            }
          }
        }
      } else {
        stop_sent.store(false, Ordering::Relaxed);
      }

      let gate_active = playback_active.load(Ordering::Relaxed)
        || crate::util::now_ms(start_instant) < gate_until_ms.load(Ordering::Relaxed);

      if gate_active {
        let zeros = vec![0.0f32; tmp.len()];
        chunk_and_send(&zeros, channels, sample_rate, &tx, &accum);
      } else {
        chunk_and_send(&tmp, channels, sample_rate, &tx, &accum);
      }
    },
    move |e| err_fn(e),
    None,
  )
}

fn chunk_and_send(
  data: &[f32],
  channels: u16,
  sample_rate: u32,
  tx: &Sender<crate::audio::AudioChunk>,
  accum: &Arc<Mutex<Vec<f32>>>,
) {
  let mut acc = accum.lock().unwrap();
  acc.extend_from_slice(data);

  let chunk_len = crate::tts::CHUNK_FRAMES * channels as usize;
  while acc.len() >= chunk_len {
    let chunk_data: Vec<f32> = acc.drain(..chunk_len).collect();
    let _ = tx.try_send(crate::audio::AudioChunk {
      data: chunk_data,
      channels,
      sample_rate,
    });
  }
}

fn peak_abs(x: &[f32]) -> f32 {
  let mut m = 0.0f32;
  for &v in x {
    let a = v.abs();
    if a > m {
      m = a;
    }
  }
  m
}
