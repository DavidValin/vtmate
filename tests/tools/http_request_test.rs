use serde_json::Value;

mod log {
  pub fn log(_level: &str, _msg: &str) {}
}

mod util {
  pub fn get_user_home_path() -> Option<std::path::PathBuf> {
    Some(std::path::PathBuf::from("/tmp"))
  }
}

// Minimal Tool trait required by the included source module
pub trait Tool {
  fn name(&self) -> &str;
  fn handle(
    &self,
    tool_call_args: &Value,
  ) -> Result<String, Box<dyn std::error::Error + Send + Sync>>;
  fn json_schema(&self) -> Result<Value, Box<dyn std::error::Error + Send + Sync>>;
}

// Mirrors `crate::tools::reserved_tool_names()`, which http_request.rs's
// loader checks a definition's name against.
mod tools {
  pub fn reserved_tool_names() -> Vec<&'static str> {
    vec![
      "web_fetch",
      "bash_command",
      "glob",
      "grep",
      "read_file",
      "search",
      "apply_patch",
      "to-do",
      "todo_define",
      "todo_update",
      "todo_list",
      "todo_get",
      "todo_get_item",
      "todo_set_status",
      "todo_delete",
    ]
  }
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

#[test]
fn test_load_http_request_definitions_skips_reserved_tool_names() {
  let dir = std::path::PathBuf::from("/tmp/.vtmate/tools/http_requests");
  std::fs::create_dir_all(&dir).unwrap();

  let def_json = |name: &str| {
    format!(
      r#"{{
        "tool_definition": {{ "name": "{}", "description": "d", "parameters": {{}} }},
        "tool_http_handler": {{ "method": "GET", "url": "https://example.com" }}
      }}"#,
      name
    )
  };

  // Collides with a built-in tool name (a static one, and one of the todo_*
  // set) - both must be rejected, never shadowing the real tool.
  let reserved_path = dir.join("reserved_collision_test_bash_command.json");
  std::fs::write(&reserved_path, def_json("bash_command")).unwrap();
  let reserved_todo_path = dir.join("reserved_collision_test_todo_get.json");
  std::fs::write(&reserved_todo_path, def_json("todo_get")).unwrap();

  let allowed_name = format!("custom_tool_{}", std::process::id());
  let allowed_path = dir.join(format!("{}.json", allowed_name));
  std::fs::write(&allowed_path, def_json(&allowed_name)).unwrap();

  let defs = http_request::load_http_request_definitions();

  let _ = std::fs::remove_file(&reserved_path);
  let _ = std::fs::remove_file(&reserved_todo_path);
  let _ = std::fs::remove_file(&allowed_path);

  assert!(
    !defs.iter().any(|d| d.tool_definition.name == "bash_command"),
    "a built-in static tool name must not be loadable as a custom HTTP tool"
  );
  assert!(
    !defs.iter().any(|d| d.tool_definition.name == "todo_get"),
    "a built-in todo_* tool name must not be loadable as a custom HTTP tool"
  );
  assert!(
    defs.iter().any(|d| d.tool_definition.name == allowed_name),
    "a non-reserved custom tool should still load"
  );
}
