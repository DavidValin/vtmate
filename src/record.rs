// ------------------------------------------------------------------
//  Record
// ------------------------------------------------------------------

use crate::START_INSTANT;
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

  // utterance capture state
  let utt_buf: Arc<Mutex<Vec<f32>>> = Arc::new(Mutex::new(Vec::new()));
  let user_speaking = Arc::new(AtomicBool::new(false));
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
      tx_utt.clone(),
      vad_thresh,
      end_silence_ms,
      min_utt_ms,
      hangover_ms,
      playback_active.clone(),
      gate_until_ms.clone(),
      stop_play_tx.clone(),
      interrupt_counter.clone(),
      utt_buf.clone(),
      user_speaking.clone(),
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
      tx_utt.clone(),
      vad_thresh,
      end_silence_ms,
      min_utt_ms,
      hangover_ms,
      playback_active.clone(),
      gate_until_ms.clone(),
      stop_play_tx.clone(),
      interrupt_counter.clone(),
      utt_buf.clone(),
      user_speaking.clone(),
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
      tx_utt.clone(),
      vad_thresh,
      end_silence_ms,
      min_utt_ms,
      hangover_ms,
      playback_active.clone(),
      gate_until_ms.clone(),
      stop_play_tx.clone(),
      interrupt_counter.clone(),
      utt_buf.clone(),
      user_speaking.clone(),
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
  tx_utt: Sender<crate::audio::AudioChunk>,
  vad_thresh: f32,
  end_silence_ms: u64,
  min_utt_ms: u64,
  hangover_ms: u64,
  playback_active: Arc<AtomicBool>,
  gate_until_ms: Arc<AtomicU64>,
  stop_play_tx: Sender<()>,
  interrupt_counter: Arc<AtomicU64>,
  utt_buf: Arc<Mutex<Vec<f32>>>,
  user_speaking: Arc<AtomicBool>,
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
        ui.agent_speaking.store(true, Ordering::Relaxed);

        if !user_speaking.swap(true, Ordering::Relaxed) {
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
      } else if user_speaking.load(Ordering::Relaxed) {
        {
          let mut b = utt_buf.lock().unwrap();
          b.extend_from_slice(data);
        }
        let last = last_voice_ms.load(Ordering::Relaxed);

        // silence detected
        if last > 0 && crate::util::now_ms(start_instant).saturating_sub(last) >= end_silence_ms {
          crate::log::log("info", "Silence detected");
          ui.agent_speaking.store(false, Ordering::Relaxed);
          user_speaking.store(false, Ordering::Relaxed);
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
            // new utterance
            if dur_ms >= min_utt_ms {
              crate::util::SPEECH_END_AT.store(
                crate::util::now_ms(&START_INSTANT),
                std::sync::atomic::Ordering::SeqCst,
              );
              // commit utterance audio
              let _ = tx_utt.send(crate::audio::AudioChunk {
                data: audio,
                channels,
                sample_rate,
              });
            } else {
              crate::log::log(
                "info",
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
  tx_utt: Sender<crate::audio::AudioChunk>,
  vad_thresh: f32,
  end_silence_ms: u64,
  min_utt_ms: u64,
  hangover_ms: u64,
  playback_active: Arc<AtomicBool>,
  gate_until_ms: Arc<AtomicU64>,
  stop_play_tx: Sender<()>,
  interrupt_counter: Arc<AtomicU64>,
  utt_buf: Arc<Mutex<Vec<f32>>>,
  user_speaking: Arc<AtomicBool>,
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

      // Convert to f32 interleaved (preserve existing behavior)
      let mut tmp = Vec::with_capacity(data.len());
      for &s in data {
        tmp.push((s as f32) / 32768.0);
      }

      let local_peak = peak_abs(&tmp);
      if let Ok(mut p) = peak.lock() {
        *p = local_peak;
      }

      if local_peak >= vad_thresh {
        last_voice_ms.store(crate::util::now_ms(start_instant), Ordering::Relaxed);
        ui.agent_speaking.store(true, Ordering::Relaxed);

        if !user_speaking.swap(true, Ordering::Relaxed) {
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
      } else if user_speaking.load(Ordering::Relaxed) {
        {
          let mut b = utt_buf.lock().unwrap();
          b.extend_from_slice(&tmp);
        }
        let last = last_voice_ms.load(Ordering::Relaxed);
        if last > 0 && crate::util::now_ms(start_instant).saturating_sub(last) >= end_silence_ms {
          crate::log::log("info", "Silence detected");
          ui.agent_speaking.store(false, Ordering::Relaxed);
          user_speaking.store(false, Ordering::Relaxed);
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
              crate::util::SPEECH_END_AT.store(
                crate::util::now_ms(&START_INSTANT),
                std::sync::atomic::Ordering::SeqCst,
              );
              let _ = tx_utt.send(crate::audio::AudioChunk {
                data: audio,
                channels,
                sample_rate,
              });
            } else {
              // FIX: match f32 behavior (warn + drop)
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
  tx_utt: Sender<crate::audio::AudioChunk>,
  vad_thresh: f32,
  end_silence_ms: u64,
  min_utt_ms: u64,
  hangover_ms: u64,
  playback_active: Arc<AtomicBool>,
  gate_until_ms: Arc<AtomicU64>,
  stop_play_tx: Sender<()>,
  interrupt_counter: Arc<AtomicU64>,
  utt_buf: Arc<Mutex<Vec<f32>>>,
  user_speaking: Arc<AtomicBool>,
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

      if stop_all_rx.try_recv().is_ok() {
        return;
      }

      // Convert once (preserve existing behavior), and reuse for peak + utt_buf + resample
      let mut tmp = Vec::with_capacity(data.len());
      for &s in data {
        tmp.push((s as f32 / u16::MAX as f32) * 2.0 - 1.0);
      }

      let local_peak = peak_abs(&tmp);
      if let Ok(mut p) = peak.lock() {
        *p = local_peak;
      }

      if local_peak >= vad_thresh {
        // FIX: remove duplicate stores
        last_voice_ms.store(crate::util::now_ms(start_instant), Ordering::Relaxed);
        ui.agent_speaking.store(true, Ordering::Relaxed);

        if !user_speaking.swap(true, Ordering::Relaxed) {
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
      } else if user_speaking.load(Ordering::Relaxed) {
        {
          let mut b = utt_buf.lock().unwrap();
          b.extend_from_slice(&tmp);
        }
        let last = last_voice_ms.load(Ordering::Relaxed);
        if last > 0 && crate::util::now_ms(start_instant).saturating_sub(last) >= end_silence_ms {
          crate::log::log("info", "Silence detected");
          // FIX: ensure UI clears speaking state on silence
          ui.agent_speaking.store(false, Ordering::Relaxed);

          user_speaking.store(false, Ordering::Relaxed);
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
              crate::util::SPEECH_END_AT.store(
                crate::util::now_ms(&START_INSTANT),
                std::sync::atomic::Ordering::SeqCst,
              );
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
    },
    move |e| err_fn(e),
    None,
  )
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
