use serde_json::Value;

// Minimal Tool trait required by the included source modules
pub trait Tool {
  fn name(&self) -> &str;
  fn handle(
    &self,
    tool_call_args: &Value,
  ) -> Result<String, Box<dyn std::error::Error + Send + Sync>>;
  fn json_schema(&self) -> Result<Value, Box<dyn std::error::Error + Send + Sync>>;
}

#[path = "../../src/tools/glob.rs"]
mod glob;

use self::glob::GlobTool;
use serde_json::json;
use std::io::Write;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

static TEST_COUNTER: AtomicUsize = AtomicUsize::new(0);

/// Create a temporary directory with test files and return its path.
/// Files are created with staggered modification times so sorting is deterministic.
fn make_test_files() -> String {
  let dir = std::env::temp_dir();
  let unique = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
  let test_dir = dir.join(format!(
    "glob_test_{}_{}",
    std::process::id(),
    unique
  ));
  std::fs::create_dir_all(&test_dir).unwrap();

  // Create a subdirectory
  let sub = test_dir.join("sub");
  std::fs::create_dir_all(&sub).unwrap();

  // Create files with different extensions
  let js1 = test_dir.join("app.js");
  let mut f = std::fs::File::create(&js1).unwrap();
  writeln!(f, "console.log('first');").unwrap();
  drop(f);

  std::thread::sleep(Duration::from_millis(10));

  let ts1 = test_dir.join("index.ts");
  let mut f = std::fs::File::create(&ts1).unwrap();
  writeln!(f, "const x: number = 1;").unwrap();
  drop(f);

  std::thread::sleep(Duration::from_millis(10));

  let js2 = sub.join("util.js");
  let mut f = std::fs::File::create(&js2).unwrap();
  writeln!(f, "export function util() {{}}").unwrap();
  drop(f);

  test_dir.to_string_lossy().to_string()
}

#[test]
fn test_glob_finds_js_files() {
  let root = make_test_files();
  let tool = GlobTool::new();
  let args = json!({"pattern": "**/*.js", "root": root});
  let result = tool.handle(&args).unwrap();
  assert!(result.contains("app.js"));
  assert!(result.contains("util.js"));
  assert!(!result.contains("index.ts"));
}

#[test]
fn test_glob_finds_ts_files() {
  let root = make_test_files();
  let tool = GlobTool::new();
  let args = json!({"pattern": "*.ts", "root": root});
  let result = tool.handle(&args).unwrap();
  assert!(result.contains("index.ts"));
  assert!(!result.contains("app.js"));
}

#[test]
fn test_glob_no_matches() {
  let root = make_test_files();
  let tool = GlobTool::new();
  let args = json!({"pattern": "*.xyz", "root": root});
  let result = tool.handle(&args).unwrap();
  assert!(result.contains("No files matched"));
}

#[test]
fn test_glob_missing_pattern() {
  let tool = GlobTool::new();
  let args = json!({});
  let result = tool.handle(&args).unwrap_err();
  assert!(result.to_string().contains("Missing 'pattern'"));
}

#[test]
fn test_glob_sorted_by_mtime() {
  let root = make_test_files();
  let tool = GlobTool::new();
  let args = json!({"pattern": "**/*.js", "root": root});
  let result = tool.handle(&args).unwrap();

  // util.js was created last, so it should appear before app.js
  let util_pos = result.find("util.js").unwrap();
  let app_pos = result.find("app.js").unwrap();
  assert!(
    util_pos < app_pos,
    "util.js (newer) should appear before app.js (older)"
  );
}
