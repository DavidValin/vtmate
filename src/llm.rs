// ------------------------------------------------------------------
//  LLM handling
// ------------------------------------------------------------------

use std::sync::{Arc, atomic::AtomicU64, OnceLock};

pub static TOOLS_SUPPORTED: OnceLock<bool> = OnceLock::new();
use crossbeam_channel::Receiver;
use reqwest::{StatusCode, Client};
use crate::tools::{get_available_tools};
use crate::tools::remember::RememberTool;
use crate::tools::store_memory::StoreMemoryTool;
use crate::tools::Tool;
use futures_util::StreamExt;
use serde_json::{Value, json};
use bytes::Bytes;

fn extract_tool_calls_from_choice(choice: &Value) -> Option<String> {
  let wrapper = json!({"choices":[choice]});
  crate::tools::handle_tool_call_from_json(&wrapper.to_string())
}

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

  let mut full_payload = String::new();
  #[derive(Clone, Copy, Debug)]
  enum ApiKind { OaiChat, OllamaGenerate, OllamaChat, LegacyCompletion }

  #[derive(serde::Serialize)]
  struct ChatMessage<'a> { role: &'a str, content: &'a str }

  #[derive(serde::Serialize)]
  struct OaiChatReq<'a> {
    model: &'a str,
    messages: Vec<ChatMessage<'a>>,
    stream: bool,
    tools: Option<Vec<serde_json::Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<&'a str>
  }

  #[derive(serde::Serialize)]
  struct OllamaGenerateReq<'a> {
    model: &'a str,
    prompt: &'a str,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    tools: Option<Vec<serde_json::Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<&'a str>
  }

  #[derive(serde::Serialize)]
  struct OllamaChatReq<'a> {
    model: &'a str,
    messages: Vec<ChatMessage<'a>>,
    stream: bool,
    tools: Option<Vec<serde_json::Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<&'a str>
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
    tools: Option<Vec<serde_json::Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<&'a str>
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
  let tool_schemas = get_available_tools()?;

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
        let tools_supported = crate::llm::TOOLS_SUPPORTED.get().copied().unwrap_or(false);
        let payload = serde_json::to_value(OaiChatReq {
          model: llama_model,
          messages,
          stream: true,
          tools: if tools_supported { Some(tool_schemas.clone()) } else { None },
          tool_choice: if tools_supported { Some("auto") } else { None }
        })?;
        // crate::log::log("debug", &format!("LLM payload: {}", payload));
        client.post(&url).json(&payload)
      }

      ApiKind::OllamaGenerate => {
        let tools_supported = crate::llm::TOOLS_SUPPORTED.get().copied().unwrap_or(false);
        let payload = serde_json::to_value(OllamaGenerateReq {
          model: llama_model,
          prompt: prompt,
          stream: true,
          max_tokens: Some(1024),
          tools: if tools_supported { Some(tool_schemas.clone()) } else { None },
          tool_choice: if tools_supported { Some("auto") } else { None }
        })?;
        //crate::log::log("debug", &format!("LLM payload: {}", payload));
        client.post(&url).json(&payload)
      }

      ApiKind::OllamaChat => {
        let messages = vec![
          ChatMessage { role: "system", content: "You are a helpful assistant." },
          ChatMessage { role: "user", content: prompt },
        ];
        let tools_supported = crate::llm::TOOLS_SUPPORTED.get().copied().unwrap_or(false);
        let payload = serde_json::to_value(OllamaChatReq {
          model: llama_model,
          messages: messages,
          stream: true,
          tools: if tools_supported { Some(tool_schemas.clone()) } else { None },
          tool_choice: if tools_supported { Some("auto") } else { None }
        })?;
        //crate::log::log("debug", &format!("LLM payload: {}", payload));
        client.post(&url).json(&payload)
      }

      ApiKind::LegacyCompletion => {
        let tools_supported = crate::llm::TOOLS_SUPPORTED.get().copied().unwrap_or(false);
        let payload = serde_json::to_value(LegacyCompletionReq {
          prompt: prompt,
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
          tools: if tools_supported { Some(tool_schemas.clone()) } else { None },
          tool_choice: if tools_supported { Some("auto") } else { None }
        })?;
        //crate::log::log("debug", &format!("LLM payload: {}", payload));
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
          if payload == "[DONE]" {
            // crate::log::log("info", &format!("Full payload: {}", full_payload.to_string()));
            return Ok(());
          }

          // parse JSON safely
          if let Ok(v) = serde_json::from_str::<serde_json::Value>(payload) {
            // Handle new Llama3.2 style: {"message":{"content":...}}
            if let Some(message) = v.get("message") {
              if let Some(content) = message.get("content").and_then(|c| c.as_str()) {
                if !content.is_empty() {
                  on_piece(&content);
                  full_payload.push_str(&content);
                }
              }
            } else {
              match kind {
                ApiKind::OaiChat | ApiKind::OllamaChat | ApiKind::OllamaGenerate => {
                  if let Some(choices) = v.get("choices").and_then(|c| c.as_array()) {
                    for choice in choices {
                      if let Some(delta) = choice.get("delta") {
                        if let Some(content) = delta.get("content").and_then(|c| c.as_str()) {
                          if !content.is_empty() {
                            on_piece(content);
                            full_payload.push_str(&content);
                          }
                        }
                      }
                      if choice.get("finish_reason").and_then(|r| r.as_str()) == Some("stop") {
                        // crate::log::log("info", &format!("Full payload: {}", full_payload.to_string()));
                        return Ok(());
                      }
                    }
                  }
                  if v.get("done").and_then(|x| x.as_bool()) == Some(true)
                    || v.get("status").and_then(|x| x.as_str()) == Some("completed")
                  {
                    // crate::log::log("info", &format!("Full payload: {}", full_payload.to_string()));
                    return Ok(());
                  }
                }
                ApiKind::LegacyCompletion => {
                  if let Some(content) = v.get("content").and_then(|c| c.as_str()) {
                    on_piece(content);
                    full_payload.push_str(content);
                  }
                  if v.get("stop").and_then(|s| s.as_bool()) == Some(true) {
                    // crate::log::log("info", &format!("Full payload: {}", full_payload.to_string()));
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


pub async fn supports_tool_calls(
    model: &str,
    llm_engine_type: &str,
    base_url: &str,
) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
    let client = reqwest::Client::new();

    match llm_engine_type {

        "ollama" => {
          // Use /api/show with POST
          let endpoint = format!("{}/api/show", base_url);
          let payload = serde_json::json!({ "name": model });

          let resp = client.post(&endpoint)
            .json(&payload)
            .send()
            .await?;

          if !resp.status().is_success() {
            return Ok(false);
          }

          let data: serde_json::Value = resp.json().await?;
          if let Some(capabilities) = data.get("capabilities").and_then(|v| v.as_array()) {
            // Check if "tools" is in capabilities
            return Ok(capabilities.iter().any(|c| c.as_str() == Some("tools")));
          }

          Ok(false)
        }

        "llama-server" => {
          // Minimal test: send a prompt asking for JSON tool call
          let payload = serde_json::json!({
              "stream": false,
              "messages": [
                { "role": "system", "content": "You are a helpful assistant." },
                { "role": "user", "content": "Respond ONLY with a JSON tool_call object for a calculator adding 1 + 1" }
              ],
              "max_tokens": 200,
              "tools": [
                {
                  "type": "function",
                  "function": {
                    "name": "calculator",
                    "description": "Perform a calculation",
                    "parameters": {
                      "type": "object",
                      "properties":{
                        "operation": {
                          "type": "string",
                          "description": "The operation to be calculated"
                        }
                      },
                      "required":["operation"]
                    }
                  }
                }
              ]
          });

          let endpoint = format!("{}/v1/chat/completions", base_url); // wrapper endpoint
          let resp = client.post(&endpoint).json(&payload).send().await?;
          let data: Value = resp.json().await?;

          // crate::log::log("info", &format!("data: {:?}", data));

          if let Some(choice) = data.get("choices").and_then(|c| c.get(0)) {
            if let Some(message) = choice.get("message") {
              if message.get("tool_calls")
                .and_then(|arr| arr.as_array())
                .map_or(false, |a| !a.is_empty())
              {
                return Ok(true);
              }
            }
          }

          Ok(false)
        }
        _ => Ok(false),
    }
}