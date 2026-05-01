// ------------------------------------------------------------------
//  Tool: Web Fetch
// ------------------------------------------------------------------

use super::Tool;
use serde_json::Value;
use serde_json::json;

// API
// ------------------------------------------------------------------

pub struct WebFetchTool;

// PRIVATE
// ------------------------------------------------------------------

impl WebFetchTool {
  pub fn new() -> Self {
    WebFetchTool
  }
}

impl Tool for WebFetchTool {
  fn name(&self) -> &str {
    "web_fetch"
  }

  fn handle(
    &self,
    tool_call_args: &Value,
  ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let url = tool_call_args.get("url").and_then(|v| v.as_str()).unwrap();
    let _content_type = tool_call_args
      .get("content_type")
      .and_then(|v| v.as_str())
      .unwrap();

    let client = reqwest::blocking::Client::new();
    let response = client.get(url).send()?;

    if !response.status().is_success() {
      return Err(
        format!(
          "HTTP {} - URL '{}' returned status {}",
          response.status(),
          url,
          response.status()
        )
        .into(),
      );
    }

    let body = response.text()?;
    Ok(body)
  }

  fn json_schema() -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
    Ok(json!({
      "type": "function",
      "function": {
        "name": "web_fetch",
        "description": "Fetches a single html page",
        "parameters": {
          "type": "object",
          "properties": {
            "url": {
              "type": "string"
            },
            "content_type": {
              "type": "string",
              "enum": ["text", "html"]
            }
          },
          "required": [
            "url",
            "content_type"
          ]
        }
      }
    }))
  }
}
