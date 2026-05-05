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

#[path = "../../src/tools/grep.rs"]
mod grep;

use self::grep::GrepTool;
use serde_json::json;
use std::io::Write;
use std::sync::atomic::{AtomicUsize, Ordering};

static TEST_COUNTER: AtomicUsize = AtomicUsize::new(0);

fn make_test_files() -> String {
  let dir = std::env::temp_dir();
  let unique = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
  let test_dir = dir.join(format!(
    "grep_test_{}_{}",
    std::process::id(),
    unique
  ));
  std::fs::create_dir_all(&test_dir).unwrap();

  // Create test file with known content
  let f1 = test_dir.join("hello.txt");
  let mut file = std::fs::File::create(&f1).unwrap();
  writeln!(file, "hello world").unwrap();
  writeln!(file, "foo bar").unwrap();
  writeln!(file, "  hello again").unwrap();
  drop(file);

  // Create another test file
  let f2 = test_dir.join("test.rs");
  let mut file = std::fs::File::create(&f2).unwrap();
  writeln!(file, "fn main() {{").unwrap();
  writeln!(file, "    let x = 42;").unwrap();
  writeln!(file, "}}").unwrap();
  writeln!(file, "// hello comment").unwrap();
  drop(file);

  test_dir.to_string_lossy().to_string()
}

#[test]
fn test_grep_basic_match() {
  let root = make_test_files();
  let tool = GrepTool::new();
  let args = json!({"pattern": "hello", "path": root});
  let result = tool.handle(&args).unwrap();
  assert!(result.contains("hello world"));
  assert!(result.contains("hello again"));
}

#[test]
fn test_grep_line_and_column() {
  let root = make_test_files();
  let tool = GrepTool::new();
  let args = json!({"pattern": "hello", "path": root});
  let result = tool.handle(&args).unwrap();
  // "hello world" starts at line 1, column 1
  assert!(result.contains("1,1:hello world"));
  // "  hello again" has hello starting at column 3 (2 leading spaces, 1-based)
  assert!(result.contains(",3:"));
}

#[test]
fn test_grep_file_filter() {
  let root = make_test_files();
  let tool = GrepTool::new();
  let args = json!({"pattern": "hello", "path": root, "file_pattern": "*.txt"});
  let result = tool.handle(&args).unwrap();
  assert!(result.contains("hello.txt"));
  assert!(!result.contains("test.rs"));
}

#[test]
fn test_grep_case_insensitive() {
  let root = make_test_files();
  let tool = GrepTool::new();
  let args = json!({"pattern": "HELLO", "path": root, "case_insensitive": true});
  let result = tool.handle(&args).unwrap();
  assert!(result.contains("hello"));
}

#[test]
fn test_grep_regex() {
  let root = make_test_files();
  let tool = GrepTool::new();
  let args = json!({"pattern": "fn \\w+\\(\\)", "path": root});
  let result = tool.handle(&args).unwrap();
  assert!(result.contains("fn main()"));
}

#[test]
fn test_grep_no_matches() {
  let root = make_test_files();
  let tool = GrepTool::new();
  let args = json!({"pattern": "zzzznotfound", "path": root});
  let result = tool.handle(&args).unwrap();
  assert!(result.contains("No matches found"));
}

#[test]
fn test_grep_missing_pattern() {
  let tool = GrepTool::new();
  let args = json!({});
  let result = tool.handle(&args).unwrap_err();
  assert!(result.to_string().contains("Missing 'pattern'"));
}

#[test]
fn test_grep_invalid_regex() {
  let tool = GrepTool::new();
  let args = json!({"pattern": "[invalid"});
  let result = tool.handle(&args).unwrap_err();
  assert!(result.to_string().contains("Invalid regex"));
}

#[test]
fn test_grep_max_results() {
  let root = make_test_files();
  let tool = GrepTool::new();
  let args = json!({"pattern": "hello", "path": root, "max_results": 1});
  let result = tool.handle(&args).unwrap();
  assert!(result.contains("limited to 1 results"));
}
