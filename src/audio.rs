// ------------------------------------------------------------------
//  Audio processing
// ------------------------------------------------------------------

use cpal::traits::{DeviceTrait, HostTrait};
use std::path::Path;

// API
// ------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct AudioChunk {
  pub data: Vec<f32>, // interleaved
  pub channels: u16,
  pub sample_rate: u32,
}

/// Convert a slice of f32 samples to 16‑bit signed PCM.
pub fn f32_to_i16(samples: &[f32]) -> Vec<i16> {
  samples
    .iter()
    .map(|s| {
      let v = s.clamp(-1.0, 1.0);
      (v * i16::MAX as f32) as i16
    })
    .collect()
}

/// The Linux builds link ALSA statically, which bakes ALSA_PLUGIN_DIR in as
/// the build container's path (e.g. /opt/alsa-static/lib/alsa-lib). That path
/// doesn't exist on the machine actually running the binary, so loading the
/// PCM plugin behind "default" (commonly the pulse/pipewire bridge) fails and
/// cpal reports no usable input/output device - even though the ALSA config
/// itself resolves fine. Point ALSA at whichever real plugin directory this
/// distro actually uses, unless the user already set one.
#[cfg(target_os = "linux")]
pub fn ensure_alsa_plugin_dir() {
  if std::env::var_os("ALSA_PLUGIN_DIR").is_some() {
    return;
  }
  const CANDIDATES: &[&str] = &[
    "/usr/lib/x86_64-linux-gnu/alsa-lib",
    "/usr/lib/aarch64-linux-gnu/alsa-lib",
    "/usr/lib/arm-linux-gnueabihf/alsa-lib",
    "/usr/lib64/alsa-lib",
    "/usr/lib/alsa-lib",
    "/usr/local/lib/alsa-lib",
  ];
  for dir in CANDIDATES {
    let path = Path::new(dir);
    let has_plugin = path
      .read_dir()
      .map(|mut entries| {
        entries.any(|e| {
          e.ok()
            .is_some_and(|e| e.file_name().to_string_lossy().starts_with("libasound_module_"))
        })
      })
      .unwrap_or(false);
    if has_plugin {
      // SAFETY: called once at startup before any thread is spawned or reads the environment.
      unsafe { std::env::set_var("ALSA_PLUGIN_DIR", dir) };
      return;
    }
  }
}

#[cfg(not(target_os = "linux"))]
pub fn ensure_alsa_plugin_dir() {}

pub fn pick_input_stream(host: &cpal::Host) -> Result<(cpal::Device, cpal::Stream), String> {
  let err = || {
    "No usable microphone stream could be opened.\n".to_string()
      + "    • On MacOS: System Settings → Privacy & Security → Microphone → allow your app/Terminal\n"
      + "    • Also check System Settings → Sound → Input\n"
  };
  let dev = host.default_input_device().ok_or_else(err)?;
  let cfg = dev.default_input_config().map_err(|_| err())?;
  let stream = dev
    .build_input_stream(&cfg.clone().into(), |_data: &[f32], _| {}, |_err| {}, None)
    .map_err(|_| err())?;
  Ok((dev, stream))
}

pub fn pick_output_stream(host: &cpal::Host) -> Result<(cpal::Device, cpal::Stream), String> {
  let err = || {
    "No usable output stream could be opened.".to_string()
      + "   • On MacOS: System Settings → Sound → Output (select a device)"
  };
  let dev = host.default_output_device().ok_or_else(err)?;
  let cfg = dev.default_output_config().map_err(|_| err())?;
  let stream = dev
    .build_output_stream(
      &cfg.clone().into(),
      |data: &mut [f32], _| data.fill(0.0),
      |_err| {},
      None,
    )
    .map_err(|_| err())?;
  Ok((dev, stream))
}

