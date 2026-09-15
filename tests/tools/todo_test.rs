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

// `todo.rs` resolves its storage directory through `crate::util::get_user_home_path()`;
// pointing it at /tmp keeps this test binary off the real ~/.vtmate.
mod util {
  pub fn get_user_home_path() -> Option<std::path::PathBuf> {
    Some(std::path::PathBuf::from("/tmp"))
  }
}

#[path = "../../src/tools/todo.rs"]
mod todo;

use self::todo::{
  TodoDefineTool, TodoDeleteTool, TodoGetItemTool, TodoGetTool, TodoListTool, TodoSetStatusTool,
  TodoUpdateTool,
};
use serde_json::json;
use std::sync::atomic::{AtomicUsize, Ordering};

static TEST_COUNTER: AtomicUsize = AtomicUsize::new(0);

/// A unique title per test (and a cleanup guard for its file), so tests can
/// run in parallel without a shared lock and don't leak into `todo_list`.
struct TestTodo {
  title: String,
}

impl TestTodo {
  fn new(prefix: &str) -> Self {
    let n = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
    TestTodo {
      title: format!("{} {} {}", prefix, std::process::id(), n),
    }
  }
}

impl Drop for TestTodo {
  fn drop(&mut self) {
    let _ = TodoDeleteTool::new().handle(&json!({ "title": self.title }));
  }
}

#[test]
fn define_then_get_round_trips() {
  let t = TestTodo::new("round-trip");
  let out = TodoDefineTool::new()
    .handle(&json!({
      "title": t.title,
      "description": "a test list",
      "tasks": [
        { "text": "first" },
        { "text": "second", "status": "in_progress", "subtasks": [
          { "text": "nested" }
        ] }
      ]
    }))
    .unwrap();
  assert!(out.contains(&t.title));
  assert!(out.contains("[ ] first"));
  assert!(out.contains("[>] second"));
  assert!(out.contains("    [ ] nested"));

  let got = TodoGetTool::new()
    .handle(&json!({ "title": t.title }))
    .unwrap();
  assert_eq!(got, out);
}

#[test]
fn define_rejects_duplicate_title() {
  let t = TestTodo::new("dup");
  TodoDefineTool::new()
    .handle(&json!({ "title": t.title, "description": "d", "tasks": [] }))
    .unwrap();
  let err = TodoDefineTool::new()
    .handle(&json!({ "title": t.title, "description": "d2", "tasks": [] }))
    .unwrap_err();
  assert!(err.to_string().contains("already exists"));
}

#[test]
fn get_item_returns_nested_subtask() {
  let t = TestTodo::new("get-item");
  TodoDefineTool::new()
    .handle(&json!({
      "title": t.title,
      "description": "d",
      "tasks": [
        { "text": "top" },
        { "text": "parent", "subtasks": [ { "text": "child", "status": "done" } ] }
      ]
    }))
    .unwrap();

  let out = TodoGetItemTool::new()
    .handle(&json!({ "title": t.title, "path": [1, 0] }))
    .unwrap();
  assert_eq!(out, "[x] child\n");
}

#[test]
fn get_item_missing_path_errors() {
  let t = TestTodo::new("get-item-missing");
  TodoDefineTool::new()
    .handle(&json!({ "title": t.title, "description": "d", "tasks": [ { "text": "only" } ] }))
    .unwrap();
  let err = TodoGetItemTool::new()
    .handle(&json!({ "title": t.title, "path": [5] }))
    .unwrap_err();
  assert!(err.to_string().contains("No task at path"));
}

#[test]
fn set_status_updates_task() {
  let t = TestTodo::new("set-status");
  TodoDefineTool::new()
    .handle(&json!({ "title": t.title, "description": "d", "tasks": [ { "text": "a task" } ] }))
    .unwrap();
  let out = TodoSetStatusTool::new()
    .handle(&json!({ "title": t.title, "path": [0], "status": "done" }))
    .unwrap();
  assert!(out.contains("[x] a task"));
}

#[test]
fn set_status_rejects_invalid_status() {
  let t = TestTodo::new("bad-status");
  TodoDefineTool::new()
    .handle(&json!({ "title": t.title, "description": "d", "tasks": [ { "text": "a task" } ] }))
    .unwrap();
  let err = TodoSetStatusTool::new()
    .handle(&json!({ "title": t.title, "path": [0], "status": "not-a-status" }))
    .unwrap_err();
  assert!(err.to_string().contains("Invalid status"));
}

