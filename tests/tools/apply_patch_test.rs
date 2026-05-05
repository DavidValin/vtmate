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

#[path = "../../src/tools/apply_patch.rs"]
mod apply_patch;

use self::apply_patch::{parse_range, ApplyPatchTool};

#[test]
fn test_simple_replace() {
  let content = "line1\nline2\nline3\nline4\nline5\n";
  let patch = "--- a/test.txt\n+++ b/test.txt\n@@ -1,5 +1,5 @@\n line1\n-line2\n+line2 modified\n line3\n line4\n line5\n";
  let hunks = ApplyPatchTool::parse_hunks(patch).unwrap();
  let result = ApplyPatchTool::apply_hunks(content, &hunks).unwrap();
  assert_eq!(result, "line1\nline2 modified\nline3\nline4\nline5\n");
}

#[test]
fn test_add_line() {
  let content = "line1\nline2\nline3\n";
  let patch = "--- a/test.txt\n+++ b/test.txt\n@@ -1,3 +1,4 @@\n line1\n line2\n+inserted\n line3\n";
  let hunks = ApplyPatchTool::parse_hunks(patch).unwrap();
  let result = ApplyPatchTool::apply_hunks(content, &hunks).unwrap();
  assert_eq!(result, "line1\nline2\ninserted\nline3\n");
}

#[test]
fn test_delete_line() {
  let content = "line1\nline2\nline3\n";
  let patch = "--- a/test.txt\n+++ b/test.txt\n@@ -1,3 +1,2 @@\n line1\n-line2\n line3\n";
  let hunks = ApplyPatchTool::parse_hunks(patch).unwrap();
  let result = ApplyPatchTool::apply_hunks(content, &hunks).unwrap();
  assert_eq!(result, "line1\nline3\n");
}

#[test]
fn test_combined_add_remove() {
  // Full test: modify + insert in one hunk
  let content = "line1\nline2\nline3\nline4\nline5\n";
  let patch = "--- a/test.txt\n+++ b/test.txt\n@@ -1,5 +1,6 @@\n line1\n-line2\n+line2 modified\n line3\n+inserted line\n line4\n line5\n";
  let hunks = ApplyPatchTool::parse_hunks(patch).unwrap();
  let result = ApplyPatchTool::apply_hunks(content, &hunks).unwrap();
  assert_eq!(result, "line1\nline2 modified\nline3\ninserted line\nline4\nline5\n");
}

#[test]
fn test_multi_hunk() {
  let content = "a\nb\nc\nd\ne\nf\ng\n";
  // Hunk 1: lines 1-3, replace a→A (1 old, 1 new)
  // Hunk 2: lines 6-7, replace f→F (2 old, 2 new)
  let patch = "--- a/test.txt\n+++ b/test.txt\n@@ -1,3 +1,3 @@\n-a\n+A\n b\n c\n@@ -6,2 +6,2 @@\n-f\n+F\n g\n";
  let hunks = ApplyPatchTool::parse_hunks(patch).unwrap();
  assert_eq!(hunks.len(), 2);
  let result = ApplyPatchTool::apply_hunks(content, &hunks).unwrap();
  assert_eq!(result, "A\nb\nc\nd\ne\nF\ng\n");
}

#[test]
fn test_no_trailing_newline() {
  let content = "line1\nline2";
  let patch = "--- a/test.txt\n+++ b/test.txt\n@@ -1,2 +1,2 @@\n line1\n-line2\n+line2 modified\n";
  let hunks = ApplyPatchTool::parse_hunks(patch).unwrap();
  let result = ApplyPatchTool::apply_hunks(content, &hunks).unwrap();
  assert_eq!(result, "line1\nline2 modified");
}

#[test]
fn test_context_mismatch_returns_error() {
  let content = "line1\nline2\nline3\n";
  let patch = "--- a/test.txt\n+++ b/test.txt\n@@ -1,3 +1,3 @@\n line1\n-wrong\n+line2 modified\n line3\n";
  let hunks = ApplyPatchTool::parse_hunks(patch).unwrap();
  let result = ApplyPatchTool::apply_hunks(content, &hunks);
  assert!(result.is_err());
}

#[test]
fn test_end_of_file_patch() {
  // Patch at the very end of file, adding a line
  let content = "foo\nbar\n";
  let patch = "--- a/test.txt\n+++ b/test.txt\n@@ -2 +2,2 @@\n bar\n+baz\n";
  let hunks = ApplyPatchTool::parse_hunks(patch).unwrap();
  let result = ApplyPatchTool::apply_hunks(content, &hunks).unwrap();
  assert_eq!(result, "foo\nbar\nbaz\n");
}

#[test]
fn test_beginning_of_file_patch() {
  // Patch at the beginning, inserting before first line
  let content = "foo\nbar\n";
  let patch = "--- a/test.txt\n+++ b/test.txt\n@@ -1,2 +1,3 @@\n+baz\n foo\n bar\n";
  let hunks = ApplyPatchTool::parse_hunks(patch).unwrap();
  let result = ApplyPatchTool::apply_hunks(content, &hunks).unwrap();
  assert_eq!(result, "baz\nfoo\nbar\n");
}

#[test]
fn test_parse_range_single() {
  let (start, count) = parse_range("5").unwrap();
  assert_eq!(start, 5);
  assert_eq!(count, 1);
}

#[test]
fn test_parse_range_with_count() {
  let (start, count) = parse_range("3,7").unwrap();
  assert_eq!(start, 3);
  assert_eq!(count, 7);
}
