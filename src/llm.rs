// ------------------------------------------------------------------
//  LLM handling
// ------------------------------------------------------------------

use crossbeam_channel::Receiver;
use std::io::{BufRead, BufReader};
use std::sync::{
  Arc,
  atomic::{AtomicU64, Ordering},
};

// API
// ------------------------------------------------------------------

pub fn ollama_stream_response_into(
  prompt: &str,
  ollama_url: &str,
  ollama_model: &str,
  stop_all_rx: Receiver<()>,
  interrupt_counter: Arc<AtomicU64>,
  expected_interrupt: u64,
  on_piece: &mut dyn FnMut(&str),
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
  #[derive(serde::Serialize)]
  struct Req<'a> {
    model: &'a str,
    prompt: &'a str,
    stream: bool,
  }

  crate::log::log("info", "Calling ollama (LLM system)");
  let client = reqwest::blocking::Client::new();
  let resp = client
    .post(ollama_url)
    .json(&Req {
      model: ollama_model,
      prompt,
      stream: true,
    })
    .send()?;

  if !resp.status().is_success() {
    return Err(format!("ollama HTTP {}", resp.status()).into());
  }

  crate::log::log("info", "Got response from ollama (LLM system)");
  let mut reader = BufReader::new(resp);
  let mut line = String::new();

  loop {
    if stop_all_rx.try_recv().is_ok() {
      return Ok(());
    }
    if interrupt_counter.load(Ordering::SeqCst) != expected_interrupt {
      return Ok(());
    }

    line.clear();
    let n = reader.read_line(&mut line)?;
    if n == 0 {
      break;
    }

    let v: serde_json::Value = match serde_json::from_str(line.trim()) {
      Ok(v) => v,
      Err(_) => continue,
    };

    if let Some(piece) = v.get("response").and_then(|x| x.as_str()) {
      if !piece.is_empty() {
        on_piece(piece);
      }
    }

    if v.get("done").and_then(|x| x.as_bool()) == Some(true) {
      break;
    }
  }

  Ok(())
}
