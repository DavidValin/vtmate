// ------------------------------------------------------------------
//  Playback
// ------------------------------------------------------------------

use crate::state::GLOBAL_STATE;
use cpal::traits::{DeviceTrait, StreamTrait};
use crossbeam_channel::Sender;
use crossbeam_channel::{Receiver, select};
use std::collections::VecDeque;
use std::sync::OnceLock;
use std::sync::{
  Arc, Mutex,
  atomic::{AtomicBool, AtomicU64, Ordering},
};
use std::thread;
use std::time::Duration;
use std::time::Instant;

// API

static WAV_TX: OnceLock<Sender<crate::audio::AudioChunk>> = OnceLock::new();

/// Set the global channel used by the WAV writer thread.
pub fn set_wav_tx(tx: Sender<crate::audio::AudioChunk>) {
  WAV_TX.set(tx).ok();
}
// ------------------------------------------------------------------

pub fn playback_thread(
  start_instant: &'static OnceLock<Instant>,
  device: cpal::Device,
  supported: cpal::SupportedStreamConfig,
  config: cpal::StreamConfig,
  rx_audio: Receiver<crate::audio::AudioChunk>,
  stop_play_rx: Receiver<()>,
  playback_active: Arc<AtomicBool>,
  gate_until_ms: Arc<AtomicU64>,
  paused: Arc<AtomicBool>,
  out_channels: u16,
  ui: crate::state::UiState,
  volume: Arc<Mutex<f32>>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
  // inst removed
  // let inst_ptr = &start_instant;
  use cpal::SampleFormat;

  let queue: Arc<Mutex<VecDeque<f32>>> = Arc::new(Mutex::new(VecDeque::new()));
  let volume_for_stream = volume.clone();
  let sample_format = supported.sample_format();
  let hangover_ms = crate::util::env_u64("HANGOVER_MS", crate::config::HANGOVER_MS_DEFAULT);

  // When this reaches a few callbacks in a row of "no real audio", we mark not-playing.
  let empty_callbacks = Arc::new(AtomicU64::new(0));

  let err_fn = |e| crate::log::log("error", &format!("output stream error: {}", e));

  let stream = match sample_format {
    SampleFormat::F32 => device.build_output_stream(
      &config,
      {
        let queue = queue.clone();
        let playback_active = playback_active.clone();
        let gate_until_ms = gate_until_ms.clone();
        let paused = paused.clone();
        let ui = ui.clone();
        let empty_callbacks = empty_callbacks.clone();
        move |out: &mut [f32], _| {
          let vol = *volume_for_stream.lock().unwrap();
          if vol == 0.0 {
            // Restore volume to default before returning
            *volume_for_stream.lock().unwrap() = 1.0;
            queue.lock().unwrap().clear();
            playback_active.store(false, Ordering::Relaxed);
            ui.playing.store(false, Ordering::Relaxed);
            gate_until_ms.store(
              crate::util::now_ms(start_instant).saturating_add(hangover_ms),
              Ordering::Relaxed,
            );
            return;
          }
          let mut q = queue.lock().unwrap();

          // Spacebar pause: output silence but do NOT consume queued samples.
          if paused.load(Ordering::Relaxed) {
            for s in out.iter_mut() {
              *s = 0.0;
            }
            // Keep "playing" state if we still have audio queued.
            if !q.is_empty() {
              playback_active.store(true, Ordering::Relaxed);
              ui.playing.store(true, Ordering::Relaxed);
              empty_callbacks.store(0, Ordering::Relaxed);
            }
            return;
          }

          let mut any_real = false;
          for s in out.iter_mut() {
            if let Some(v) = q.pop_front() {
              *s = v.clamp(-1.0, 1.0) * vol;
              any_real = true;
            } else {
              *s = 0.0;
            }
          }
          if any_real {
            empty_callbacks.store(0, Ordering::Relaxed);
          } else {
            let n = empty_callbacks.fetch_add(1, Ordering::Relaxed) + 1;
            if n >= 1 {
              playback_active.store(false, Ordering::Relaxed);
              ui.playing.store(false, Ordering::Relaxed);
              gate_until_ms.store(
                crate::util::now_ms(start_instant).saturating_add(hangover_ms),
                Ordering::Relaxed,
              );
            }
          }
        }
      },
      err_fn,
      None,
    )?,
    SampleFormat::I16 => device.build_output_stream(
      &config,
      {
        let queue = queue.clone();
        let playback_active = playback_active.clone();
        let gate_until_ms = gate_until_ms.clone();
        let paused = paused.clone();
        let ui = ui.clone();
        let empty_callbacks = empty_callbacks.clone();
        move |out: &mut [i16], _| {
          let vol = *volume_for_stream.lock().unwrap();
          if vol == 0.0 {
            queue.lock().unwrap().clear();
            playback_active.store(false, Ordering::Relaxed);
            ui.playing.store(false, Ordering::Relaxed);
            gate_until_ms.store(
              crate::util::now_ms(start_instant).saturating_add(hangover_ms),
              Ordering::Relaxed,
            );

            // ✅ FIX: silence
            for s in out.iter_mut() {
              *s = 0;
            }
            return;
          }
          let mut q = queue.lock().unwrap();

          if paused.load(Ordering::Relaxed) {
            for s in out.iter_mut() {
              *s = 0;
            }
            if !q.is_empty() {
              playback_active.store(true, Ordering::Relaxed);
              ui.playing.store(true, Ordering::Relaxed);
              empty_callbacks.store(0, Ordering::Relaxed);
            }
            return;
          }

          let mut any_real = false;
          for s in out.iter_mut() {
            if let Some(v) = q.pop_front() {
              any_real = true;
              let v = v.clamp(-1.0, 1.0);
              *s = ((v * vol).clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
            } else {
              *s = 0;
            }
          }

          if any_real {
            empty_callbacks.store(0, Ordering::Relaxed);
          } else {
            let n = empty_callbacks.fetch_add(1, Ordering::Relaxed) + 1;
            if n >= 1 {
              playback_active.store(false, Ordering::Relaxed);
              ui.playing.store(false, Ordering::Relaxed);
              gate_until_ms.store(
                crate::util::now_ms(start_instant).saturating_add(hangover_ms),
                Ordering::Relaxed,
              );
            }
          }
        }
      },
      err_fn,
      None,
    )?,
    SampleFormat::U16 => device.build_output_stream(
      &config,
      {
        let queue = queue.clone();
        let playback_active = playback_active.clone();
        let gate_until_ms = gate_until_ms.clone();
        let paused = paused.clone();
        let ui = ui.clone();
        let empty_callbacks = empty_callbacks.clone();
        move |out: &mut [u16], _| {
          let vol = *volume_for_stream.lock().unwrap();
          if vol == 0.0 {
            queue.lock().unwrap().clear();
            playback_active.store(false, Ordering::Relaxed);
            ui.playing.store(false, Ordering::Relaxed);
            gate_until_ms.store(
              crate::util::now_ms(start_instant).saturating_add(hangover_ms),
              Ordering::Relaxed,
            );

            // ✅ FIX: silence for unsigned (midpoint)
            for s in out.iter_mut() {
              *s = u16::MAX / 2;
            }
            return;
          }
          let mut q = queue.lock().unwrap();

          if paused.load(Ordering::Relaxed) {
            for s in out.iter_mut() {
              *s = u16::MAX / 2;
            }
            if !q.is_empty() {
              playback_active.store(true, Ordering::Relaxed);
              ui.playing.store(true, Ordering::Relaxed);
              empty_callbacks.store(0, Ordering::Relaxed);
            }
            return;
          }

          let mut any_real = false;
          for s in out.iter_mut() {
            if let Some(v) = q.pop_front() {
              any_real = true;
              let v = v.clamp(-1.0, 1.0);
              let norm = (v + 1.0) * 0.5;
              *s = ((norm * vol).clamp(-1.0, 1.0) * u16::MAX as f32) as u16;
            } else {
              *s = u16::MAX / 2;
            }
          }

          if any_real {
            empty_callbacks.store(0, Ordering::Relaxed);
          } else {
            let n = empty_callbacks.fetch_add(1, Ordering::Relaxed) + 1;
            if n >= 1 {
              playback_active.store(false, Ordering::Relaxed);
              ui.playing.store(false, Ordering::Relaxed);
              gate_until_ms.store(
                crate::util::now_ms(start_instant).saturating_add(hangover_ms),
                Ordering::Relaxed,
              );
            }
          }
        }
      },
      err_fn,
      None,
    )?,
    other => return Err(format!("unsupported output format: {other:?}").into()),
  };

  loop {
    stream.play()?;
    // Reset state before each stream
    *volume.lock().unwrap() = 1.0;
    queue.lock().unwrap().clear();
    empty_callbacks.store(0, Ordering::Relaxed);
    playback_active.store(false, Ordering::Relaxed);
    ui.playing.store(false, Ordering::Relaxed);
    loop {
      select! {
        recv(stop_play_rx) -> _ => {
          // Drain any pending audio chunks from rx_audio
          while let Ok(_) = rx_audio.try_recv() {}
          // Clear queue immediately before stopping
          queue.lock().unwrap().clear();
          // Stop current stream immediately by dropping it; let outer loop recreate
          break;
        }
        recv(rx_audio) -> msg => {
          let Ok(chunk) = msg else { break };
          // Forward to wav writer if set
          if let Some(tx) = WAV_TX.get() {
            // Determine data that will actually be played
            let mut out_data = if chunk.channels != out_channels {
              convert_channels(&chunk.data, chunk.channels, out_channels)
            } else {
              chunk.data.clone()
            };
            if chunk.sample_rate != config.sample_rate.0 {
              let resampled = crate::audio::resample_to(&out_data, out_channels, chunk.sample_rate, config.sample_rate.0);
              out_data = resampled;
            }
            let writer_chunk = crate::audio::AudioChunk {
              data: out_data,
              channels: out_channels,
              sample_rate: config.sample_rate.0,
            };
            tx.send(writer_chunk).unwrap_or(());
          }
          let channels = out_channels as usize;
          let max_samples = crate::tts::QUEUE_CAP_FRAMES * channels;
          loop {
            let q = queue.lock().unwrap();
            if q.len() + chunk.data.len() <= max_samples {
              break;
            }
            drop(q);
            thread::sleep(Duration::from_millis(5));
          }

          if GLOBAL_STATE.get().unwrap().processing_response.load(Ordering::Relaxed) || *volume.lock().unwrap() == 0.0 {
            let mut vol = volume.lock().unwrap();
            *vol = 1.0;
            GLOBAL_STATE.get().unwrap().processing_response.store(false, Ordering::Relaxed);
          }
          let mut q = queue.lock().unwrap();
          let data = if chunk.channels != out_channels {
            convert_channels(&chunk.data, chunk.channels, out_channels)
          } else {
            chunk.data.clone()
          };
          if chunk.sample_rate != config.sample_rate.0 {
            let resampled = crate::audio::resample_to(&data, out_channels, chunk.sample_rate, config.sample_rate.0);
            for s in resampled { q.push_back(s); }
          } else {
            for s in data { q.push_back(s); }
          }
          empty_callbacks.store(0, Ordering::Relaxed);
          playback_active.store(true, Ordering::Relaxed);
          ui.playing.store(true, Ordering::Relaxed);
        }
      }
    }
  }
}

// PRIVATE
// ------------------------------------------------------------------

fn convert_channels(input: &[f32], in_channels: u16, out_channels: u16) -> Vec<f32> {
  if in_channels == out_channels {
    return input.to_vec();
  }
  let in_ch = in_channels as usize;
  let out_ch = out_channels as usize;
  let frames = input.len() / in_ch;
  let mut out = Vec::with_capacity(frames * out_ch);
  for f in 0..frames {
    let frame = &input[f * in_ch..f * in_ch + in_ch];
    match (in_ch, out_ch) {
      (1, oc) => {
        let v = frame[0];
        for _ in 0..oc {
          out.push(v);
        }
      }
      (ic, 1) => {
        let sum: f32 = frame.iter().copied().sum();
        out.push(sum / ic as f32);
      }
      _ => {
        let n = in_ch.min(out_ch);
        for i in 0..n {
          out.push(frame[i]);
        }
        for _ in n..out_ch {
          out.push(0.0);
        }
      }
    }
  }
  out
}
