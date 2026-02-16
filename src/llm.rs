// ------------------------------------------------------------------
//  LLM handling
// ------------------------------------------------------------------

use crossbeam_channel::Receiver;
use serde_json;
use std::io::{BufRead, BufReader};
use std::sync::{
  Arc,
  atomic::{AtomicU64, Ordering},
};

// API
// ------------------------------------------------------------------


// Compatibility Matrix for the unified streaming function
//______________________________________________________________________________
// Server       | Very Old        | Old                  | New                  |
// ------------ | --------------- | -------------------- | -------------------- |
// Ollama       | ❌ Pre-chat     | ✅ /api/chat         | ✅ /api+OpenAI       |
// llama.cpp    | ❌ Legacy only  | ⚠ /completion only   | ✅ /v1/chat          |
// Llamafile    | ❌ Legacy only  | ⚠ /completion only   | ✅ /completion+chat  |
// OpenAI prox. | N/A             | ✅ /v1/chat          | ✅ /v1/chat          |
//______________________________________________________________________________|
// Notes:
// - Streaming works for all supported endpoints.
// - Roles supported in chat endpoints; legacy /completion needs manual prompt encoding.
// - 'model' required for OpenAI/Ollama chat endpoints, ignored for legacy llama.cpp/Llamafile.
pub fn llama_server_stream_response_into(
    prompt: &str,
    llama_url: &str,
    llama_model: &str,
    stop_all_rx: Receiver<()>,
    interrupt_counter: Arc<AtomicU64>,
    expected_interrupt: u64,
    on_piece: &mut dyn FnMut(&str),
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {

    #[derive(Clone, Copy, Debug)]
    enum ApiKind {
      OaiChat,        // /v1/chat/completions
      OllamaChat,     // /api/chat
      LegacyCompletion, // llamafile /completion
    }

    #[derive(serde::Serialize)]
    struct ChatMessage<'a> {
      role: &'a str,
      content: &'a str,
    }

    #[derive(serde::Serialize)]
    struct OaiChatReq<'a> {
      model: &'a str,
      messages: Vec<ChatMessage<'a>>,
      stream: bool,
    }

    #[derive(serde::Serialize)]
    struct OllamaChatReq<'a> {
      model: &'a str,
      messages: Vec<ChatMessage<'a>>,
      stream: bool,
    }

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

    fn guess_kind_from_url(u: &str) -> ApiKind {
      if u.contains("/completion") {
        ApiKind::LegacyCompletion
      } else if u.contains("/api/chat") {
        ApiKind::OllamaChat
      } else {
        ApiKind::OaiChat
      }
    }

    fn should_fallback_status(code: reqwest::StatusCode) -> bool {
      code == reqwest::StatusCode::NOT_FOUND
        || code == reqwest::StatusCode::METHOD_NOT_ALLOWED
        || code == reqwest::StatusCode::UNPROCESSABLE_ENTITY
        || code == reqwest::StatusCode::BAD_REQUEST
        || code == reqwest::StatusCode::UNSUPPORTED_MEDIA_TYPE
    }

    fn candidates(url: &str) -> Vec<(String, ApiKind)> {
      let mut out = Vec::new();
      out.push((url.to_string(), guess_kind_from_url(url)));

      let base = base_from_full_url(url);

      out.push((format!("{}/v1/chat/completions", base), ApiKind::OaiChat));
      out.push((format!("{}/api/chat", base), ApiKind::OllamaChat));
      out.push((format!("{}/completion", base), ApiKind::LegacyCompletion));

      let mut seen = std::collections::HashSet::<String>::new();
      out.into_iter().filter(|(u, _)| seen.insert(u.clone())).collect()
    }

    let client = reqwest::blocking::Client::new();
    let tries = candidates(llama_url);
    let mut last_err: Option<String> = None;

    crate::log::log("info", &format!("Calling endpoint (auto-detect) starting at {llama_url}"));

    for (url, kind) in tries {
      if stop_all_rx.try_recv().is_ok() || interrupt_counter.load(Ordering::SeqCst) != expected_interrupt {
        return Ok(());
      }

      let req = match kind {
        ApiKind::OaiChat => {
          let messages = vec![
            ChatMessage { role: "system", content: "You are a helpful assistant." },
            ChatMessage { role: "user", content: prompt },
          ];
          client.post(url.clone())
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .json(&OaiChatReq { model: llama_model, messages, stream: true })
        }

        ApiKind::OllamaChat => {
          let messages = vec![
            ChatMessage { role: "system", content: "You are a helpful assistant." },
            ChatMessage { role: "user", content: prompt },
          ];
          client.post(url.clone())
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .json(&OllamaChatReq { model: llama_model, messages, stream: true })
        }

        ApiKind::LegacyCompletion => {
          // Llamafile legacy payload
          client.post(url.clone())
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .json(&LegacyCompletionReq {
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

      let resp = match req.send() {
        Ok(r) => r,
        Err(e) => {
          last_err = Some(format!("Request to {url} failed: {e}"));
          continue;
        }
      };

      if !resp.status().is_success() {
        let status = resp.status();
        let msg = format!("Endpoint {url} returned HTTP {status}");
        last_err = Some(msg.clone());
        if should_fallback_status(status) {
          continue;
        } else {
          return Err(msg.into());
        }
      }

      crate::log::log("info", &format!("Using endpoint: {url}"));
      crate::log::log("info", "Streaming response...");

      let mut reader = BufReader::new(resp);
      let mut line = String::new();

      loop {
        if stop_all_rx.try_recv().is_ok() || interrupt_counter.load(Ordering::SeqCst) != expected_interrupt {
          return Ok(());
        }

        line.clear();
        let n = reader.read_line(&mut line)?;
        if n == 0 { break; }

        let trimmed = line.trim();
        if trimmed.is_empty() { continue; }

        let payload = trimmed.strip_prefix("data:").unwrap_or(trimmed).trim();
        if payload == "[DONE]" { return Ok(()); }

        let v: serde_json::Value = match serde_json::from_str(payload) {
          Ok(v) => v,
          Err(_) => continue,
        };

        match kind {
          ApiKind::OaiChat | ApiKind::OllamaChat => {
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
            if let Some(msg) = v.get("message").and_then(|m| m.as_object()) {
              if let Some(content) = msg.get("content").and_then(|c| c.as_str()) {
                if !content.is_empty() { on_piece(content); }
              }
            }
            if v.get("done").and_then(|x| x.as_bool()) == Some(true)
              || v.get("status").and_then(|x| x.as_str()) == Some("completed") {
              return Ok(());
            }
          }

          ApiKind::LegacyCompletion => {
            if let Some(content) = v.get("content").and_then(|c| c.as_str()) {
              if !content.is_empty() { on_piece(content); }
            }
            if v.get("stop").and_then(|s| s.as_bool()) == Some(true) {
              return Ok(());
            }
          }
        }
      }

      return Ok(());
    }

    Err(last_err.unwrap_or_else(|| "No endpoint candidates succeeded".to_string()).into())
}


// PRIVATE
// ------------------------------------------------------------------

fn strip_trailing_slash(s: &str) -> &str {
  s.strip_suffix('/').unwrap_or(s)
}

pub fn base_from_full_url(u: &str) -> String {
  let u = strip_trailing_slash(u);

  for suffix in [
    // Ollama native
    "/api/generate",
    "/api/chat",
    "/api",
    // OpenAI-compatible
    "/v1/chat/completions",
    "/v1/completions",
    "/v1/responses",
    "/v1",
    // llama.cpp legacy
    "/completion",
  ] {
    if u.ends_with(suffix) {
      return u[..u.len() - suffix.len()].to_string();
    }
  }

  u.to_string()
}