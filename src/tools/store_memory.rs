// ------------------------------------------------------------------
//  Tool: Store memory
// ------------------------------------------------------------------

use super::Tool;
use crate::memory::{KnowledgeUnit, Memory, Predicate};
use crate::util;
use serde_json::Value;
use serde_json::json;
// use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

// API
// ------------------------------------------------------------------

pub struct StoreMemoryTool;

// PRIVATE
// ------------------------------------------------------------------

impl StoreMemoryTool {
  pub fn new() -> Self {
    StoreMemoryTool
  }
}

impl Tool for StoreMemoryTool {
  fn name(&self) -> &str {
    "store_memory"
  }

  fn handle(
    &self,
    tool_call_args: &Value,
  ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    // Extract required fields
    let subject = tool_call_args
      .get("subject")
      .and_then(|v| v.as_str())
      .ok_or("Missing or invalid 'subject'")?;
    let predicate = tool_call_args
      .get("predicate")
      .and_then(|v| v.as_str())
      .ok_or("Missing or invalid 'predicate'")?;
    let object = tool_call_args
      .get("object")
      .and_then(|v| v.as_str())
      .ok_or("Missing or invalid 'object'")?;

    // Optional fields
    let location = tool_call_args.get("location").and_then(|v| v.as_str());
    let timestamp = match tool_call_args.get("timestamp") {
      Some(v) => {
        let secs = v.as_i64().ok_or("Invalid 'timestamp'")?;
        Some(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(secs as u64))
      }
      None => None,
    };

    let ts = timestamp.unwrap_or_else(|| SystemTime::now());

    // Build knowledge unit
    let unit = KnowledgeUnit {
      subject: subject.to_string(),
      predicate: Predicate {
        name: predicate.to_string(),
        inverse: "".to_string(),
      },
      object: object.to_string(),
      location: location.map(|s| s.to_string()),
      timestamp: ts,
    };

    // Load or create memory
    let memory_path = crate::memory::ensure_memory_path();
    let path = memory_path.as_str();
    let mut memory = crate::memory::ensure_memory_file(path)?;

    // Store unit and persist
    memory.store(unit);
    memory.save_to_file(path)?;

    Ok(format!(
      "Stored memory: {} {} {}",
      subject, predicate, object
    ))
  }

  fn json_schema() -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
    Ok(json!({
      "type": "function",
      "function": {
        "name": "store_memory",
        "description": "Use this tool to store relevant memories from the latest user message that can be useful in the future. Use accurate Subject and Object and pick the right Predicate from the allowed values.",
        "parameters": {
          "type": "object",
          "properties": {
            "subject": {
              "type": "string",
              "description": "the person or thing performing the action."
            },
            "predicate": {
              "type": "string",
              "description": "the verb",
              "enum": [
                "believed",
                "assumed",
                "made",
                "saw",
                "said",
                "said to",
                "failed at",
                "wanted",
                "thought",
                "asked about",
                "planned",
                "requested",
                "ordered",
                "complained about",
                "ocurred at",
                "created",
                "met with",
                "destroyed",
                "modified",
                "examined",
                "inspected",
                "evaluated",
                "tested",
                "analyzed",
                "calculated",
                "estimated",
                "predicted",
                "performed",
                "executed",
                "completed",
                "succeeded",
                "confirmed",
                "approved",
                "denied",
                "received",
                "sent",
                "delivered",
                "communicated",
                "informed",
                "informed about",
                "questioned",
                "inquired",
                "participated in",
                "attended",
                "presented",
                "displayed",
                "demonstrated"
              ]
            },
            "object": {
              "type": "string",
              "description": "the person or thing that receives the action of the verb"
            },
            "location": {
              "type": "string",
              "description": "optional location if its specified or inferred from the message"
            },
            "timestamp": {
              "type": "integer",
              "description": "optional timestamp if its specified or inferred from the message"
            }
          },
          "required": [
            "subject",
            "predicate",
            "object"
          ]
        }
      }
    }))
  }
}
