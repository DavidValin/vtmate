// ------------------------------------------------------------------
//  Audio processing
// ------------------------------------------------------------------

use cpal::traits::{DeviceTrait, HostTrait};
use std::path::{Path, PathBuf};

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

/// Initialise a WAV writer thread that writes incoming audio chunks to a wav file.
/// Returns a channel sender that can be used to forward audio chunks.
pub fn init_wav_writer(path: &Path) -> crossbeam_channel::Sender<AudioChunk> {
  let (tx, rx) = crossbeam_channel::bounded::<AudioChunk>(1);
  let out_path = path.to_path_buf();
  std::thread::spawn(move || {
    use std::fs::File;
    let mut writer_opt: Option<hound::WavWriter<File>> = None;
    for chunk in rx.iter() {
      if writer_opt.is_none() {
        let spec = hound::WavSpec {
          channels: chunk.channels,
          sample_rate: chunk.sample_rate,
          bits_per_sample: 16,
          sample_format: hound::SampleFormat::Int,
        };
        let f = match File::create(&out_path) {
          Ok(f) => f,
          Err(e) => {
            eprintln!("Failed to create wav file {:?}: {}", out_path, e);
            return;
          }
        };
        writer_opt = match hound::WavWriter::new(f, spec) {
          Ok(w) => Some(w),
          Err(e) => {
            eprintln!("Failed to create wav writer: {}", e);
            return;
          }
        };
      }
      if let Some(writer) = &mut writer_opt {
        let samples = f32_to_i16(&chunk.data);
        for s in samples {
          if writer.write_sample(s).is_err() {
            eprintln!("Failed to write sample to wav file");
            break;
          }
        }
      }
    }
    if let Some(mut writer) = writer_opt {
      if writer.flush().is_err() {
        eprintln!("Failed to flush wav writer");
      }
    }
  });
  tx
}

/// Write plain text to a file.
pub fn write_txt(path: &Path, text: &str) -> Result<(), std::io::Error> {
  std::fs::write(path, text)
}
