// ------------------------------------------------------------------
//  Tool handling
// ------------------------------------------------------------------

use bash_command::BashCommandTool;
use serde_json::{Value, json};
use web_fetch::WebFetchTool;

// API
// ------------------------------------------------------------------

pub mod bash_command;
pub mod web_fetch;

pub trait Tool {
  fn name(&self) -> &str;
  fn handle(
    &self,
    tool_call_args: &Value,
  ) -> Result<String, Box<dyn std::error::Error + Send + Sync>>;
  fn json_schema() -> Result<Value, Box<dyn std::error::Error + Send + Sync>>;
}

pub fn get_available_tools() -> Result<Vec<Value>, Box<dyn std::error::Error + Send + Sync>> {
  Ok(vec![
    WebFetchTool::json_schema()?,
    BashCommandTool::json_schema()?,
  ])
}

/// Given a list of tool names, return their JSON schemas.
pub fn tools_schemas(
  tool_names: &[String],
) -> Result<Vec<Value>, Box<dyn std::error::Error + Send + Sync>> {
  let mut schemas = Vec::new();
  for name in tool_names {
    match name.as_str() {
      "web_fetch" => schemas.push(WebFetchTool::json_schema()?),
      "bash_command" => schemas.push(BashCommandTool::json_schema()?),
      _ => return Err(format!("Unknown tool: {}", name).into()),
    }
  }
  Ok(schemas)
}

pub fn validate_tool_call(
  tool_call_args: &Value,
  schema: &Value,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
  // Locate parameters section: function.parameters
  let schema_params = schema.get("parameters");
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

fn format_failure(reasons: Vec<String>) -> String {
  json!({ "status": "failed", "reasons": reasons }).to_string()
}

pub fn handle_tool_call(
  call_json: &str,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
  crate::log::log("debug", &format!("handling tool: {}", call_json));

  let result = try_handle_tool_call(call_json);

  match result {
    Ok(body) => Ok(body),
    Err(e) => {
      let msg = e.to_string();
      crate::log::log("error", &format!("tool call failed: {}", msg));
      Ok(format_failure(vec![msg]))
    }
  }
}

fn try_handle_tool_call(
  call_json: &str,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
  let v: Value = serde_json::from_str(call_json)?;
  let name = v
    .get("name")
    .and_then(|x| x.as_str())
    .ok_or("Missing tool name")?;
  let args = v.get("arguments").ok_or("Missing arguments")?;

  let schema = match name {
    "web_fetch" => WebFetchTool::json_schema()?,
    "bash_command" => BashCommandTool::json_schema()?,
    _ => return Err(format!("Unknown tool: {}", name).into()),
  };

  crate::log::log("debug", &format!("validating tool args"));
  // validate the tool call args against the schema
  validate_tool_call(args, &schema)?;

  match name {
    "web_fetch" => WebFetchTool::new().handle(args),
    "bash_command" => BashCommandTool::new().handle(args),
    _ => unreachable!(),
  }
}
