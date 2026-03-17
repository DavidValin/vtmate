// ------------------------------------------------------------------
//  LLM handling
// ------------------------------------------------------------------

use std::sync::{Arc, atomic::AtomicU64, OnceLock};
use crossbeam_channel::Receiver;
use reqwest::StatusCode;
use futures_util::StreamExt;
use bytes::Bytes;
use serde_json::{json, Value};
use serde::{Deserialize, Serialize};
use crate::tools::store_memory::StoreMemoryTool;
use crate::tools::Tool;
use crate::tools::get_available_tools;
use crate::tools::handle_tool_call;
use std::collections::HashMap;
use crate::tools::remember::RememberTool;

pub static TOOLS_SUPPORTED: OnceLock<bool> = OnceLock::new();

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChatMessage {
  pub role: String,
  pub content: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct ChatMessageRef<'a> {
  role: &'a str,
  content: &'a str,
}

/// Stream response from Llama/Ollama endpoints, fallback if one fails, and mid-stream cancellation support
pub async fn llama_server_stream_response_into(
  conversation_history: &[ChatMessage],
  full_history: bool,
  include_tools: bool,
  user_prompt: &str,
  llama_host: &str,
  llama_model: &str,
  server_type: &str,
  stop_all_rx: &Receiver<()>,
  interrupt_counter: Arc<AtomicU64>,
  expected_interrupt: u64,
  on_piece: &mut dyn FnMut(&str)
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {

  let mut full_payload = String::new();
  let mut partial_tool_calls_buf: HashMap<String, String> = HashMap::new();
  let mut name_map: HashMap<String, String> = HashMap::new();
  let mut last_key: Option<String> = None;

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
  let tool_schemas = get_available_tools()?;

  for (url, kind) in tries {
    if stop_all_rx.try_recv().is_ok() ||
      interrupt_counter.load(std::sync::atomic::Ordering::SeqCst) != expected_interrupt {
        return Ok(());
    }

    crate::log::log("info", &format!("Trying endpoint: {}", url));

    let new_user_prompt = ChatMessageRef {
      role: "user",
      content: user_prompt,
    };

    // contains the full prompt (with previous history)
    let mut full_messages_vec: Vec<ChatMessageRef> = Vec::new();

    // prepare messages
    full_messages_vec.push(ChatMessageRef {
      role: "system",
      content: "You are a helpful assistant.",
    });

    // include the full chat history in the prompt
    if full_history {
      for m in conversation_history.iter() {
        full_messages_vec.push(ChatMessageRef {
          role: m.role.as_str(),
          content: &m.content,
        });
      }
    }

    // add latest message at the end of history
    full_messages_vec.push(ChatMessageRef {
      role: "user",
      content: user_prompt,
    });

    let messages = full_messages_vec;
    let tools_supported = crate::llm::TOOLS_SUPPORTED.get().copied().unwrap_or(false);

    let req = match kind {
      ApiKind::OaiChat => {
        let payload = json!({
          "model": llama_model,
          "messages": messages,
          "stream": true,
          "tools": if include_tools && tools_supported {
            Some(vec![StoreMemoryTool::json_schema()?])
          } else {
            None
          },
          "tool_choice": if include_tools && tools_supported {
            Some("auto")
          } else {
            None
          },
          "parallel_tool_calls": if include_tools && tools_supported {
            Some(true)
          } else {
            None
          },
        });
        client.post(&url).json(&payload)
      }

      ApiKind::OllamaGenerate => {
        let payload = json!({
          "model": llama_model,
          "prompt": user_prompt,
          "stream": true,
          "max_tokens": 1024,
          "tools": if include_tools && tools_supported {
            Some(vec![StoreMemoryTool::json_schema()?])
          } else {
            None
          },
          "tool_choice": if include_tools && tools_supported {
            Some("auto")
          } else {
            None
          },
          "parallel_tool_calls": if include_tools && tools_supported {
            Some(true)
          } else {
            None
          }
        });
        client.post(&url).json(&payload)
      }

      ApiKind::OllamaChat => {
        let payload = json!({
          "model": llama_model,
          "messages": messages,
          "stream": true,
          "tools": if include_tools && tools_supported {
            Some(vec![StoreMemoryTool::json_schema()?])
          } else {
            None
          },
          "tool_choice": if include_tools && tools_supported {
            Some("auto")
          } else {
            None
          },
          "parallel_tool_calls": if include_tools && tools_supported {
            Some(true)
          } else {
            None
          }
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
      if should_fallback_status(status) {
        continue;
      } else {
        return Err(last_err.clone().unwrap().into());
      }
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
                  on_piece(content);
                  full_payload.push_str(&content);
                }
              }

              if let Some(tool_calls_value) = message.get("tool_calls") {
                if tools_supported {
                  match tool_calls_value {
                    serde_json::Value::Array(arr) => {
                      process_tool_calls_array(
                        arr,
                        &mut partial_tool_calls_buf,
                        &mut name_map,
                        &mut last_key,
                      );
                    }
                    _ => {}
                  }
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
                          if let Some(tool_calls_value) = delta.get("tool_calls") {
                            if tools_supported {
                              // tool_calls can be a string or an array of objects. Handle both.
                              match tool_calls_value {
                                serde_json::Value::Array(arr) => {
                                  process_tool_calls_array(
                                    arr,
                                    &mut partial_tool_calls_buf,
                                    &mut name_map,
                                    &mut last_key,
                                  );
                                }
                                _ => {}
                              }
                            }
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

      let resp = client.post(&endpoint).json(&payload).send().await?;

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
            { "role": "user", "content": "Calculate 1 + 1 using the available tools" }
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

      if let Some(choice) = data.get("choices").and_then(|c: &serde_json::Value| c.get(0)) {
        if let Some(message) = choice.get("message") {
          if message
            .get("tool_calls")
            .and_then(|arr: &serde_json::Value| arr.as_array())
            .map_or(false, |a: &Vec<serde_json::Value>| !a.is_empty())
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


// processes an array of tool calls, accumulating arguments across chunks
fn process_tool_calls_array(
  arr: &Vec<serde_json::Value>,
  args_map: &mut HashMap<String, String>,
  name_map: &mut HashMap<String, String>,
  last_key: &mut Option<String>,
) {
  for tc in arr {
    let key = if let Some(id) = tc.get("id").and_then(|v| v.as_str()) {
      id.to_string()
    } else {
      last_key.clone().unwrap_or("__no_id__".to_string())
    };
    *last_key = Some(key.clone());
    if let Some(func_obj) = tc.get("function") {
      // extract the tool name from the function object
      let name = func_obj
        .get("name")
        .and_then(|n| n.as_str())
        .unwrap_or("unknown");
      // store name if not already
      name_map
        .entry(key.clone())
        .or_insert_with(|| name.to_string());
      if let Some(args_val) = func_obj.get("arguments") {
        // buffers partial string arguments
        // validates them as JSON and executes the tool call once a complete arguments payload is available
        match args_val {
          serde_json::Value::String(s) => {
            let buf = args_map.entry(key.clone()).or_insert_with(String::new);
            buf.push_str(s);
            if serde_json::from_str::<serde_json::Value>(buf).is_ok() {
              let stored_name = name_map.get(&key).map(|s: &String| s.as_str()).unwrap_or("unknown");
              *buf = serde_json::to_string(
                &serde_json::from_str::<serde_json::Value>(buf).unwrap_or(serde_json::Value::Null),
              )
              .unwrap_or("{}".to_string());
              let payload = format!(r#"{{"name":"{}","arguments":{}}}"#, stored_name, buf);
              let _ = handle_tool_call(&payload);
              buf.clear();
            }
          }
          _ => {
            let args_str = serde_json::to_string(args_val).unwrap_or("{}".to_string());
            let stored_name = name_map.get(&key).map(|s: &String| s.as_str()).unwrap_or("unknown");
            let payload = format!(r#"{{"name":"{}","arguments":{}}}"#, stored_name, args_str);
            let _ = handle_tool_call(&payload);
          }
        }
      }
    }
  }
}