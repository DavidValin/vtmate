// ------------------------------------------------------------------
//  Playback
// ------------------------------------------------------------------

use cpal::traits::{DeviceTrait, StreamTrait};
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
// ------------------------------------------------------------------

pub fn playback_thread(
  start_instant: OnceLock<Instant>,
  device: cpal::Device,
  supported: cpal::SupportedStreamConfig,
  config: cpal::StreamConfig,
  rx_audio: Receiver<crate::audio::AudioChunk>,
  rx_stop: Receiver<()>,
  stop_all_rx: Receiver<()>,
  playback_active: Arc<AtomicBool>,
  gate_until_ms: Arc<AtomicU64>,
  paused: Arc<AtomicBool>,
  out_channels: u16,
  ui: crate::ui::UiState,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
  let start_instant_ref = start_instant.clone();

  // start_instant_ref removed
  use cpal::SampleFormat;

  let queue: Arc<Mutex<VecDeque<f32>>> = Arc::new(Mutex::new(VecDeque::new()));
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
              *s = v;
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
                crate::util::now_ms(&start_instant).saturating_add(hangover_ms),
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
              *s = (v * i16::MAX as f32) as i16;
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
                crate::util::now_ms(&start_instant).saturating_add(hangover_ms),
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
              *s = (norm * u16::MAX as f32) as u16;
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
                crate::util::now_ms(&start_instant).saturating_add(hangover_ms),
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

  stream.play()?;

  playback_active.store(false, Ordering::Relaxed);
  ui.playing.store(false, Ordering::Relaxed);

  loop {
    select! {
      recv(stop_all_rx) -> _ => {
        queue.lock().unwrap().clear();
        playback_active.store(false, Ordering::Relaxed);
        ui.playing.store(false, Ordering::Relaxed);
        break;
      }
      recv(rx_stop) -> _ => {
        queue.lock().unwrap().clear();
        playback_active.store(false, Ordering::Relaxed);
        ui.playing.store(false, Ordering::Relaxed);
        empty_callbacks.store(0, Ordering::Relaxed);
        gate_until_ms.store(crate::util::now_ms(&start_instant_ref).saturating_add(hangover_ms), Ordering::Relaxed);

        // IMPORTANT: also drain any already-enqueued audio chunks.
        // Without this, multi-phrase TTS may have queued extra chunks
        // in the crossbeam channel; they would get played even after
        // we clear the output queue, and can race with interruption UI.
        while rx_audio.try_recv().is_ok() {}
      }
      recv(rx_audio) -> msg => {
        let Ok(chunk) = msg else { break };

        // Sanity: must match playback SR
        let channels = out_channels as usize;
        let max_samples = crate::tts::QUEUE_CAP_FRAMES * channels;

        // Backpressure: wait until there's room
        loop {
          {
            let q = queue.lock().unwrap();
            if q.len() + chunk.data.len() <= max_samples {
              break;
            }
          }
          thread::sleep(Duration::from_millis(5));
        }
        {
          let mut q = queue.lock().unwrap();
          // resample if needed
          if chunk.sample_rate != config.sample_rate.0 {
            let resampled = crate::audio::resample_to(&chunk.data, chunk.channels, chunk.sample_rate, config.sample_rate.0);
            for s in resampled {
                q.push_back(s);
            }
          } else {
            for s in chunk.data {
              q.push_back(s);
            }
          }
        }

        empty_callbacks.store(0, Ordering::Relaxed);
        playback_active.store(true, Ordering::Relaxed);
        ui.playing.store(true, Ordering::Relaxed);
      }
    }
  }

  drop(stream);
  Ok(())
}