#[test]
fn update_add_edit_remove_reorder() {
  let t = TestTodo::new("update-ops");
  TodoDefineTool::new()
    .handle(&json!({
      "title": t.title,
      "description": "d",
      "tasks": [ { "text": "one" }, { "text": "two" } ]
    }))
    .unwrap();

  // add a top-level task and a subtask of it
  let out = TodoUpdateTool::new()
    .handle(&json!({
      "title": t.title,
      "operations": [
        { "op": "add", "path": [], "text": "three" },
        { "op": "add", "path": [2], "text": "three-a" }
      ]
    }))
    .unwrap();
  assert!(out.contains("[ ] three"));
  assert!(out.contains("    [ ] three-a"));

  // edit
  let out = TodoUpdateTool::new()
    .handle(&json!({
      "title": t.title,
      "operations": [ { "op": "edit", "path": [0], "text": "one (edited)" } ]
    }))
    .unwrap();
  assert!(out.contains("[ ] one (edited)"));

  // reorder: move "two" (index 1) to the front
  let out = TodoUpdateTool::new()
    .handle(&json!({
      "title": t.title,
      "operations": [ { "op": "reorder", "path": [1], "index": 0 } ]
    }))
    .unwrap();
  let two_pos = out.find("two").unwrap();
  let edited_pos = out.find("one (edited)").unwrap();
  assert!(two_pos < edited_pos, "expected 'two' to now come first:\n{}", out);

  // remove
  let out = TodoUpdateTool::new()
    .handle(&json!({
      "title": t.title,
      "operations": [ { "op": "remove", "path": [0] } ]
    }))
    .unwrap();
  assert!(!out.contains("[ ] two"));
}

#[test]
fn update_batch_is_atomic_on_failure() {
  let t = TestTodo::new("atomic");
  TodoDefineTool::new()
    .handle(&json!({ "title": t.title, "description": "d", "tasks": [ { "text": "keep me" } ] }))
    .unwrap();

  let err = TodoUpdateTool::new()
    .handle(&json!({
      "title": t.title,
      "operations": [
        { "op": "edit", "path": [0], "text": "changed" },
        { "op": "remove", "path": [9] }
      ]
    }))
    .unwrap_err();
  assert!(err.to_string().contains("Operation 1"));

  let got = TodoGetTool::new()
    .handle(&json!({ "title": t.title }))
    .unwrap();
  assert!(got.contains("[ ] keep me"), "edit from the failed batch must not have applied:\n{}", got);
}

#[test]
fn list_reports_progress_summary() {
  let t = TestTodo::new("list-progress");
  TodoDefineTool::new()
    .handle(&json!({
      "title": t.title,
      "description": "d",
      "tasks": [
        { "text": "a", "status": "done" },
        { "text": "b", "status": "in_progress" },
        { "text": "c" }
      ]
    }))
    .unwrap();

  let out = TodoListTool::new().handle(&json!({})).unwrap();
  let line = out
    .lines()
    .find(|l| l.contains(&t.title))
    .unwrap_or_else(|| panic!("todo_list did not mention '{}':\n{}", t.title, out));
  assert!(line.contains("1/3 done, 1 in progress, 1 pending"), "{}", line);
}

#[test]
fn delete_removes_the_todo() {
  let t = TestTodo::new("delete");
  TodoDefineTool::new()
    .handle(&json!({ "title": t.title, "description": "d", "tasks": [] }))
    .unwrap();
  TodoDeleteTool::new()
    .handle(&json!({ "title": t.title }))
    .unwrap();
  let err = TodoGetTool::new()
    .handle(&json!({ "title": t.title }))
    .unwrap_err();
  assert!(err.to_string().contains("No TODO named"));
}

#[test]
fn operations_on_unknown_title_report_not_found() {
  let title = "definitely-not-a-real-todo-title-xyz";
  assert!(
    TodoGetTool::new()
      .handle(&json!({ "title": title }))
      .unwrap_err()
      .to_string()
      .contains("No TODO named")
  );
  assert!(
    TodoGetItemTool::new()
      .handle(&json!({ "title": title, "path": [0] }))
      .unwrap_err()
      .to_string()
      .contains("No TODO named")
  );
  assert!(
    TodoSetStatusTool::new()
      .handle(&json!({ "title": title, "path": [0], "status": "done" }))
      .unwrap_err()
      .to_string()
      .contains("No TODO named")
  );
  assert!(
    TodoUpdateTool::new()
      .handle(&json!({ "title": title, "operations": [] }))
      .unwrap_err()
      .to_string()
      .contains("No TODO named")
  );
  assert!(
    TodoDeleteTool::new()
      .handle(&json!({ "title": title }))
      .unwrap_err()
      .to_string()
      .contains("No TODO named")
  );
}
