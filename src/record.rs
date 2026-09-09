// ------------------------------------------------------------------
//  Record
// ------------------------------------------------------------------

use crate::START_INSTANT;
use cpal::traits::{DeviceTrait, StreamTrait};
use crossbeam_channel::Sender;
use std::sync::OnceLock;
use std::sync::{
  Arc, Mutex,
  atomic::{AtomicBool, AtomicU64, Ordering},
};
use std::time::Instant;

// API
// ------------------------------------------------------------------

pub fn record_thread(
  start_instant: &'static OnceLock<Instant>,
  device: cpal::Device,
  supported: cpal::SupportedStreamConfig,
  config: cpal::StreamConfig,
  tx_utt: Sender<crate::audio::Utterance>, // utterance -> conversation
  tx_ui: Sender<String>,                    // UI channel for interrupt banner
  vad_thresh: f32,
  end_silence_ms: u64,
  playback_active: Arc<AtomicBool>,
  gate_until_ms: Arc<AtomicU64>,
  interrupt_counter: Arc<AtomicU64>,
  peak: Arc<Mutex<f32>>,

  ui: crate::state::UiState,
  volume: Arc<Mutex<f32>>,
  recording_paused: Arc<AtomicBool>,
  utterance_meta: Arc<Mutex<crate::state::UtteranceMeta>>,
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

  // the builder closure owns its copies; the supervisor loop keeps its own
  let paused_flag = recording_paused.clone();
  let peak_meter = peak.clone();
  let build_stream = move || -> Result<cpal::Stream, cpal::BuildStreamError> {
    match sample_format {
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
      interrupt_counter.clone(),
      utt_buf.clone(),
      user_speaking.clone(),
      last_voice_ms.clone(),
      stop_sent.clone(),
      peak.clone(),
      ui.clone(),
      volume.clone(),
      recording_paused.clone(),
      utterance_meta.clone(),
      tx_ui.clone(),
      err_fn,
    ),

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
      interrupt_counter.clone(),
      utt_buf.clone(),
      user_speaking.clone(),
      last_voice_ms.clone(),
      stop_sent.clone(),
      peak.clone(),
      ui.clone(),
      volume.clone(),
      recording_paused.clone(),
      utterance_meta.clone(),
      tx_ui.clone(),
      err_fn,
    ),

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
      interrupt_counter.clone(),
      utt_buf.clone(),
      user_speaking.clone(),
      last_voice_ms.clone(),
      stop_sent.clone(),
      peak.clone(),
      ui.clone(),
      volume.clone(),
      recording_paused.clone(),
      utterance_meta.clone(),
      tx_ui.clone(),
      err_fn,
    ),

      other => panic!("unsupported input format: {other:?}"),
    }
  };

  match sample_format {
    SampleFormat::F32 | SampleFormat::I16 | SampleFormat::U16 => {}
    other => return Err(format!("unsupported input format: {other:?}").into()),
  }

  // The microphone is only open while recording: the device is opened when
  // recording starts and closed shortly after it stops, so vtmate does not
  // show up as using the microphone (nor keep it from other applications)
  // while it sits idle waiting for a shortcut.
  //
  // Closing is delayed by `MIC_RELEASE_AFTER`: the callback flushes the
  // captured utterance on its first run after `recording_paused` goes up, so
  // the device has to outlive the pause by a few callbacks. Back-to-back
  // recordings within that window reuse the open device.
  const MIC_RELEASE_AFTER: std::time::Duration = std::time::Duration::from_millis(400);
  const OPEN_RETRY_AFTER: std::time::Duration = std::time::Duration::from_secs(2);

  // cpal keeps an open ALSA handle inside the `Device` from the moment its
  // configuration is queried, so the microphone would show as in use before
  // any recording. Building a stream takes that handle over and dropping it
  // closes the device, leaving it free until recording really starts.
  match build_stream() {
    Ok(s) => drop(s),
    Err(e) => crate::log::log(
      "debug",
      &format!("could not release the idle microphone handle: {}", e),
    ),
  }

  let mut stream: Option<cpal::Stream> = None;
  let mut paused_at: Option<Instant> = None;
  let mut failed_at: Option<Instant> = None;
  loop {
    let paused = paused_flag.load(Ordering::Relaxed);
    if !paused {
      paused_at = None;
      if stream.is_none() && failed_at.is_none_or(|t| t.elapsed() >= OPEN_RETRY_AFTER) {
        let opened = build_stream()
          .map_err(|e| e.to_string())
          .and_then(|s| s.play().map(|_| s).map_err(|e| e.to_string()));
        match opened {
          Ok(s) => {
            stream = Some(s);
            failed_at = None;
            crate::log::log("debug", "microphone opened");
          }
          Err(e) => {
            failed_at = Some(Instant::now());
            crate::log::log("error", &format!("cannot open the microphone: {}", e));
          }
        }
      }
    } else if stream.is_some() {
      match paused_at {
        None => paused_at = Some(Instant::now()),
        Some(t) if t.elapsed() >= MIC_RELEASE_AFTER => {
          stream = None; // closing the stream releases the device
          paused_at = None;
          if let Ok(mut p) = peak_meter.lock() {
            *p = 0.0;
          }
          crate::log::log("debug", "microphone released");
        }
        Some(_) => {}
      }
    }
    std::thread::sleep(std::time::Duration::from_millis(5));
  }
}

