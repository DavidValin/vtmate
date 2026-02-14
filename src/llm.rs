// ------------------------------------------------------------------
//  LLM handling
// ------------------------------------------------------------------

use crossbeam_channel::Receiver;
use serde_json::json;
use std::io::{BufRead, BufReader};
use std::sync::{
  atomic::{AtomicU64, Ordering},
  Arc,
};

// API
// ------------------------------------------------------------------

// llama-server client (multiversion)
// ----------------------------------
pub fn llama_server_stream_response_into(
  prompt: &str,
  llama_url: &str,
  stop_all_rx: Receiver<()>,
  interrupt_counter: Arc<AtomicU64>,
  expected_interrupt: u64,
  on_piece: &mut dyn FnMut(&str),
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
  #[derive(Clone, Copy, Debug)]
  enum ApiKind {
    OaiChat,      // /v1/chat/completions
    OaiCompletions, // /v1/completions
    LegacyCompletion, // /completion (llama.cpp legacy)
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
  struct OaiCompletionReq<'a> {
    model: &'a str,
    prompt: &'a str,
    stream: bool,
  }

  #[derive(serde::Serialize)]
  struct LegacyCompletionReq<'a> {
    prompt: &'a str,
    stream: bool,
  }

  fn candidates(llama_url: &str) -> Vec<(String, ApiKind)> {
    let mut out = Vec::new();

    // 1) Always try exactly what user passed first
    // Guess kind based on path (best effort); if unknown, assume OAI chat payload first.
    let guessed_kind = {
      let u = llama_url;
      if u.contains("/completion") {
        ApiKind::LegacyCompletion
      } else if u.contains("/v1/completions") {
        ApiKind::OaiCompletions
      } else {
        ApiKind::OaiChat
      }
    };
    out.push((llama_url.to_string(), guessed_kind));

    // 2) Then derive likely alternates from base
    let base = base_from_full_url(llama_url);
    let base_no_slash = strip_trailing_slash(&base).to_string();

    // Try OpenAI-compatible first (common for llama-server + newer llamafile)
    out.push((format!("{}/v1/chat/completions", base_no_slash), ApiKind::OaiChat));
    out.push((format!("{}/v1/completions", base_no_slash), ApiKind::OaiCompletions));

    // Then legacy llama.cpp endpoint (common for older builds / legacy setups)
    out.push((format!("{}/completion", base_no_slash), ApiKind::LegacyCompletion));

    // Also handle case where user base is already .../v1
    out.push((format!("{}/chat/completions", strip_trailing_slash(llama_url)), ApiKind::OaiChat));
    out.push((format!("{}/completions", strip_trailing_slash(llama_url)), ApiKind::OaiCompletions));

    // De-dupe while preserving order
    let mut seen = std::collections::HashSet::<String>::new();
    out.into_iter().filter(|(u, _)| seen.insert(u.clone())).collect()
  }

  fn should_fallback_status(code: reqwest::StatusCode) -> bool {
    // Wrong endpoint / method / not found / unsupported media type
    code == reqwest::StatusCode::NOT_FOUND
      || code == reqwest::StatusCode::METHOD_NOT_ALLOWED
      || code == reqwest::StatusCode::UNPROCESSABLE_ENTITY
      || code == reqwest::StatusCode::BAD_REQUEST
      || code == reqwest::StatusCode::UNSUPPORTED_MEDIA_TYPE
  }

  let client = reqwest::blocking::Client::new();
  let tries = candidates(llama_url);

  crate::log::log("info", &format!("Calling llama endpoint (auto-detect) starting at {llama_url}"));

  // Try endpoints until one works
  let mut last_err: Option<String> = None;

  for (url, kind) in tries {
    if stop_all_rx.try_recv().is_ok() {
      return Ok(());
    }
    if interrupt_counter.load(Ordering::SeqCst) != expected_interrupt {
      return Ok(());
    }

    let req = match kind {
      ApiKind::OaiChat => client
        .post(url.clone())
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .json(&OaiChatReq {
          model: "",
          messages: vec![ChatMessage { role: "user", content: prompt }],
          stream: true,
        }),
      ApiKind::OaiCompletions => client
        .post(url.clone())
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .json(&OaiCompletionReq { model: "", prompt, stream: true }),
      ApiKind::LegacyCompletion => client
        .post(url.clone())
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .json(&LegacyCompletionReq { prompt, stream: true }),
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

      // If it looks like the wrong endpoint/payload, try next candidate.
      if should_fallback_status(status) {
        continue;
      } else {
        return Err(msg.into());
      }
    }

    crate::log::log("info", &format!("Using llama endpoint: {url} ({kind:?})"));
    crate::log::log("info", "Got response, starting stream read");

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

      // Many servers use SSE: "data: ...", sometimes with "data: [DONE]"
      let payload = if let Some(rest) = trimmed.strip_prefix("data:") {
        rest.trim()
      } else {
        trimmed
      };

      if payload == "[DONE]" {
        return Ok(());
      }

      let v: serde_json::Value = match serde_json::from_str(payload) {
        Ok(v) => v,
        Err(_) => continue,
      };

      // ---- Case A: OpenAI-compatible streaming (/v1/chat/completions) ----
      if let Some(choices) = v.get("choices").and_then(|c| c.as_array()) {
        for choice in choices {
          // chat: delta.content
          if let Some(delta) = choice.get("delta").and_then(|d| d.as_object()) {
            if let Some(content) = delta.get("content").and_then(|c| c.as_str()) {
              if !content.is_empty() {
                on_piece(content);
              }
            }
          }

          // completions: choice.text
          if let Some(text) = choice.get("text").and_then(|t| t.as_str()) {
            if !text.is_empty() {
              on_piece(text);
            }
          }

          // finish
          if choice.get("finish_reason").and_then(|r| r.as_str()) == Some("stop") {
            return Ok(());
          }
        }
        continue;
      }

      // ---- Case B: legacy llama.cpp /completion streaming ----
      // Streaming format typically emits { "content": "...", "stop": false } chunks.
      if let Some(content) = v.get("content").and_then(|c| c.as_str()) {
        if !content.is_empty() {
          on_piece(content);
        }
      }
      if v.get("stop").and_then(|s| s.as_bool()) == Some(true) {
        return Ok(());
      }
    }

    return Ok(());
  }

  Err(last_err
    .unwrap_or_else(|| "No llama endpoint candidates succeeded".to_string())
    .into())
}


