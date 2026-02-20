// ------------------------------------------------------------------
//  LLM handling
// ------------------------------------------------------------------

use std::sync::{Arc, atomic::AtomicU64};
use crossbeam_channel::Receiver;
use reqwest::StatusCode;
use futures_util::StreamExt;
use bytes::Bytes;

/// Stream response from Llama/Ollama endpoints, fallback if one fails, and mid-stream cancellation support
pub async fn llama_server_stream_response_into(
  prompt: &str,
  llama_host: &str,
  llama_model: &str,
  server_type: &str,
  stop_all_rx: &Receiver<()>,
  interrupt_counter: Arc<AtomicU64>,
  expected_interrupt: u64,
  on_piece: &mut dyn FnMut(&str),
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {

  #[derive(Clone, Copy, Debug)]
  enum ApiKind { OaiChat, OllamaGenerate, OllamaChat, LegacyCompletion }

  #[derive(serde::Serialize)]
  struct ChatMessage<'a> { role: &'a str, content: &'a str }

  #[derive(serde::Serialize)]
  struct OaiChatReq<'a> { model: &'a str, messages: Vec<ChatMessage<'a>>, stream: bool }

  #[derive(serde::Serialize)]
  struct OllamaGenerateReq<'a> { model: &'a str, prompt: &'a str, stream: bool, #[serde(skip_serializing_if = "Option::is_none")] max_tokens: Option<u32> }

  #[derive(serde::Serialize)]
  struct OllamaChatReq<'a> { model: &'a str, messages: Vec<ChatMessage<'a>>, stream: bool }

  #[derive(serde::Serialize)]
  struct LegacyCompletionReq<'a> {
    prompt: &'a str,
    stream: bool,
    n_predict: u32,
    temperature: f32,
    stop: Vec<&'a str>,
    repeat_last_n: u32,
    repeat_penalty: f32,
    top_k: u32,
    top_p: f32,
    min_p: f32,
    tfs_z: f32,
    typical_p: f32,
    presence_penalty: f32,
    frequency_penalty: f32,
    mirostat: u8,
    mirostat_tau: f32,
    mirostat_eta: f32,
    grammar: &'a str,
    n_probs: u32,
    min_keep: u32,
    image_data: Vec<&'a str>,
    cache_prompt: bool,
    api_key: &'a str,
    slot_id: i32,
  }

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
        out.push((format!("http://{}/completion", base), ApiKind::LegacyCompletion));
        out.push((format!("http://{}/api/chat", base), ApiKind::OllamaChat));
      }
      "ollama" => {
        out.push((format!("http://{}/v1/generate", base), ApiKind::OllamaGenerate));
        out.push((format!("http://{}/api/chat", base), ApiKind::OllamaChat));
        out.push((format!("http://{}/v1/chat/completions", base), ApiKind::OaiChat));
        out.push((format!("http://{}/completion", base), ApiKind::LegacyCompletion));
      }
      _ => {
        out.push((format!("http://{}/v1/generate", base), ApiKind::OllamaGenerate));
        out.push((format!("http://{}/api/chat", base), ApiKind::OllamaChat));
        out.push((format!("http://{}/v1/chat/completions", base), ApiKind::OaiChat));
        out.push((format!("http://{}/completion", base), ApiKind::LegacyCompletion));
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
        let messages = vec![
          ChatMessage { role: "system", content: "You are a helpful assistant." },
          ChatMessage { role: "user", content: prompt },
        ];
        client.post(&url).json(&OaiChatReq { model: llama_model, messages, stream: true })
      }
      ApiKind::OllamaGenerate => {
        client.post(&url).json(&OllamaGenerateReq { model: llama_model, prompt, stream: true, max_tokens: Some(1024) })
      }
      ApiKind::OllamaChat => {
        let messages = vec![
          ChatMessage { role: "system", content: "You are a helpful assistant." },
          ChatMessage { role: "user", content: prompt },
        ];
        client.post(&url).json(&OllamaChatReq { model: llama_model, messages, stream: true })
      }
      ApiKind::LegacyCompletion => {
        client.post(&url).json(&LegacyCompletionReq {
          prompt,
          stream: true,
          n_predict: 400,
          temperature: 0.7,
          stop: vec!["</s>", "Assistant:", "User:"],
          repeat_last_n: 256,
          repeat_penalty: 1.18,
          top_k: 40,
          top_p: 0.95,
          min_p: 0.05,
          tfs_z: 1.0,
          typical_p: 1.0,
          presence_penalty: 0.0,
          frequency_penalty: 0.0,
          mirostat: 0,
          mirostat_tau: 5.0,
          mirostat_eta: 0.1,
          grammar: "",
          n_probs: 0,
          min_keep: 0,
          image_data: vec![],
          cache_prompt: true,
          api_key: "",
          slot_id: -1,
        })
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
                ApiKind::LegacyCompletion => {
                  if let Some(content) = v.get("content").and_then(|c| c.as_str()) { on_piece(content); }
                  if v.get("stop").and_then(|s| s.as_bool()) == Some(true) { return Ok(()); }
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