/// Linear interpolation resample of interleaved audio.
pub fn resample_interleaved_linear(
  input: &[f32],
  channels: u16,
  in_sr: u32,
  out_sr: u32,
) -> Vec<f32> {
  if in_sr == out_sr || input.is_empty() {
    return input.to_vec();
  }
  let ch = channels as usize;
  let frames = input.len() / ch;
  // De-interleave
  let mut per_ch: Vec<Vec<f32>> = vec![Vec::with_capacity(frames); ch];
  for f in 0..frames {
    for c in 0..ch {
      per_ch[c].push(input[f * ch + c]);
    }
  }
  let mut per_ch_rs: Vec<Vec<f32>> = Vec::with_capacity(ch);
  for c in 0..ch {
    per_ch_rs.push(resample_linear(&per_ch[c], in_sr, out_sr));
  }
  let out_frames = per_ch_rs[0].len();
  let mut out = Vec::with_capacity(out_frames * ch);
  for f in 0..out_frames {
    for c in 0..ch {
      out.push(per_ch_rs[c][f]);
    }
  }
  out
}

/// Linear interpolation resample of mono audio.
pub fn resample_linear(input: &[f32], in_sr: u32, out_sr: u32) -> Vec<f32> {
  if in_sr == out_sr || input.is_empty() {
    return input.to_vec();
  }
  let ratio = out_sr as f64 / in_sr as f64;
  let out_len = ((input.len() as f64) * ratio).round() as usize;
  let mut out = Vec::with_capacity(out_len);
  for i in 0..out_len {
    let src_pos = (i as f64) / ratio;
    let idx = src_pos.floor() as usize;
    let frac = (src_pos - idx as f64) as f32;
    let a = *input.get(idx).unwrap_or(&0.0);
    let b = *input.get(idx + 1).unwrap_or(&a);
    out.push(a + (b - a) * frac);
  }
  out
}

pub fn resample_to(input: &[f32], channels: u16, in_sr: u32, out_sr: u32) -> Vec<f32> {
  #[allow(unused_imports)]
  use std::fmt::Debug;
  // crate::log::log(
  //   "debug",
  //   &format!(
  //     "[resample_to] in {} samples@{}Hz, out {}Hz, len {}",
  //     input.len(),
  //     in_sr,
  //     out_sr,
  //     input.len()
  //   ),
  // );
  if in_sr == out_sr || input.is_empty() {
    return input.to_vec();
  }
  // mono
  if channels == 1 {
    resample_linear(input, in_sr, out_sr)
  } else {
    // interleaved
    resample_interleaved_linear(input, channels, in_sr, out_sr)
  }
}

pub fn convert_to_mono(utt: &crate::audio::AudioChunk) -> Vec<f32> {
  let pcm_f32 = &utt.data;
  if utt.channels == 1 {
    pcm_f32.clone()
  } else {
    let ch = utt.channels as usize;
    let frames = pcm_f32.len() / ch;
    let mut mono = Vec::with_capacity(frames);
    for f in 0..frames {
      let start = f * ch;
      let sum: f32 = pcm_f32[start..start + ch].iter().sum();
      mono.push(sum / ch as f32);
    }
    mono
  }
}

/// Downmix a chunk to mono and resample it to `target_sr`, so chunks from
/// different sources (mic input vs. TTS playback) can share one WAV file.
pub fn normalize_to_mono(chunk: &AudioChunk, target_sr: u32) -> Vec<f32> {
  let mono = convert_to_mono(chunk);
  resample_to(&mono, 1, chunk.sample_rate, target_sr)
}

