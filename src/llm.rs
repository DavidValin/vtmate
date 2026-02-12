// ------------------------------------------------------------------
//  LLM handling
// ------------------------------------------------------------------

use crossbeam_channel::Receiver;
use serde_json::json;
use std::io::{BufRead, BufReader};
use std::sync::{
  Arc,
  atomic::{AtomicU64, Ordering},
};

// API
// ------------------------------------------------------------------

// llama-server client
pub fn llama_server_stream_response_into(
  prompt: &str,
  llama_url: &str,
  stop_all_rx: Receiver<()>,
  interrupt_counter: Arc<AtomicU64>,
  expected_interrupt: u64,
  on_piece: &mut dyn FnMut(&str),
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
  #[derive(serde::Serialize)]
  struct Message<'a> {
    role: &'a str,
    content: &'a str,
  }
  #[derive(serde::Serialize)]
  struct Req<'a> {
    model: &'a str,
    messages: Vec<Message<'a>>,
    stream: bool,
  }

  crate::log::log("info", "Calling llama-server (LLM system)");
  let client = reqwest::blocking::Client::new();
  let url = format!("{}/v1/chat/completions", llama_url.trim_end_matches('/'));
  let resp = client
    .post(&url)
    .json(&Req {
      model: "",
      messages: vec![Message {
        role: "user",
        content: prompt,
      }],
      stream: true,
    })
    .send()?;

  if !resp.status().is_success() {
    return Err(format!("llama-server HTTP {}", resp.status()).into());
  }

  crate::log::log("info", "Got response from llama-server (LLM system)");
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
    let trimmed = line.trim();
    if trimmed.is_empty() {
      continue;
    }
    // llama-server sends data: { ... }
    let content_line = if trimmed.starts_with("data: ") {
      &trimmed[6..]
    } else {
      trimmed
    };
    let v: serde_json::Value = match serde_json::from_str(content_line) {
      Ok(v) => v,
      Err(_) => continue,
    };
    if let Some(choices) = v.get("choices").and_then(|c| c.as_array()) {
      for choice in choices {
        if let Some(delta) = choice.get("delta").and_then(|d| d.as_object()) {
          if let Some(content) = delta.get("content").and_then(|c| c.as_str()) {
            if !content.is_empty() {
              on_piece(content);
            }
          }
        }
        if choice.get("finish_reason").and_then(|r| r.as_str()) == Some("stop") {
          return Ok(());
        }
      }
    }
  }
  Ok(())
}

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
  struct ChatReq<'a> {
    model: &'a str,
    messages: Vec<serde_json::Value>,
    stream: bool,
  }

  #[derive(serde::Serialize)]
  struct OldReq<'a> {
    model: &'a str,
    prompt: &'a str,
    stream: bool,
  }

  crate::log::log("info", "Calling ollama (LLM system)");
  let client = reqwest::blocking::Client::new();

  // First try new /v1/chat/completions endpoint
  let new_url = format!("{}/v1/chat/completions", ollama_url.trim_end_matches('/'));
  let new_resp = client
    .post(&new_url)
    .json(&ChatReq {
      model: ollama_model,
      messages: vec![json!({
        "role": "user",
        "content": prompt
      })],
      stream: true,
    })
    .send();

  let resp = match new_resp {
    Ok(r) if r.status().is_success() => r,
    _ => {
      // Fallback to old /api/generate endpoint
      let old_url = ollama_url.trim_end_matches('/');
      client
        .post(old_url)
        .json(&OldReq {
          model: ollama_model,
          prompt,
          stream: true,
        })
        .send()?
    }
  };

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

    // Determine whether the line contains a stream chunk for chat or old format
    let trimmed = line.trim();
    if trimmed.is_empty() {
      continue;
    }

    // For new endpoint, data lines start with "data: "
    let content_line = if trimmed.starts_with("data: ") {
      &trimmed[6..]
    } else {
      trimmed
    };

    let v: serde_json::Value = match serde_json::from_str(content_line) {
      Ok(v) => v,
      Err(_) => continue,
    };

    if let Some(choices) = v.get("choices").and_then(|c| c.as_array()) {
      // Handle chat completions format
      for choice in choices {
        if let Some(delta) = choice.get("delta").and_then(|d| d.as_object()) {
          if let Some(content) = delta.get("content").and_then(|c| c.as_str()) {
            if !content.is_empty() {
              on_piece(content);
            }
          }
        }
        if choice.get("finish_reason").and_then(|r| r.as_str()) == Some("stop") {
          return Ok(());
        }
      }
    } else if let Some(piece) = v.get("response").and_then(|x| x.as_str()) {
      // Old format
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