// ollama client (multiversion)
// ----------------------------
pub fn ollama_stream_response_into(
  prompt: &str,
  ollama_url: &str,
  ollama_model: &str,
  stop_all_rx: Receiver<()>,
  interrupt_counter: Arc<AtomicU64>,
  expected_interrupt: u64,
  on_piece: &mut dyn FnMut(&str),
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
  #[derive(Clone, Copy, Debug)]
  enum ApiKind {
    // OpenAI-compatible
    OaiChat,        // /v1/chat/completions
    OaiResponses,   // /v1/responses (some proxies / compat layers)
    OaiCompletions, // /v1/completions

    // Ollama native
    OllamaChat,     // /api/chat
    OllamaGenerate, // /api/generate
  }

  #[derive(serde::Serialize)]
  struct OaiChatReq<'a> {
    model: &'a str,
    messages: Vec<serde_json::Value>,
    stream: bool,
  }

  #[derive(serde::Serialize)]
  struct OaiResponsesReq<'a> {
    model: &'a str,
    input: serde_json::Value,
    stream: bool,
  }

  #[derive(serde::Serialize)]
  struct OaiCompletionReq<'a> {
    model: &'a str,
    prompt: &'a str,
    stream: bool,
  }

  #[derive(serde::Serialize)]
  struct OllamaChatReq<'a> {
    model: &'a str,
    messages: Vec<serde_json::Value>,
    stream: bool,
  }

  #[derive(serde::Serialize)]
  struct OllamaGenerateReq<'a> {
    model: &'a str,
    prompt: &'a str,
    stream: bool,
  }

  fn guess_kind_from_url(u: &str) -> ApiKind {
    if u.contains("/v1/chat/completions") {
      ApiKind::OaiChat
    } else if u.contains("/v1/responses") {
      ApiKind::OaiResponses
    } else if u.contains("/v1/completions") {
      ApiKind::OaiCompletions
    } else if u.contains("/api/chat") {
      ApiKind::OllamaChat
    } else if u.contains("/api/generate") {
      ApiKind::OllamaGenerate
    } else {
      // Default: try OAI chat first since most "OpenAI compatible URL" setups want that.
      ApiKind::OaiChat
    }
  }

  fn should_fallback_status(code: reqwest::StatusCode) -> bool {
    // Common "wrong endpoint / wrong schema" statuses
    code == reqwest::StatusCode::NOT_FOUND
      || code == reqwest::StatusCode::METHOD_NOT_ALLOWED
      || code == reqwest::StatusCode::UNPROCESSABLE_ENTITY
      || code == reqwest::StatusCode::BAD_REQUEST
      || code == reqwest::StatusCode::UNSUPPORTED_MEDIA_TYPE
  }

  fn candidates(ollama_url: &str) -> Vec<(String, ApiKind)> {
    let mut out: Vec<(String, ApiKind)> = Vec::new();

    // 1) Always try exactly what the user passed first.
    out.push((ollama_url.to_string(), guess_kind_from_url(ollama_url)));

    // 2) Then derive likely alternates from a base.
    let base = base_from_full_url(ollama_url);

    // ---- OpenAI-compatible variants (common for "Ollama OpenAI compatible URL") ----
    out.push((format!("{}/v1/chat/completions", base), ApiKind::OaiChat));
    out.push((format!("{}/v1/responses", base), ApiKind::OaiResponses));
    out.push((format!("{}/v1/completions", base), ApiKind::OaiCompletions));

    // ---- Native Ollama variants ----
    out.push((format!("{}/api/chat", base), ApiKind::OllamaChat));
    out.push((format!("{}/api/generate", base), ApiKind::OllamaGenerate));

    // Also handle the case where the passed URL itself is ".../api" or ".../v1"
    {
      let u = strip_trailing_slash(ollama_url);
      if u.ends_with("/api") {
        out.push((format!("{}/chat", u), ApiKind::OllamaChat));
        out.push((format!("{}/generate", u), ApiKind::OllamaGenerate));
      }
      if u.ends_with("/v1") {
        out.push((format!("{}/chat/completions", u), ApiKind::OaiChat));
        out.push((format!("{}/responses", u), ApiKind::OaiResponses));
        out.push((format!("{}/completions", u), ApiKind::OaiCompletions));
      }
    }

    // De-dupe while preserving order
    let mut seen = std::collections::HashSet::<String>::new();
    out.into_iter().filter(|(u, _)| seen.insert(u.clone())).collect()
  }

  let client = reqwest::blocking::Client::new();

  crate::log::log(
    "info",
    &format!("Calling ollama (auto-detect) starting at {ollama_url}"),
  );

  let tries = candidates(ollama_url);
  let mut last_err: Option<String> = None;

  // Choose an Ollama-native default model if none provided? Keep behavior: you pass model explicitly.
  // If empty model, Ollama will error — same as before. We keep interface/behavior intact.

  for (url, kind) in tries {
    if stop_all_rx.try_recv().is_ok() {
      return Ok(());
    }
    if interrupt_counter.load(Ordering::SeqCst) != expected_interrupt {
      return Ok(());
    }

    // Build request based on candidate kind
    let req = match kind {
      ApiKind::OaiChat => client
        .post(url.clone())
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .json(&OaiChatReq {
          model: ollama_model,
          messages: vec![json!({"role": "user", "content": prompt})],
          stream: true,
        }),

      ApiKind::OaiResponses => {
        // Send as responses-style "input" with chat-like structure; many compat layers accept either
        // string input or message objects. We'll use a simple string to be maximally compatible.
        client
          .post(url.clone())
          .header(reqwest::header::CONTENT_TYPE, "application/json")
          .json(&OaiResponsesReq {
            model: ollama_model,
            input: json!(prompt),
            stream: true,
          })
      }

      ApiKind::OaiCompletions => client
        .post(url.clone())
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .json(&OaiCompletionReq {
          model: ollama_model,
          prompt,
          stream: true,
        }),

      ApiKind::OllamaChat => client
        .post(url.clone())
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .json(&OllamaChatReq {
          model: ollama_model,
          messages: vec![json!({"role": "user", "content": prompt})],
          stream: true,
        }),

      ApiKind::OllamaGenerate => client
        .post(url.clone())
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .json(&OllamaGenerateReq {
          model: ollama_model,
          prompt,
          stream: true,
        }),
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

      // Try next candidate if it looks like "wrong endpoint/schema"
      if should_fallback_status(status) {
        continue;
      } else {
        return Err(msg.into());
      }
    }

    crate::log::log("info", &format!("Using ollama endpoint: {url}"));
    crate::log::log("info", "Got response from ollama, starting stream read");

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

      // Handle SSE framing: "data: ..." and OpenAI "[DONE]"
      let payload = if let Some(rest) = trimmed.strip_prefix("data:") {
        rest.trim()
      } else {
        trimmed
      };

      if payload == "[DONE]" {
        return Ok(());
      }

      // Native Ollama streaming is JSON-per-line without "data:" (usually),
      // but some proxies might still SSE-wrap it. We handle both.
      let v: serde_json::Value = match serde_json::from_str(payload) {
        Ok(v) => v,
        Err(_) => continue,
      };

      // ---------- OpenAI-compatible chat/completions ----------
      if let Some(choices) = v.get("choices").and_then(|c| c.as_array()) {
        for choice in choices {
          // chat: delta.content
          if let Some(delta) = choice.get("delta").and_then(|d| d.as_object()) {
            if let Some(content) = delta.get("content").and_then(|c| c.as_str()) {
              if !content.is_empty() {
                on_piece(content);
              }
            }
          }

          // completions: choice.text
          if let Some(text) = choice.get("text").and_then(|t| t.as_str()) {
            if !text.is_empty() {
              on_piece(text);
            }
          }

          if choice.get("finish_reason").and_then(|r| r.as_str()) == Some("stop") {
            return Ok(());
          }
        }
        continue;
      }

      // ---------- OpenAI Responses API streaming (varies across implementations) ----------
      // Common patterns:
      // - { "type": "response.output_text.delta", "delta": "..." }
      // - { "type": "...", "text": "..." }
      // - Or nested output objects
      if let Some(t) = v.get("type").and_then(|x| x.as_str()) {
        if t.contains("delta") {
          if let Some(delta) = v.get("delta").and_then(|x| x.as_str()) {
            if !delta.is_empty() {
              on_piece(delta);
            }
          }
          if let Some(text) = v.get("text").and_then(|x| x.as_str()) {
            if !text.is_empty() {
              on_piece(text);
            }
          }
        }
        // completion/stop signals for responses-style streams are inconsistent;
        // we'll also check for explicit done markers below.
      }

      // ---------- Native Ollama /api/generate ----------
      // Stream chunks: { "response": "...", "done": false, ... }
      if let Some(piece) = v.get("response").and_then(|x| x.as_str()) {
        if !piece.is_empty() {
          on_piece(piece);
        }
      }

      // ---------- Native Ollama /api/chat ----------
      // Stream chunks often: { "message": { "role": "...", "content": "..." }, "done": false }
      if let Some(msg) = v.get("message").and_then(|m| m.as_object()) {
        if let Some(content) = msg.get("content").and_then(|c| c.as_str()) {
          if !content.is_empty() {
            on_piece(content);
          }
        }
      }

      // ---------- Done markers ----------
      // Native Ollama: "done": true
      if v.get("done").and_then(|x| x.as_bool()) == Some(true) {
        return Ok(());
      }

      // Some responses-style implementations use "response" object with "status": "completed"
      if v.get("status").and_then(|x| x.as_str()) == Some("completed") {
        return Ok(());
      }
    }

    // Stream ended cleanly for the first working endpoint
    return Ok(());
  }

  Err(last_err
    .unwrap_or_else(|| "No ollama endpoint candidates succeeded".to_string())
    .into())
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