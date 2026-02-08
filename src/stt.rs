// ------------------------------------------------------------------
//  STT - Speech to Text
// ------------------------------------------------------------------

use hound;
use reqwest::blocking::multipart;
use std::io::Cursor;

// API
// ------------------------------------------------------------------

pub fn whisper_transcribe(
  pcm_chunks: &[i16],
  sample_rate: u32,
  channels: u16,
  openai_api_key: &str,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
  // Convert PCM to f32, resample to 16kHz mono, then encode to WAV in memory
  // Convert input to f32 samples
  let samples_f32: Vec<f32> = pcm_chunks
    .iter()
    .map(|&s| s as f32 / i16::MAX as f32)
    .collect();
  // Resample to 16kHz mono
  let resampled = crate::audio::resample_to(&samples_f32, channels, sample_rate, 16_000);
  let mono = crate::audio::mix_to_mono(&resampled, channels);
  // Convert back to i16
  let wav_data: Vec<i16> = mono
    .iter()
    .map(|&s| (s * i16::MAX as f32).clamp(-i16::MAX as f32, i16::MAX as f32) as i16)
    .collect();

  // Write WAV to memory
  let mut cursor = Cursor::new(Vec::<u8>::new());
  let spec = hound::WavSpec {
    channels: 1,
    sample_rate: 16_000,
    bits_per_sample: 16,
    sample_format: hound::SampleFormat::Int,
  };
  let mut writer = hound::WavWriter::new(&mut cursor, spec)?;
  for &sample in wav_data.iter() {
    writer.write_sample(sample)?;
  }
  writer.finalize()?;

  let wav_bytes = cursor.into_inner();

  // ---- multipart upload ----
  let file_part = multipart::Part::bytes(wav_bytes)
    .file_name("audio.wav")
    .mime_str("audio/wav")?;

  let form = multipart::Form::new()
    .text("model", "whisper-1")
    .text("temperature", "0.0")
    .text("temperature_inc", "0.2")
    .text("response_format", "json")
    .part("file", file_part);

  let client = reqwest::blocking::Client::new();
  let resp = client
    .post("http://127.0.0.1:8080/inference")
    .bearer_auth(openai_api_key)
    .multipart(form)
    .send()?;

  // Better error visibility if non-2xx
  let status = resp.status();
  let json: serde_json::Value = resp.json()?;
  if !status.is_success() {
    return Err(format!("Whisper HTTP error {}: {}", status, json).into());
  }

  Ok(json["text"].as_str().unwrap_or_default().to_string())
}
