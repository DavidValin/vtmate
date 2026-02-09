// ------------------------------------------------------------------
//  Audio processing
// ------------------------------------------------------------------

use cpal::traits::{DeviceTrait, HostTrait};

// API
// ------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct AudioChunk {
  pub data: Vec<f32>, // interleaved
  pub channels: u16,
  pub sample_rate: u32,
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

  // Resample each channel
  let mut per_ch_rs: Vec<Vec<f32>> = Vec::with_capacity(ch);
  for c in 0..ch {
    per_ch_rs.push(resample_linear(&per_ch[c], in_sr, out_sr));
  }

  // Re-interleave
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
  if in_sr == out_sr || input.is_empty() {
    return input.to_vec();
  }
  // mono
  if channels == 1 {
    resample_linear(input, in_sr, out_sr)
  }
  // interleaved
  else {
    resample_interleaved_linear(input, channels, in_sr, out_sr)
  }
}
