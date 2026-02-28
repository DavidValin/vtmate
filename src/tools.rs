// ------------------------------------------------------------------
//  Tool handling
// ------------------------------------------------------------------

use remember::RememberTool;
use serde_json::{Value, json};
use store_memory::StoreMemoryTool;

// API
// ------------------------------------------------------------------


pub mod remember;
pub mod store_memory;

pub trait Tool {
  fn name(&self) -> &str;
  fn handle(&self, tool_call_args: &Value) -> Result<String, Box<dyn std::error::Error + Send + Sync>>;
  fn json_schema() -> Result<Value, Box<dyn std::error::Error + Send + Sync>>;
}

pub fn get_available_tools() -> Result<Vec<Value>, Box<dyn std::error::Error + Send + Sync>> {
  let remember_schema = RememberTool::json_schema()?;
  let store_schema = StoreMemoryTool::json_schema()?;
  Ok(vec![
    json!({
        "type": "function",
        "function": {
            "name": "remember",
            "description": "",
            "parameters": remember_schema
        }
    }),
    json!({
        "type": "function",
        "function": {
            "name": "store_memory",
            "description": "",
            "parameters": store_schema
        }
    }),
  ])
}

pub fn validate_tool_call(
  tool_call_args: &Value,
  schema: &Value,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
  // Locate parameters section: function.parameters
  let schema_params = schema.get("function").and_then(|f| f.get("parameters"));
  if let Some(schema_params) = schema_params {
    // required fields
    if let Some(required) = schema_params.get("required").and_then(|v| v.as_array()) {
      for key in required {
        if let Some(k) = key.as_str() {
          if !tool_call_args.get(k).is_some() {
            return Err(format!("Missing required field: {}", k).into());
          }
        }
      }
    }
    // properties validation
    if let Some(schema_props) = schema_params.get("properties").and_then(|v| v.as_object()) {
      for (name, schema_prop_def) in schema_props {
        if let Some(arg_val) = tool_call_args.get(name) {
          if let Some(type_str) = schema_prop_def.get("type").and_then(|v| v.as_str()) {
            let type_ok = match type_str {
              "string" => arg_val.is_string(),
              "number" => arg_val.is_number(),
              "integer" => arg_val.is_i64() || arg_val.is_u64() || arg_val.is_f64(),
              "boolean" => arg_val.is_boolean(),
              "array" => arg_val.is_array(),
              "object" => arg_val.is_object(),
              _ => true,
            };
            if !type_ok {
              return Err(
                format!(
                  "Field '{}' has incorrect type. Expected {}, got {}",
                  name, type_str, arg_val
                )
                .into(),
              );
            }
          }
        }
      }
    }
  }
  Ok(())
}

pub fn handle_tool_call(
  call_json: &str,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
  let v: Value = serde_json::from_str(call_json)?;
  let name = v
    .get("name")
    .and_then(|x| x.as_str())
    .ok_or("Missing tool name")?;
  let args = v.get("arguments").ok_or("Missing arguments")?;

  let schema = match name {
    "store_memory" => StoreMemoryTool::json_schema()?,
    "remember" => RememberTool::json_schema()?,
    _ => return Err(format!("Unknown tool: {}", name).into()),
  };
  validate_tool_call(args, &schema)?;
  match name {
    "store_memory" => StoreMemoryTool::new().handle(args),
    "remember" => RememberTool::new().handle(args),
    _ => unreachable!(),
  }
}

// Helper to detect and run tool calls from a chunk string
pub fn handle_tool_call_from_json(chunk: &str) -> Option<String> {
  let v: Value = serde_json::from_str(chunk).ok()?;
  let choices = v.get("choices")?.as_array()?;
  if choices.is_empty() {
    return None;
  }
  let message = &choices[0]["message"];
  let tool_calls = message.get("tool_calls")?.as_array()?;
  let mut outputs = Vec::new();
  for call in tool_calls {
    let name = call.get("name")?.as_str()?;
    let arguments: &str = call.get("arguments")?.as_str()?;
    let payload = format!(r#"{{\"name\":\"{}\",\"arguments\":{}}}"#, name, arguments);

    crate::log::log("debug", name);
    crate::log::log("debug", arguments);

    match handle_tool_call(&payload) {
      Ok(out) => outputs.push(out),
      Err(e) => outputs.push(format!("Error: {}", e)),
    }
  }
  Some(outputs.join("\n"))
}
