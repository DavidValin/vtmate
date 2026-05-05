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

#[path = "../../src/tools/read_file.rs"]
mod read_file;

use self::read_file::{LineRange, ReadFileTool};
use serde_json::json;
use std::io::Write;
use std::sync::atomic::{AtomicUsize, Ordering};

static TEST_COUNTER: AtomicUsize = AtomicUsize::new(0);

fn make_test_file(lines: &[&str]) -> String {
  let dir = std::env::temp_dir();
  let unique = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
  let path = dir.join(format!(
    "read_file_test_{}_{}.txt",
    std::process::id(),
    unique
  ));
  let mut file = std::fs::File::create(&path).unwrap();
  for line in lines {
    writeln!(file, "{}", line).unwrap();
  }
  path.to_string_lossy().to_string()
}

#[test]
fn test_parse_range_valid() {
  let r = LineRange::parse("1-5").unwrap();
  assert_eq!(r.start, 1);
  assert_eq!(r.end, 5);
}

#[test]
fn test_parse_range_single_line() {
  let r = LineRange::parse("3-3").unwrap();
  assert_eq!(r.start, 3);
  assert_eq!(r.end, 3);
}

#[test]
fn test_parse_range_invalid_start_greater_than_end() {
  let err = LineRange::parse("5-2").unwrap_err();
  assert!(err.contains("must be >="));
}

#[test]
fn test_parse_range_zero_start() {
  let err = LineRange::parse("0-5").unwrap_err();
  assert!(err.contains(">= 1"));
}

#[test]
fn test_parse_range_bad_format() {
  let err = LineRange::parse("abc-5").unwrap_err();
  assert!(err.contains("positive integer"));
}

#[test]
fn test_read_single_range() {
  let path = make_test_file(&["hello world", "this is the second line", "third line"]);
  let tool = ReadFileTool::new();
  let args = json!({"file_path": path, "ranges": "1-2"});
  let result = tool.handle(&args).unwrap();
  assert_eq!(result, "1:hello world\n2:this is the second line");
}

#[test]
fn test_read_multiple_ranges() {
  let path = make_test_file(&["line1", "line2", "line3", "line4"]);
  let tool = ReadFileTool::new();
  let args = json!({"file_path": path, "ranges": "1-2,4-4"});
  let result = tool.handle(&args).unwrap();
  assert_eq!(result, "1:line1\n2:line2\n\n4:line4");
}

#[test]
fn test_truncation_message() {
  let path = make_test_file(&["only one line"]);
  let tool = ReadFileTool::new();
  let args = json!({"file_path": path, "ranges": "1-5"});
  let result = tool.handle(&args).unwrap();
  assert_eq!(result, "1:only one line\n(file ended at 1, truncating...)");
}

#[test]
fn test_truncation_in_middle_of_range() {
  let path = make_test_file(&["line1", "line2", "line3"]);
  let tool = ReadFileTool::new();
  let args = json!({"file_path": path, "ranges": "2-6"});
  let result = tool.handle(&args).unwrap();
  assert_eq!(result, "2:line2\n3:line3\n(file ended at 3, truncating...)");
}

#[test]
fn test_file_not_found() {
  let tool = ReadFileTool::new();
  let args = json!({"file_path": "/nonexistent/file.txt", "ranges": "1-1"});
  let result = tool.handle(&args).unwrap_err();
  assert!(result.to_string().contains("Failed to read file"));
}

#[test]
fn test_missing_file_path() {
  let tool = ReadFileTool::new();
  let args = json!({"ranges": "1-1"});
  let result = tool.handle(&args).unwrap_err();
  assert!(result.to_string().contains("Missing 'file_path'"));
}

#[test]
fn test_missing_ranges() {
  let tool = ReadFileTool::new();
  let args = json!({"file_path": "/some/file"});
  let result = tool.handle(&args).unwrap_err();
  assert!(result.to_string().contains("Missing 'ranges'"));
}