/// How long after interrupting the agent before another interruption can be
/// raised. One burst of speech is one interruption, however long you talk.
const INTERRUPT_COOLDOWN_MS: u64 = 1200;

/// When that hold-off ends. Kept here rather than in `gate_until_ms`, which
/// the playback thread rewrites constantly and would cut it short.
static INTERRUPT_GATE_MS: AtomicU64 = AtomicU64::new(0);

// PRIVATE
// ------------------------------------------------------------------

fn build_input_f32(
  start_instant: &'static OnceLock<Instant>,
  device: &cpal::Device,
  config: &cpal::StreamConfig,
  channels: u16,
  sample_rate: u32,
  tx_utt: Sender<crate::audio::Utterance>,
  vad_thresh: f32,
  end_silence_ms: u64,
  min_utt_ms: u64,
  hangover_ms: u64,
  playback_active: Arc<AtomicBool>,
  gate_until_ms: Arc<AtomicU64>,
  interrupt_counter: Arc<AtomicU64>,
  utt_buf: Arc<Mutex<Vec<f32>>>,
  user_speaking: Arc<AtomicBool>,
  last_voice_ms: Arc<AtomicU64>,
  stop_sent: Arc<AtomicBool>,
  peak: Arc<Mutex<f32>>,
  ui: crate::state::UiState,
  volume: Arc<Mutex<f32>>,
  recording_paused: Arc<AtomicBool>,
  utterance_meta: Arc<Mutex<crate::state::UtteranceMeta>>,
  tx_ui: Sender<String>,
  mut err_fn: impl FnMut(cpal::StreamError) + Send + 'static,
) -> Result<cpal::Stream, cpal::BuildStreamError> {
  device.build_input_stream(
    config,
    move |data: &[f32], _| {
      let local_peak = peak_abs(data);

      if let Ok(mut p) = peak.lock() {
        *p = local_peak;
      }
      if recording_paused.load(Ordering::Relaxed) {
        // flush buffer if not empty
        let mut b = utt_buf.lock().unwrap();
        if !b.is_empty() {
          let audio = std::mem::take(&mut *b);
          let denom = (sample_rate as u64).saturating_mul(channels as u64).max(1);
          let dur_ms = (audio.len() as u64).saturating_mul(1000) / denom;
          if dur_ms >= min_utt_ms {
            crate::util::SPEECH_END_AT.store(
              crate::util::now_ms(&START_INSTANT),
              std::sync::atomic::Ordering::SeqCst,
            );
            let meta = utterance_meta
                .lock()
                .map(|m| m.clone())
                .unwrap_or_default();
            let _ = tx_utt.send(crate::audio::Utterance {
              audio: crate::audio::AudioChunk {
                data: audio,
                channels,
                sample_rate,
              },
              kind: meta.kind,
              attachment: meta.attachment,
              text: None,
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
        return;
      }
      let local_peak = peak_abs(data);

      // read live, not from the value this stream was built with: the
      // settings popup can change them while the microphone is open
      let vad_thresh = crate::state::GLOBAL_STATE
        .get()
        .map(|s| s.vad_threshold())
        .unwrap_or(vad_thresh);

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

        // Interrupt while the agent is audible. `playback_active` cannot be
        // used here: it drops every time the queue empties between chunks, so
        // speaking in one of those gaps (which is most of the start of a
        // phrase) was silently ignored.
        let agent_audible = crate::state::GLOBAL_STATE
          .get()
          .map(|st| st.playback.speaking.load(Ordering::Relaxed))
          .unwrap_or_else(|| playback_active.load(Ordering::Relaxed));
        // One interrupt per burst of speech. Without this hold-off every
        // audio callback raised another one for as long as you kept talking,
        // printing a wall of "USER interrupted".
        let gate_open =
          crate::util::now_ms(start_instant) >= INTERRUPT_GATE_MS.load(Ordering::Relaxed);
        if agent_audible && gate_open {
          // silence audio
          let mut vol = volume.lock().unwrap();
          *vol = 0.0;
          interrupt_counter.fetch_add(1, Ordering::SeqCst);
          let _ = tx_ui.send("user_interrupt_show|".to_string());
          stop_sent.store(true, Ordering::Relaxed);
          INTERRUPT_GATE_MS.store(
            crate::util::now_ms(start_instant).saturating_add(INTERRUPT_COOLDOWN_MS),
            Ordering::Relaxed,
          );
          gate_until_ms.store(
            crate::util::now_ms(start_instant).saturating_add(hangover_ms),
            Ordering::Relaxed,
          );
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
        if last > 0
          && !crate::state::GLOBAL_STATE
            .get()
            .unwrap()
            .ptt
            .load(Ordering::Relaxed)
          && crate::util::now_ms(start_instant).saturating_sub(last)
            >= crate::state::GLOBAL_STATE
              .get()
              .map(|s| s.end_silence_ms.load(Ordering::Relaxed))
              .unwrap_or(end_silence_ms)
        {
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
              let meta = utterance_meta
                  .lock()
                  .map(|m| m.clone())
                  .unwrap_or_default();
              let _ = tx_utt.send(crate::audio::Utterance {
                audio: crate::audio::AudioChunk {
                  data: audio,
                  channels,
                  sample_rate,
                },
                kind: meta.kind,
                attachment: meta.attachment,
                text: None,
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
  tx_utt: Sender<crate::audio::Utterance>,
  vad_thresh: f32,
  end_silence_ms: u64,
  min_utt_ms: u64,
  hangover_ms: u64,
  playback_active: Arc<AtomicBool>,
  gate_until_ms: Arc<AtomicU64>,
  interrupt_counter: Arc<AtomicU64>,
  utt_buf: Arc<Mutex<Vec<f32>>>,
  user_speaking: Arc<AtomicBool>,
  last_voice_ms: Arc<AtomicU64>,
  stop_sent: Arc<AtomicBool>,
  peak: Arc<Mutex<f32>>,
  ui: crate::state::UiState,
  volume: Arc<Mutex<f32>>,
  recording_paused: Arc<AtomicBool>,
  utterance_meta: Arc<Mutex<crate::state::UtteranceMeta>>,
  tx_ui: Sender<String>,
  mut err_fn: impl FnMut(cpal::StreamError) + Send + 'static,
) -> Result<cpal::Stream, cpal::BuildStreamError> {
  device.build_input_stream(
    config,
    move |data: &[i16], _| {
      if recording_paused.load(Ordering::Relaxed) {
        // Flush buffer if not empty
        let mut b = utt_buf.lock().unwrap();
        if !b.is_empty() {
          let audio = std::mem::take(&mut *b);
          let denom = (sample_rate as u64).saturating_mul(channels as u64).max(1);
          let dur_ms = (audio.len() as u64).saturating_mul(1000) / denom;
          if dur_ms >= min_utt_ms {
            crate::util::SPEECH_END_AT.store(
              crate::util::now_ms(&START_INSTANT),
              std::sync::atomic::Ordering::SeqCst,
            );
            let meta = utterance_meta
                .lock()
                .map(|m| m.clone())
                .unwrap_or_default();
            let _ = tx_utt.send(crate::audio::Utterance {
              audio: crate::audio::AudioChunk {
                data: audio,
                channels,
                sample_rate,
              },
              kind: meta.kind,
              attachment: meta.attachment,
              text: None,
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

      // read live, not from the value this stream was built with: the
      // settings popup can change them while the microphone is open
      let vad_thresh = crate::state::GLOBAL_STATE
        .get()
        .map(|s| s.vad_threshold())
        .unwrap_or(vad_thresh);

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

        // Interrupt while the agent is audible. `playback_active` cannot be
        // used here: it drops every time the queue empties between chunks, so
        // speaking in one of those gaps (which is most of the start of a
        // phrase) was silently ignored.
        let agent_audible = crate::state::GLOBAL_STATE
          .get()
          .map(|st| st.playback.speaking.load(Ordering::Relaxed))
          .unwrap_or_else(|| playback_active.load(Ordering::Relaxed));
        // One interrupt per burst of speech. Without this hold-off every
        // audio callback raised another one for as long as you kept talking,
        // printing a wall of "USER interrupted".
        let gate_open =
          crate::util::now_ms(start_instant) >= INTERRUPT_GATE_MS.load(Ordering::Relaxed);
        if agent_audible && gate_open {
          // silence audio
          let mut vol = volume.lock().unwrap();
          *vol = 0.0;
          interrupt_counter.fetch_add(1, Ordering::SeqCst);
          let _ = tx_ui.send("user_interrupt_show|".to_string());
          stop_sent.store(true, Ordering::Relaxed);
          INTERRUPT_GATE_MS.store(
            crate::util::now_ms(start_instant).saturating_add(INTERRUPT_COOLDOWN_MS),
            Ordering::Relaxed,
          );
          gate_until_ms.store(
            crate::util::now_ms(start_instant).saturating_add(hangover_ms),
            Ordering::Relaxed,
          );
          playback_active.store(false, Ordering::Relaxed);
          stop_sent.store(false, Ordering::Relaxed);
        }
      } else if user_speaking.load(Ordering::Relaxed) {
        {
          let mut b = utt_buf.lock().unwrap();
          b.extend_from_slice(&tmp);
        }
        let last = last_voice_ms.load(Ordering::Relaxed);
        if last > 0
          && !crate::state::GLOBAL_STATE
            .get()
            .unwrap()
            .ptt
            .load(Ordering::Relaxed)
          && crate::util::now_ms(start_instant).saturating_sub(last)
            >= crate::state::GLOBAL_STATE
              .get()
              .map(|s| s.end_silence_ms.load(Ordering::Relaxed))
              .unwrap_or(end_silence_ms)
        {
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
              let meta = utterance_meta
                  .lock()
                  .map(|m| m.clone())
                  .unwrap_or_default();
              let _ = tx_utt.send(crate::audio::Utterance {
                audio: crate::audio::AudioChunk {
                  data: audio,
                  channels,
                  sample_rate,
                },
                kind: meta.kind,
                attachment: meta.attachment,
                text: None,
              });
            } else {
              // FIX: match f32 behavior (warn + drop)
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

fn build_input_u16(
  start_instant: &'static OnceLock<Instant>,
  device: &cpal::Device,
  config: &cpal::StreamConfig,
  channels: u16,
  sample_rate: u32,
  tx_utt: Sender<crate::audio::Utterance>,
  vad_thresh: f32,
  end_silence_ms: u64,
  min_utt_ms: u64,
  hangover_ms: u64,
  playback_active: Arc<AtomicBool>,
  gate_until_ms: Arc<AtomicU64>,
  interrupt_counter: Arc<AtomicU64>,
  utt_buf: Arc<Mutex<Vec<f32>>>,
  user_speaking: Arc<AtomicBool>,
  last_voice_ms: Arc<AtomicU64>,
  stop_sent: Arc<AtomicBool>,
  peak: Arc<Mutex<f32>>,
  ui: crate::state::UiState,
  volume: Arc<Mutex<f32>>,
  recording_paused: Arc<AtomicBool>,
  utterance_meta: Arc<Mutex<crate::state::UtteranceMeta>>,
  tx_ui: Sender<String>,
  mut err_fn: impl FnMut(cpal::StreamError) + Send + 'static,
) -> Result<cpal::Stream, cpal::BuildStreamError> {
  device.build_input_stream(
    config,
    move |data: &[u16], _| {
      // Convert once (preserve existing behavior), and reuse for peak + utt_buf + resample
      let mut tmp = Vec::with_capacity(data.len());
      for &s in data {
        tmp.push((s as f32 / u16::MAX as f32) * 2.0 - 1.0);
      }

      let local_peak = peak_abs(&tmp);
      if let Ok(mut p) = peak.lock() {
        *p = local_peak;
      }

      if recording_paused.load(Ordering::Relaxed) {
        // flush buffer if not empty
        let mut b = utt_buf.lock().unwrap();
        if !b.is_empty() {
          let audio = std::mem::take(&mut *b);
          let denom = (sample_rate as u64).saturating_mul(channels as u64).max(1);
          let dur_ms = (audio.len() as u64).saturating_mul(1000) / denom;
          if dur_ms >= min_utt_ms {
            crate::util::SPEECH_END_AT.store(
              crate::util::now_ms(&START_INSTANT),
              std::sync::atomic::Ordering::SeqCst,
            );
            let meta = utterance_meta
                .lock()
                .map(|m| m.clone())
                .unwrap_or_default();
            let _ = tx_utt.send(crate::audio::Utterance {
              audio: crate::audio::AudioChunk {
                data: audio,
                channels,
                sample_rate,
              },
              kind: meta.kind,
              attachment: meta.attachment,
              text: None,
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
        return;
      }
      // read live, not from the value this stream was built with: the
      // settings popup can change them while the microphone is open
      let vad_thresh = crate::state::GLOBAL_STATE
        .get()
        .map(|s| s.vad_threshold())
        .unwrap_or(vad_thresh);

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

        // Interrupt while the agent is audible. `playback_active` cannot be
        // used here: it drops every time the queue empties between chunks, so
        // speaking in one of those gaps (which is most of the start of a
        // phrase) was silently ignored.
        let agent_audible = crate::state::GLOBAL_STATE
          .get()
          .map(|st| st.playback.speaking.load(Ordering::Relaxed))
          .unwrap_or_else(|| playback_active.load(Ordering::Relaxed));
        // One interrupt per burst of speech. Without this hold-off every
        // audio callback raised another one for as long as you kept talking,
        // printing a wall of "USER interrupted".
        let gate_open =
          crate::util::now_ms(start_instant) >= INTERRUPT_GATE_MS.load(Ordering::Relaxed);
        if agent_audible && gate_open {
          // silence audio
          let mut vol = volume.lock().unwrap();
          *vol = 0.0;
          interrupt_counter.fetch_add(1, Ordering::SeqCst);
          let _ = tx_ui.send("user_interrupt_show|".to_string());
          stop_sent.store(true, Ordering::Relaxed);
          INTERRUPT_GATE_MS.store(
            crate::util::now_ms(start_instant).saturating_add(INTERRUPT_COOLDOWN_MS),
            Ordering::Relaxed,
          );
          gate_until_ms.store(
            crate::util::now_ms(start_instant).saturating_add(hangover_ms),
            Ordering::Relaxed,
          );
          playback_active.store(false, Ordering::Relaxed);
          stop_sent.store(false, Ordering::Relaxed);
        }
      } else if user_speaking.load(Ordering::Relaxed) {
        {
          let mut b = utt_buf.lock().unwrap();
          b.extend_from_slice(&tmp);
        }
        let last = last_voice_ms.load(Ordering::Relaxed);
        if last > 0
          && !crate::state::GLOBAL_STATE
            .get()
            .unwrap()
            .ptt
            .load(Ordering::Relaxed)
          && crate::util::now_ms(start_instant).saturating_sub(last)
            >= crate::state::GLOBAL_STATE
              .get()
              .map(|s| s.end_silence_ms.load(Ordering::Relaxed))
              .unwrap_or(end_silence_ms)
        {
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
              let meta = utterance_meta
                  .lock()
                  .map(|m| m.clone())
                  .unwrap_or_default();
              let _ = tx_utt.send(crate::audio::Utterance {
                audio: crate::audio::AudioChunk {
                  data: audio,
                  channels,
                  sample_rate,
                },
                kind: meta.kind,
                attachment: meta.attachment,
                text: None,
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
