// ------------------------------------------------------------------
//  Tool: HTTP Request
//  Dynamically loads HTTP request definitions from
//  ~/.vtmate/tools/http_requests/*.json and registers each as a tool.
//
//  Each definition has two parts:
//    tool_definition       — JSON schema for the LLM tool call
//    tool_http_handler     — translates the call into an HTTP request
//
//  Values in the handler can reference call arguments via PICK_FROM['key']
// ------------------------------------------------------------------

use super::Tool;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::fs;

// API
// ------------------------------------------------------------------

/// Top-level JSON definition for an HTTP request tool
#[derive(Debug, Deserialize, Clone)]
pub struct HttpRequestDefinition {
  pub tool_definition: ToolDefinition,
  pub tool_http_handler: HttpHandler,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ToolDefinition {
  pub name: String,
  pub description: String,
  pub parameters: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct HttpHandler {
  #[serde(default = "default_method")]
  pub method: String,
  pub url: String,
  #[serde(default)]
  pub headers: HashMap<String, String>,
  #[serde(default)]
  pub body: Value,
}

fn default_method() -> String {
  "GET".to_string()
}

pub struct HttpRequestTool {
  pub definition: HttpRequestDefinition,
}

impl HttpRequestTool {
  pub fn new(definition: HttpRequestDefinition) -> Self {
    HttpRequestTool { definition }
  }

  /// Replace PICK_FROM['key'] patterns with the corresponding argument value
  fn resolve_template(template: &str, args: &Value) -> String {
    let mut result = String::new();
    let mut chars = template.char_indices().peekable();
    let mut buf = String::new();

    while let Some((_, ch)) = chars.next() {
      if ch == 'P' {
        // Check if this starts a PICK_FROM expression
        let start = buf.len();
        buf.push('P');
        let mut rest = String::new();
        rest.push('P');
        while let Some(&(_, c)) = chars.peek() {
          if c == '\'' || c == ']' {
            break;
          }
          let (_, c2) = chars.next().unwrap();
          rest.push(c2);
        }
        if rest == "PICK_FROM['" {
          // Read until the closing ]
          let mut key = String::new();
          while let Some(&(_, _c)) = chars.peek() {
            let (_, c2) = chars.next().unwrap();
            if c2 == ']' {
              break;
            }
            key.push(c2);
          }
          // Replace with argument value
          if let Some(val) = args.get(key.trim()) {
            match val {
              Value::String(s) => buf.push_str(s),
              _ => buf.push_str(&val.to_string()),
            }
          }
          buf.truncate(start);
          result.push_str(&buf);
          buf.clear();
        }
      }
      buf.push(ch);
    }
    result.push_str(&buf);
    result
  }
}

impl Tool for HttpRequestTool {
  fn name(&self) -> &str {
    &self.definition.tool_definition.name
  }

  fn handle(
    &self,
    tool_call_args: &Value,
  ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let def = &self.definition;
    let handler = &def.tool_http_handler;

    // Resolve URL template with PICK_FROM references
    let url = Self::resolve_template(&handler.url, tool_call_args);

    // Build headers with PICK_FROM references
    let mut headers_map = reqwest::header::HeaderMap::new();
    for (key, value) in &handler.headers {
      let resolved_key = Self::resolve_template(key, tool_call_args);
      let resolved_value = Self::resolve_template(value, tool_call_args);
      headers_map.insert(
        reqwest::header::HeaderName::from_bytes(resolved_key.as_bytes())
          .map_err(|e| format!("Invalid header name '{}': {}", key, e))?,
        reqwest::header::HeaderValue::from_str(&resolved_value)
          .map_err(|e| format!("Invalid header value '{}': {}", value, e))?,
      );
    }

    // Resolve body template with PICK_FROM references
    let body = Self::resolve_body(&handler.body, tool_call_args);

    let client = reqwest::blocking::Client::builder()
      .default_headers(headers_map)
      .build()?;

    let method = handler.method.to_uppercase();

    let response = match method.as_str() {
      "GET" => client.get(&url).send()?,
      "POST" => client.post(&url).json(&body).send()?,
      "PUT" => client.put(&url).json(&body).send()?,
      "PATCH" => client.patch(&url).json(&body).send()?,
      "DELETE" => client.delete(&url).send()?,
      _ => return Err(format!("Unsupported method: {}", method).into()),
    };

    if !response.status().is_success() {
      return Ok(
        json!({
          "status": "failed",
          "reasons": [format!("HTTP {} - {}", response.status(), url)]
        })
        .to_string(),
      );
    }

    let text = response.text()?;

    // Try to parse response as JSON, fall back to wrapping in a response field
    match serde_json::from_str::<Value>(&text) {
      Ok(json_val) => Ok(json_val.to_string()),
      Err(_) => Ok(json!({"response": text}).to_string()),
    }
  }

  fn json_schema(&self) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
    let tool_def = &self.definition.tool_definition;

    Ok(json!({
      "type": "function",
      "function": {
        "name": tool_def.name,
        "description": tool_def.description,
        "parameters": {
          "type": "object",
          "properties": tool_def.parameters,
          "required": tool_def.parameters
            .iter()
            .filter_map(|(k, v)| {
                if v.get("default").is_some() { None } else { Some(k.clone()) }
            })
            .collect::<Vec<String>>()
        }
      }
    }))
  }
}

impl HttpRequestTool {
  /// Recursively walk a Value and replace PICK_FROM['key'] in strings
  fn resolve_body(v: &Value, args: &Value) -> Value {
    match v {
      Value::String(s) => {
        let resolved = Self::resolve_template(s, args);
        // Try to parse as JSON if the result looks like a JSON value
        match serde_json::from_str::<Value>(&resolved) {
          Ok(parsed) => parsed,
          Err(_) => Value::String(resolved),
        }
      }
      Value::Array(arr) => {
        let resolved: Vec<Value> = arr.iter().map(|v| Self::resolve_body(v, args)).collect();
        Value::Array(resolved)
      }
      Value::Object(obj) => {
        let mut resolved = serde_json::Map::new();
        for (key, val) in obj {
          let resolved_key = Self::resolve_template(key, args);
          resolved.insert(resolved_key, Self::resolve_body(val, args));
        }
        Value::Object(resolved)
      }
      _ => v.clone(),
    }
  }
}

/// Loads all HTTP request definitions from ~/.vtmate/tools/http_requests/*.json
pub fn load_http_request_definitions() -> Vec<HttpRequestDefinition> {
  let home = crate::util::get_user_home_path();
  let dir = match home {
    Some(ref h) => h.join(".vtmate").join("tools").join("http_requests"),
    None => {
      crate::log::log(
        "warn",
        "No home directory found, skipping HTTP request tools",
      );
      return Vec::new();
    }
  };

  if !dir.exists() {
    return Vec::new();
  }

  let mut definitions = Vec::new();
  if let Ok(entries) = fs::read_dir(&dir) {
    for entry in entries.flatten() {
      let path = entry.path();
      if path.extension().and_then(|e| e.to_str()) == Some("json") {
        match fs::read_to_string(&path) {
          Ok(content) => match serde_json::from_str::<HttpRequestDefinition>(&content) {
            Ok(def) => {
              crate::log::log(
                "info",
                &format!("Loaded HTTP request tool: {}", def.tool_definition.name),
              );
              definitions.push(def);
            }
            Err(e) => {
              crate::log::log(
                "error",
                &format!("Failed to parse {}: {}", path.display(), e),
              );
            }
          },
          Err(e) => {
            crate::log::log(
              "error",
              &format!("Failed to read {}: {}", path.display(), e),
            );
          }
        }
      }
    }
  }
  definitions
}
