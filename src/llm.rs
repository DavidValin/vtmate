// ------------------------------------------------------------------
//  LLM handling
// ------------------------------------------------------------------

use bytes::Bytes;
use futures_util::StreamExt;
use reqwest::StatusCode;
use serde_json::json;
use std::sync::{Arc, atomic::AtomicU64};

/// Stream response from Llama/Ollama endpoints, fallback if one fails, and mid-stream cancellation support.
/// When `include_tools` is true, the LLM may return tool_calls which are delivered via `on_tool_call`.
/// Reasoning tokens (from models like Gemma 4) are delivered via `on_reasoning`.
pub async fn llama_server_stream_response_into(
  messages: &Vec<crate::conversation::ChatMessage>,
  llama_host: &str,
  llama_model: &str,
  server_type: &str,
  interrupt_counter: Arc<AtomicU64>,
  expected_interrupt: u64,
  on_piece: &mut dyn FnMut(&str),
  include_tools: bool,
  tools: &[String],
  mut on_tool_call: Option<&mut dyn FnMut(&serde_json::Value)>,
  mut on_reasoning: Option<&mut dyn FnMut(&str)>,
  think: bool,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
  crate::log::log(
    "debug",
    &format!(
      "llama_server_stream_response_into called with include_tools: {} tools: {:?}",
      include_tools, tools
    ),
  );
  #[derive(Clone, Copy, Debug)]
  enum ApiKind {
    OaiChat,
    OllamaChat,
  }

  /// Accumulates one streamed tool call across SSE chunks, keyed by `index`.
  #[derive(Default, Clone)]
  struct ToolCallBuilder {
    id: String,
    name: String,
    arguments: String,
  }

  /// Turn accumulated builders into complete tool_call JSON values, draining the buffer.
  fn finalize_tool_calls(builders: &mut Vec<Option<ToolCallBuilder>>) -> Vec<serde_json::Value> {
    builders
      .drain(..)
      .flatten()
      .filter(|b| !b.name.is_empty())
      .map(|b| {
        json!({
          "id": b.id,
          "type": "function",
          "function": {
            "name": b.name,
            "arguments": b.arguments
          }
        })
      })
      .collect()
  }

  /// Finalize and dispatch any pending tool calls, if there are any and a callback is set.
  fn flush_tool_calls(
    builders: &mut Vec<Option<ToolCallBuilder>>,
    on_tool_call: &mut Option<&mut dyn FnMut(&serde_json::Value)>,
  ) {
    if builders.is_empty() {
      return;
    }
    let calls = finalize_tool_calls(builders);
    if let Some(cb) = on_tool_call.as_mut() {
      for tc in &calls {
        let name = tc
          .get("function")
          .and_then(|f| f.get("name"))
          .and_then(|n| n.as_str())
          .unwrap_or("");
        let args = tc
          .get("function")
          .and_then(|f| f.get("arguments"))
          .and_then(|a| a.as_str())
          .unwrap_or("");
        crate::log::send_line(&format!("\n\x1b[32m {} called with {}", name, args));
        cb(tc);
      }
    }
  }

  /// Serialize one chat message for the wire, including tool calls on assistant turns
  /// and the call id on `tool` results.
  fn message_to_json(m: &crate::conversation::ChatMessage, kind: ApiKind) -> serde_json::Value {
    let mut obj = json!({ "role": m.role, "content": m.content });
    if let Some(calls) = m.tool_calls.as_ref().filter(|c| !c.is_empty()) {
      let calls: Vec<serde_json::Value> = calls
        .iter()
        .map(|tc| {
          let mut tc = tc.clone();
          if let Some(args) = tc.pointer_mut("/function/arguments") {
            match kind {
              // OpenAI-compatible endpoints expect `arguments` as a JSON-encoded string.
              ApiKind::OaiChat => {
                if !args.is_string() {
                  *args = serde_json::Value::String(args.to_string());
                }
              }
              // Ollama's native API expects an object.
              ApiKind::OllamaChat => {
                if let Some(parsed) = args
                  .as_str()
                  .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
                {
                  *args = parsed;
                }
              }
            }
          }
          tc
        })
        .collect();
      obj["tool_calls"] = serde_json::Value::Array(calls);
    }
    if m.role == "tool" {
      if let Some(id) = &m.tool_call_id {
        obj["tool_call_id"] = json!(id);
      }
      if let Some(name) = &m.tool_name {
        match kind {
          ApiKind::OaiChat => obj["name"] = json!(name),
          ApiKind::OllamaChat => obj["tool_name"] = json!(name),
        }
      }
    }
    obj
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
    let base = host
      .trim_start_matches("http://")
      .trim_start_matches("https://")
      .trim_end_matches('/');
    let mut out = Vec::new();
    match server_type {
      "llama-server" => {
        out.push((
          format!("http://{}/v1/chat/completions", base),
          ApiKind::OaiChat,
        ));
        out.push((format!("http://{}/api/chat", base), ApiKind::OaiChat));
      }
      "ollama" => {
        out.push((
          format!("http://{}/v1/chat/completions", base),
          ApiKind::OaiChat,
        ));
        out.push((format!("http://{}/api/chat", base), ApiKind::OllamaChat));
      }
      _ => {}
    }
    out
  }

  let client = reqwest::Client::new();
  let tries = candidates(llama_host, server_type);
  let mut last_err: Option<String> = None;

  for (url, kind) in tries {
    if interrupt_counter.load(std::sync::atomic::Ordering::SeqCst) != expected_interrupt {
      return Ok(());
    }

    crate::log::log("info", &format!("Trying endpoint: {}", url));

    let tools_payload = if include_tools {
      Some(crate::tools::tools_schemas(tools).unwrap_or_default())
    } else {
      None
    };
    crate::log::log(
      "debug",
      &format!(
        "{:?} payload tools: {:?}",
        kind,
        tools_payload.as_ref().map(|v| v
          .iter()
          .filter_map(|t| t.get("name").and_then(|n| n.as_str()))
          .collect::<Vec<_>>())
      ),
    );
    let payload = json!({
      "model": llama_model,
      "messages": messages.iter().map(|m| message_to_json(m, kind)).collect::<Vec<_>>(),
      "think": think,
      "stream": true,
      "tools": tools_payload,
      "tool_choice": if include_tools { Some("auto") } else { None::<&str> },
      "parallel_tool_calls": if include_tools { Some(false) } else { None::<bool> },
      "options": {
        "think": think
      },
    });
    let req = client.post(&url).json(&payload);

    let resp = match tokio::time::timeout(std::time::Duration::from_secs(120), req.send()).await {
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
      if should_fallback_status(status) {
        continue;
      } else {
        return Err(last_err.clone().unwrap().into());
      }
    }

    crate::log::log("info", &format!("Streaming response from: {}", url));
    // inside your endpoint loop
    let mut stream = resp.bytes_stream();
    // Accumulates fragmented tool_calls (see ToolCallBuilder) for this endpoint attempt.
    let mut tool_call_builders: Vec<Option<ToolCallBuilder>> = Vec::new();

    // Bound how long we wait for the next chunk: without this, a server that accepts
    // the connection but stops sending bytes mid-stream (no close, no data) hangs here
    // forever and is deaf to Esc/Undo, since those are only checked between chunks.
    let stall_timeout = std::time::Duration::from_secs(120);
    let poll_interval = std::time::Duration::from_millis(250);
    let mut since_last_chunk = std::time::Duration::ZERO;

    loop {
      // check stop signal, polled regardless of whether data is arriving
      if interrupt_counter.load(std::sync::atomic::Ordering::SeqCst) != expected_interrupt {
        return Ok(());
      }

      let chunk_result = match tokio::time::timeout(poll_interval, stream.next()).await {
        Ok(Some(r)) => {
          since_last_chunk = std::time::Duration::ZERO;
          r
        }
        Ok(None) => break, // stream ended normally
        Err(_) => {
          since_last_chunk += poll_interval;
          if since_last_chunk >= stall_timeout {
            crate::log::log(
              "error",
              &format!(
                "Streaming stalled (no data for {}s) at {}",
                stall_timeout.as_secs(),
                url
              ),
            );
            break; // fallback to next endpoint
          }
          continue;
        }
      };

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
          if payload == "[DONE]" {
            flush_tool_calls(&mut tool_call_builders, &mut on_tool_call);
            return Ok(());
          }

          // parse JSON safely
          if let Ok(v) = serde_json::from_str::<serde_json::Value>(payload) {
            // Handle new Llama3.2 style: {"message":{"content":...}}
            if let Some(message) = v.get("message") {
              if let Some(content) = message.get("content").and_then(|c| c.as_str()) {
                if !content.is_empty() {
                  on_piece(content);
                }
              }
              // Check for tool_calls in message (non-streaming response)
              if let Some(tcs) = message.get("tool_calls").and_then(|t| t.as_array()) {
                if !tcs.is_empty() {
                  if let Some(ref mut cb) = on_tool_call {
                    for tc in tcs {
                      // Extract name and arguments from inside "function" wrapper
                      if let Some(func) = tc.get("function") {
                        if let Some(name) = func.get("name").and_then(|v| v.as_str()) {
                          let args = func.get("arguments").and_then(|v| v.as_str()).unwrap_or("");
                          let msg = format!("\n\x1b[32m {} called with {}", name, args);
                          crate::log::send_line(&msg);
                        }
                      }
                      cb(tc);
                    }
                  }
                }
              }
              // End-of-stream signal from Ollama chat API
              if v.get("done").and_then(|x| x.as_bool()) == Some(true) {
                flush_tool_calls(&mut tool_call_builders, &mut on_tool_call);
                return Ok(());
              }
            } else {
              match kind {
                ApiKind::OaiChat | ApiKind::OllamaChat => {
                  if let Some(choices) = v.get("choices").and_then(|c| c.as_array()) {
                    for choice in choices {
                      if let Some(delta) = choice.get("delta") {
                        if let Some(content) = delta.get("content").and_then(|c| c.as_str()) {
                          if !content.is_empty() {
                            on_piece(content);
                          }
                        }
                        // Extract reasoning tokens (Gemma 4, DeepSeek, etc.)
                        if let Some(reasoning) = delta.get("reasoning").and_then(|r| r.as_str()) {
                          if !reasoning.is_empty() {
                            if let Some(ref mut cb) = on_reasoning {
                              cb(reasoning);
                            }
                          }
                        }
                        // Accumulate by index — arguments arrive as partial JSON text.
                        if let Some(tcs) = delta.get("tool_calls").and_then(|t| t.as_array()) {
                          for tc in tcs {
                            let idx = tc.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as usize;
                            if tool_call_builders.len() <= idx {
                              tool_call_builders.resize_with(idx + 1, || None);
                            }
                            let builder =
                              tool_call_builders[idx].get_or_insert_with(ToolCallBuilder::default);
                            if let Some(id) = tc.get("id").and_then(|v| v.as_str()) {
                              if !id.is_empty() {
                                builder.id = id.to_string();
                              }
                            }
                            if let Some(func) = tc.get("function") {
                              if let Some(name) = func.get("name").and_then(|v| v.as_str()) {
                                if !name.is_empty() {
                                  builder.name = name.to_string();
                                }
                              }
                              if let Some(args) = func.get("arguments").and_then(|v| v.as_str()) {
                                builder.arguments.push_str(args);
                              }
                            }
                          }
                        }
                      }
                      let finish_reason = choice.get("finish_reason").and_then(|r| r.as_str());
                      if finish_reason == Some("stop") || finish_reason == Some("tool_calls") {
                        flush_tool_calls(&mut tool_call_builders, &mut on_tool_call);
                        return Ok(());
                      }
                    }
                  }
                  if v.get("done").and_then(|x| x.as_bool()) == Some(true)
                    || v.get("status").and_then(|x| x.as_str()) == Some("completed")
                  {
                    flush_tool_calls(&mut tool_call_builders, &mut on_tool_call);
                    return Ok(());
                  }
                }
              }
            }
          }
        }
      }
    }

    // Stream ended (closed or stalled) without an explicit finish signal —
    // still flush any tool call fragments accumulated so far.
    flush_tool_calls(&mut tool_call_builders, &mut on_tool_call);
    return Ok(());
  }

  // all endpoints failed
  Err(
    last_err
      .unwrap_or_else(|| "No endpoint candidates succeeded".to_string())
      .into(),
  )
}
