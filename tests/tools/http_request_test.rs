use serde_json::Value;

// Minimal Tool trait required by the included source module
pub trait Tool {
  fn name(&self) -> &str;
  fn handle(
    &self,
    tool_call_args: &Value,
  ) -> Result<String, Box<dyn std::error::Error + Send + Sync>>;
  fn json_schema(&self) -> Result<Value, Box<dyn std::error::Error + Send + Sync>>;
}

#[path = "../../src/tools/http_request.rs"]
mod http_request;

use self::http_request::HttpRequestTool;
use serde_json::json;

#[test]
fn test_resolve_template_substitutes_single_placeholder() {
  let args = json!({"id": "42"});
  let out = HttpRequestTool::resolve_template("https://api.example.com/items/PICK_FROM['id']", &args);
  assert_eq!(out, "https://api.example.com/items/42");
}

#[test]
fn test_resolve_template_substitutes_multiple_placeholders() {
  let args = json!({"id": "42", "name": "widget"});
  let out = HttpRequestTool::resolve_template(
    "prefix-PICK_FROM['id']-PICK_FROM['name']-suffix",
    &args,
  );
  assert_eq!(out, "prefix-42-widget-suffix");
}

#[test]
fn test_resolve_template_no_placeholders_unchanged() {
  let args = json!({});
  let out = HttpRequestTool::resolve_template("no placeholders here", &args);
  assert_eq!(out, "no placeholders here");
}

#[test]
fn test_resolve_template_missing_key_drops_placeholder() {
  let args = json!({});
  let out = HttpRequestTool::resolve_template("value=PICK_FROM['missing']", &args);
  assert_eq!(out, "value=");
}

#[test]
fn test_resolve_template_unterminated_marker_left_literal() {
  let args = json!({"id": "42"});
  let out = HttpRequestTool::resolve_template("dangling PICK_FROM['id", &args);
  assert_eq!(out, "dangling PICK_FROM['id");
}

#[test]
fn test_resolve_template_non_string_value() {
  let args = json!({"count": 7});
  let out = HttpRequestTool::resolve_template("total=PICK_FROM['count']", &args);
  assert_eq!(out, "total=7");
}
