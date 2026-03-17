// ------------------------------------------------------------------
//  Tool: Remember
// ------------------------------------------------------------------

use super::Tool;
use crate::memory::Memory;
use crate::util;
use serde_json::Value;
use serde_json::json;
// use std::path::Path;

// API
// ------------------------------------------------------------------

pub struct RememberTool;

// PRIVATE
// ------------------------------------------------------------------

impl RememberTool {
  pub fn new() -> Self {
    RememberTool
  }
}

impl Tool for RememberTool {
  fn name(&self) -> &str {
    "remember"
  }

  fn handle(
    &self,
    tool_call_args: &Value,
  ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    // Validation handled in tools.rs
    // Extract query string
    let query = tool_call_args
      .get("query")
      .and_then(|v| v.as_str())
      .ok_or("Missing or invalid 'query'")?;

    // Load or create memory
    let memory_path = crate::memory::ensure_memory_path();
    let path = memory_path.as_str();
    let memory = crate::memory::ensure_memory_file(path)?;

    // Perform query
    let top_k = 5;
    let ef_search = 50;
    let retrieved_units = memory.query(query, top_k, ef_search);

    // Build context string
    let context_text = Memory::build_context_from_units(&retrieved_units);

    Ok(context_text)
  }

  fn json_schema() -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
    Ok(json!({
      "type": "function",
      "function": {
        "name": "remember",
        "description": "Retrieves relevant memories based on a query",
        "parameters": {
          "type": "object",
          "properties": {
            "query": {
              "type": "string"
            }
          },
          "required": [
            "query"
          ]
        }
      }
    }))
  }
}
