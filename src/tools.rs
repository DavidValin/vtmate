// ------------------------------------------------------------------
//  Tool handling
// ------------------------------------------------------------------

use apply_patch::ApplyPatchTool;
use bash_command::BashCommandTool;
use glob::GlobTool;
use grep::GrepTool;
use http_request::HttpRequestTool;
use read_file::ReadFileTool;
use search::SearchTool;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::OnceLock;
use web_fetch::WebFetchTool;

// API
// ------------------------------------------------------------------

pub mod apply_patch;
pub mod bash_command;
pub mod glob;
pub mod grep;
pub mod http_request;
pub mod read_file;
pub mod search;
pub mod web_fetch;

pub trait Tool {
  fn name(&self) -> &str;
  fn handle(
    &self,
    tool_call_args: &Value,
  ) -> Result<String, Box<dyn std::error::Error + Send + Sync>>;
  fn json_schema(&self) -> Result<Value, Box<dyn std::error::Error + Send + Sync>>;
}

// Global cache for dynamically loaded HTTP request tools
static HTTP_REQUEST_TOOLS: OnceLock<Vec<HttpRequestTool>> = OnceLock::new();

fn load_http_tools() -> &'static Vec<HttpRequestTool> {
  HTTP_REQUEST_TOOLS.get_or_init(|| {
    let defs = http_request::load_http_request_definitions();
    defs.into_iter().map(HttpRequestTool::new).collect()
  })
}

/// Given a list of tool names, return their JSON schemas.
pub fn tools_schemas(
  tool_names: &[String],
) -> Result<Vec<Value>, Box<dyn std::error::Error + Send + Sync>> {
  let mut schemas = Vec::new();
  // Build a lookup map for dynamic HTTP request tools
  let http_tools = load_http_tools();
  let http_tool_map: HashMap<&str, &HttpRequestTool> = http_tools
    .iter()
    .map(|t: &HttpRequestTool| (t.name(), t))
    .collect();

  for name in tool_names {
    match name.as_str() {
      "web_fetch" => schemas.push(WebFetchTool::new().json_schema()?),
      "bash_command" => schemas.push(BashCommandTool::new().json_schema()?),
      "search" => schemas.push(SearchTool::new().json_schema()?),
      "apply_patch" => schemas.push(ApplyPatchTool::new().json_schema()?),
      "read_file" => schemas.push(ReadFileTool::new().json_schema()?),
      "glob" => schemas.push(GlobTool::new().json_schema()?),
      "grep" => schemas.push(GrepTool::new().json_schema()?),
      _ => {
        if let Some(tool) = http_tool_map.get(name.as_str()) {
          schemas.push(tool.json_schema()? as Value);
        } else {
          crate::log::log("warn", &format!("Unknown tool '{}' — skipping", name));
        }
      }
    }
  }

  crate::log::log(
    "debug",
    &format!(
      "tools_schemas: requested {:?}, returning {} schemas",
      tool_names,
      schemas.len()
    ),
  );
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
  let name = name.to_lowercase();
  let args = v.get("arguments").ok_or("Missing arguments")?;

  // Resolve schema: try static tools first, then dynamic
  let schema = match name.as_str() {
    "web_fetch" => WebFetchTool::new().json_schema()?,
    "bash_command" => BashCommandTool::new().json_schema()?,
    "search" => SearchTool::new().json_schema()?,
    "apply_patch" => ApplyPatchTool::new().json_schema()?,
    "read_file" => ReadFileTool::new().json_schema()?,
    "glob" => GlobTool::new().json_schema()?,
    "grep" => GrepTool::new().json_schema()?,
    _ => {
      let http_tools = load_http_tools();
      let tool = http_tools
        .iter()
        .find(|t: &&HttpRequestTool| t.name() == name)
        .ok_or_else(|| format!("Unknown tool: {}", name))?;
      tool.json_schema()?
    }
  };

  crate::log::log("debug", &format!("validating tool args"));
  validate_tool_call(args, &schema)?;

  match name.as_str() {
    "web_fetch" => WebFetchTool::new().handle(args),
    "bash_command" => BashCommandTool::new().handle(args),
    "search" => SearchTool::new().handle(args),
    "apply_patch" => ApplyPatchTool::new().handle(args),
    "read_file" => ReadFileTool::new().handle(args),
    "glob" => GlobTool::new().handle(args),
    "grep" => GrepTool::new().handle(args),
    _ => {
      let http_tools = load_http_tools();
      let tool = http_tools
        .iter()
        .find(|t: &&HttpRequestTool| t.name() == name)
        .ok_or_else(|| format!("Unknown tool: {}", name))?;
      tool.handle(args)
    }
  }
}
