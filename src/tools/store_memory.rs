use serde_json::json;
use serde_json::Value;

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use super::Tool;
use crate::memory::{KnowledgeUnit, Memory, Predicate};

pub struct StoreMemoryTool;

impl StoreMemoryTool {
  pub fn new() -> Self {
    StoreMemoryTool
  }
}

impl Tool for StoreMemoryTool {
  fn name(&self) -> &str {
    "store_memory"
  }

  fn handle(&self, args: &Value) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    // Extract required fields
    let subject = args
      .get("subject")
      .and_then(|v| v.as_str())
      .ok_or("Missing or invalid 'subject'")?;
    let predicate = args
      .get("predicate")
      .and_then(|v| v.as_str())
      .ok_or("Missing or invalid 'predicate'")?;
    let object = args
      .get("object")
      .and_then(|v| v.as_str())
      .ok_or("Missing or invalid 'object'")?;

    // Optional fields
    let location = args.get("location").and_then(|v| v.as_str());
    let timestamp = match args.get("timestamp") {
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
    let path = "memory.json";
    let mut memory = if Path::new(path).exists() {
      Memory::load_from_file(path)?
    } else {
      Memory::new(1000)
    };

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
        "description": "Stores a memory in form of 'Subject -> Predicate -> Object' at an optional physical location in a specific time.",
        "parameters": {
          "type": "object",
          "properties": {
            "subject": {
              "type": "string"
            },
            "predicate": {
              "type": "string",
              "enum": [
                "believed",
                "assumed",
                "made",
                "saw",
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
              "type": "string"
            },
            "location": {
              "type": "string"
            },
            "timestamp": {
              "type": "integer"
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