/// Initialise a WAV writer thread that writes incoming audio chunks to a
/// mono wav file. The sample rate is taken from the first chunk; every chunk
/// (whatever its channel count or rate) is normalised to that format before
/// being written. `gap_ms` of silence is appended after each chunk.
/// Returns a channel sender that can be used to forward audio chunks.
pub fn init_wav_writer(path: &Path, gap_ms: u32) -> crossbeam_channel::Sender<AudioChunk> {
  let (tx, rx) = crossbeam_channel::unbounded::<AudioChunk>();
  let out_path = path.to_path_buf();
  std::thread::spawn(move || {
    use std::fs::File;
    use std::io::BufWriter;
    let mut writer_opt: Option<hound::WavWriter<BufWriter<File>>> = None;
    let mut target_sr = 0u32;
    for chunk in rx.iter() {
      if writer_opt.is_none() {
        target_sr = chunk.sample_rate;
        let spec = hound::WavSpec {
          channels: 1,
          sample_rate: target_sr,
          bits_per_sample: 16,
          sample_format: hound::SampleFormat::Int,
        };
        writer_opt = match hound::WavWriter::create(&out_path, spec) {
          Ok(w) => Some(w),
          Err(e) => {
            eprintln!("Failed to create wav file {:?}: {}", out_path, e);
            return;
          }
        };
      }
      let Some(writer) = &mut writer_opt else { break };
      let samples = f32_to_i16(&normalize_to_mono(&chunk, target_sr));
      let silence = (target_sr as u64 * gap_ms as u64 / 1000) as usize;
      let write = samples
        .into_iter()
        .chain(std::iter::repeat(0_i16).take(silence))
        .try_for_each(|s| writer.write_sample(s))
        .and_then(|_| writer.flush());
      if let Err(e) = write {
        eprintln!("Failed to write to wav file {:?}: {}", out_path, e);
        break;
      }
    }
    if let Some(writer) = writer_opt {
      if let Err(e) = writer.finalize() {
        eprintln!("Failed to finalize wav file: {}", e);
      }
    }
  });
  tx
}

/// Write plain text to a file.
pub fn write_txt(path: &Path, text: &str) -> Result<(), std::io::Error> {
  std::fs::write(path, text)
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn normalize_to_mono_downmixes_and_resamples() {
    // 4 stereo frames at 48 kHz -> 2 mono frames at 24 kHz
    let chunk = AudioChunk {
      data: vec![0.2, 0.4, 0.2, 0.4, 0.2, 0.4, 0.2, 0.4],
      channels: 2,
      sample_rate: 48_000,
    };
    let out = normalize_to_mono(&chunk, 24_000);
    assert_eq!(out.len(), 2);
    for v in out {
      assert!((v - 0.3).abs() < 1e-6, "expected downmixed 0.3, got {v}");
    }
  }

  #[test]
  fn normalize_to_mono_is_identity_for_matching_mono() {
    let chunk = AudioChunk { data: vec![0.1, -0.5, 0.9], channels: 1, sample_rate: 16_000 };
    assert_eq!(normalize_to_mono(&chunk, 16_000), chunk.data);
  }

  #[test]
  fn wav_writer_produces_mono_file_at_first_chunk_rate() {
    let dir = std::env::temp_dir().join(format!("vtmate_wav_test_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("out.wav");
    let tx = init_wav_writer(&path, 0);
    // first chunk: mono 16 kHz, 1600 frames (100 ms)
    tx.send(AudioChunk { data: vec![0.1; 1600], channels: 1, sample_rate: 16_000 }).unwrap();
    // second chunk: stereo 48 kHz, 4800 frames (100 ms) -> must become 1600 mono frames
    tx.send(AudioChunk { data: vec![0.2; 9600], channels: 2, sample_rate: 48_000 }).unwrap();
    drop(tx);
    // wait for the writer thread to finalize
    let mut reader = None;
    for _ in 0..200 {
      std::thread::sleep(std::time::Duration::from_millis(10));
      if let Ok(r) = hound::WavReader::open(&path) {
        if r.len() == 3200 {
          reader = Some(r);
          break;
        }
      }
    }
    let reader = reader.expect("wav file not finalized with expected length");
    let spec = reader.spec();
    assert_eq!(spec.channels, 1);
    assert_eq!(spec.sample_rate, 16_000);
    assert_eq!(reader.len(), 3200);
    std::fs::remove_dir_all(&dir).ok();
  }
}
