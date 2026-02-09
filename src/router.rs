// ------------------------------------------------------------------
//  Router
// ------------------------------------------------------------------

use crossbeam_channel::{select, Receiver, Sender};

// API
// ------------------------------------------------------------------

pub fn router_thread(
  rx: Receiver<crate::audio::AudioChunk>,
  tx: Sender<crate::audio::AudioChunk>,
  out_channels: u16,
  stop_all_rx: Receiver<()>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
  loop {
    select! {
      recv(stop_all_rx) -> _ => break,
      recv(rx) -> msg => {
        let Ok(chunk) = msg else { break };
        let converted = convert_channels(&chunk.data, chunk.channels, out_channels);
        let out_chunk = crate::audio::AudioChunk {
          data: converted,
          channels: out_channels,
          sample_rate: chunk.sample_rate,
        };
        let _ = tx.send(out_chunk);
      }
    }
  }
  Ok(())
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
