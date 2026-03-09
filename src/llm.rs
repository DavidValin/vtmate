// ------------------------------------------------------------------
//  LLM handling
// ------------------------------------------------------------------

use std::sync::{Arc, atomic::AtomicU64};
use crossbeam_channel::Receiver;
use reqwest::StatusCode;
use futures_util::StreamExt;
use bytes::Bytes;
use serde_json::json;

/// Stream response from Llama/Ollama endpoints, fallback if one fails, and mid-stream cancellation support
pub async fn llama_server_stream_response_into(
  messages: &Vec<crate::conversation::ChatMessage>,
  llama_host: &str,
  llama_model: &str,
  server_type: &str,
  stop_all_rx: &Receiver<()>,
  interrupt_counter: Arc<AtomicU64>,
  expected_interrupt: u64,
  on_piece: &mut dyn FnMut(&str),
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {

  #[derive(Clone, Copy, Debug)]
  enum ApiKind { OaiChat, OllamaGenerate, OllamaChat }


  fn should_fallback_status(code: StatusCode) -> bool {
    matches!(
      code,
      StatusCode::NOT_FOUND
      | StatusCode::METHOD_NOT_ALLOWED
      | StatusCode::UNPROCESSABLE_ENTITY
      | StatusCode::BAD_REQUEST
      | StatusCode::UNSUPPORTED_MEDIA_TYPE
    )
  }

  fn candidates(host: &str, server_type: &str) -> Vec<(String, ApiKind)> {
    let base = host.trim_start_matches("http://").trim_start_matches("https://").trim_end_matches('/');
    let mut out = Vec::new();
    match server_type {
      "llama-server" => {
        out.push((format!("http://{}/v1/chat/completions", base), ApiKind::OaiChat));
        out.push((format!("http://{}/api/chat", base), ApiKind::OaiChat));
      }
      "ollama" => {
        out.push((format!("http://{}/v1/generate", base), ApiKind::OllamaGenerate));
        out.push((format!("http://{}/api/chat", base), ApiKind::OllamaChat));
      }
      _ => {
        out.push((format!("http://{}/v1/chat/completions", base), ApiKind::OaiChat));
        out.push((format!("http://{}/api/chat", base), ApiKind::OllamaChat));
      }
    }
    out
  }

  let client = reqwest::Client::new();
  let tries = candidates(llama_host, server_type);
  let mut last_err: Option<String> = None;

  for (url, kind) in tries {
    if stop_all_rx.try_recv().is_ok() ||
      interrupt_counter.load(std::sync::atomic::Ordering::SeqCst) != expected_interrupt {
        return Ok(());
    }

    crate::log::log("info", &format!("Trying endpoint: {}", url));

    let req = match kind {
      ApiKind::OaiChat => {
        let payload = json!({
          "model": llama_model,
          "messages": messages.iter().map(|m| json!({ "role": m.role, "content": m.content })).collect::<Vec<_>>(),
          "stream": true
        });
        client.post(&url).json(&payload)
      }
      ApiKind::OllamaGenerate => {
        let prompt_str = messages.iter().map(|m| m.content.as_str()).collect::<Vec<&str>>().join("\n");
        let payload = json!({
          "model": llama_model,
          "prompt": prompt_str,
          "stream": true,
          "max_tokens": 1024
        });
        client.post(&url).json(&payload)
      }
      ApiKind::OllamaChat => {
        let payload = json!({
          "model": llama_model,
          "messages": messages.iter().map(|m| json!({ "role": m.role, "content": m.content })).collect::<Vec<_>>(),
          "stream": true
        });
        client.post(&url).json(&payload)
      }
    };

    let resp = match tokio::time::timeout(std::time::Duration::from_secs(3), req.send()).await {
      Ok(Ok(r)) => r,
      Ok(Err(e)) => {
        last_err = Some(format!("Request to {} failed: {}", url, e));
        log::warn!("{}", last_err.as_ref().unwrap());
        continue;
      }
      Err(_) => {
        last_err = Some(format!("Request to {} timed out", url));
        log::warn!("{}", last_err.as_ref().unwrap());
        continue;
      }
    };

    if !resp.status().is_success() {
      let status = resp.status();
      last_err = Some(format!("Endpoint {} returned HTTP {}", url, status));
      log::warn!("{}", last_err.as_ref().unwrap());
      if should_fallback_status(status) { continue; } else { return Err(last_err.clone().unwrap().into()); }
    }

    crate::log::log("info", &format!("Streaming response from: {}", url));
    // inside your endpoint loop
    let mut stream = resp.bytes_stream();

    while let Some(chunk_result) = stream.next().await {
      // check stop signal mid-stream
      if stop_all_rx.try_recv().is_ok() ||
        interrupt_counter.load(std::sync::atomic::Ordering::SeqCst) != expected_interrupt
      {
        return Ok(());
      }

      let chunk: Bytes = match chunk_result {
        Ok(b) => b,
        Err(e) => {
          crate::log::log("error", &format!("Streaming error at {}: {}", url, e));
          break; // fallback to next endpoint
        }
      };

      if let Ok(text) = std::str::from_utf8(&chunk) {
        // crate::log::log("debug", &format!("chunk: {}", text));
        for line in text.lines() {
          let payload = line.trim().strip_prefix("data:").unwrap_or(line).trim();
          if payload == "[DONE]" { return Ok(()); }

          // parse JSON safely
          if let Ok(v) = serde_json::from_str::<serde_json::Value>(payload) {
            // Handle new Llama3.2 style: {"message":{"content":...}}
            if let Some(message) = v.get("message") {
              if let Some(content) = message.get("content").and_then(|c| c.as_str()) {
                if !content.is_empty() { on_piece(content); }
              }
            } else {
              match kind {
                ApiKind::OaiChat | ApiKind::OllamaChat | ApiKind::OllamaGenerate => {
                  if let Some(choices) = v.get("choices").and_then(|c| c.as_array()) {
                    for choice in choices {
                      if let Some(delta) = choice.get("delta") {
                        if let Some(content) = delta.get("content").and_then(|c| c.as_str()) {
                          if !content.is_empty() { on_piece(content); }
                        }
                      }
                      if choice.get("finish_reason").and_then(|r| r.as_str()) == Some("stop") {
                        return Ok(());
                      }
                    }
                  }
                  if v.get("done").and_then(|x| x.as_bool()) == Some(true)
                    || v.get("status").and_then(|x| x.as_str()) == Some("completed")
                  {
                    return Ok(());
                  }
                }
              }
            }
          }
        }
      }
    }

    // success streaming completed
    return Ok(());
  }

  // all endpoints failed
  Err(last_err.unwrap_or_else(|| "No endpoint candidates succeeded".to_string()).into())
}