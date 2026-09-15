// ------------------------------------------------------------------
//  Tool: TODO lists
//
//  Named, persisted checklists at ~/.vtmate/todos/<slug>.json so the agent
//  can define a plan, work through it across turns, and resume an
//  in-progress one after a restart. Every tool call reads the relevant file
//  fresh, mutates it, and writes it back atomically (tmp file + rename) —
//  there is no in-memory cache, so these tools stay stateless the same way
//  the rest of src/tools/ is; disk is the only state.
// ------------------------------------------------------------------

use super::Tool;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::fs;
use std::io::Write as _;
use std::path::PathBuf;

// API
// ------------------------------------------------------------------

pub struct TodoDefineTool;
pub struct TodoUpdateTool;
pub struct TodoListTool;
pub struct TodoGetTool;
pub struct TodoGetItemTool;
pub struct TodoSetStatusTool;
pub struct TodoDeleteTool;

impl TodoDefineTool {
  pub fn new() -> Self {
    TodoDefineTool
  }
}
impl TodoUpdateTool {
  pub fn new() -> Self {
    TodoUpdateTool
  }
}
impl TodoListTool {
  pub fn new() -> Self {
    TodoListTool
  }
}
impl TodoGetTool {
  pub fn new() -> Self {
    TodoGetTool
  }
}
impl TodoGetItemTool {
  pub fn new() -> Self {
    TodoGetItemTool
  }
}
impl TodoSetStatusTool {
  pub fn new() -> Self {
    TodoSetStatusTool
  }
}
impl TodoDeleteTool {
  pub fn new() -> Self {
    TodoDeleteTool
  }
}

/// The seven todo_* tool names, registered together under the single
/// "to-do" alias in an agent's `tools=` line — see `config::parse_tools`.
pub const TOOL_NAMES: [&str; 7] = [
  "todo_define",
  "todo_update",
  "todo_list",
  "todo_get",
  "todo_get_item",
  "todo_set_status",
  "todo_delete",
];

/// A TODO list as stored on disk: title + description (both agent-supplied,
/// shown by the `t` popup) plus its tasks.
#[derive(Serialize, Deserialize)]
pub(crate) struct TodoDoc {
  title: String,
  #[serde(default)]
  description: String,
  #[serde(default)]
  tasks: Vec<TodoItem>,
}

/// The TODO most recently written to disk — what the live `t` popup shows,
/// on the assumption that whichever list the agent touched last is the one
/// currently being worked.
pub(crate) fn most_recent_doc() -> Option<TodoDoc> {
  list_docs().into_iter().next().map(|(doc, _)| doc)
}

/// Render a doc as `[ ]`/`[>]`/`[x]` checklist text, title and description
/// first — shared by every tool's return value and the `t` popup, so both
/// surfaces show the same thing.
pub(crate) fn render(doc: &TodoDoc) -> String {
  let mut out = format!("{}\n{}\n\n", doc.title, doc.description);
  render_items(&doc.tasks, 0, &mut out);
  out
}

// PRIVATE
// ------------------------------------------------------------------

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum TodoStatus {
  Pending,
  InProgress,
  Done,
}

impl TodoStatus {
  fn marker(&self) -> &'static str {
    match self {
      TodoStatus::Pending => " ",
      TodoStatus::InProgress => ">",
      TodoStatus::Done => "x",
    }
  }

  fn parse(s: &str) -> Result<Self, String> {
    match s {
      "pending" => Ok(TodoStatus::Pending),
      "in_progress" => Ok(TodoStatus::InProgress),
      "done" => Ok(TodoStatus::Done),
      other => Err(format!(
        "Invalid status '{}': expected 'pending', 'in_progress', or 'done'",
        other
      )),
    }
  }
}

#[derive(Clone, Serialize, Deserialize)]
struct TodoItem {
  text: String,
  status: TodoStatus,
  #[serde(default)]
  subtasks: Vec<TodoItem>,
}

fn render_items(items: &[TodoItem], depth: usize, out: &mut String) {
  for item in items {
    out.push_str(&"    ".repeat(depth));
    out.push_str(&format!("[{}] {}\n", item.status.marker(), item.text));
    render_items(&item.subtasks, depth + 1, out);
  }
}

fn render_item(item: &TodoItem) -> String {
  let mut out = String::new();
  render_items(std::slice::from_ref(item), 0, &mut out);
  out
}

fn count_statuses(items: &[TodoItem], counts: &mut (usize, usize, usize)) {
  for item in items {
    match item.status {
      TodoStatus::Pending => counts.0 += 1,
      TodoStatus::InProgress => counts.1 += 1,
      TodoStatus::Done => counts.2 += 1,
    }
    count_statuses(&item.subtasks, counts);
  }
}

fn progress_summary(doc: &TodoDoc) -> String {
  let mut counts = (0usize, 0usize, 0usize);
  count_statuses(&doc.tasks, &mut counts);
  let total = counts.0 + counts.1 + counts.2;
  format!(
    "{}/{} done, {} in progress, {} pending",
    counts.2, total, counts.1, counts.0
  )
}

// Storage
// ------------------------------------------------------------------

fn todos_dir() -> Result<PathBuf, String> {
  let home = crate::util::get_user_home_path().ok_or("Could not determine home directory")?;
  Ok(home.join(".vtmate").join("todos"))
}

/// Lowercase, non-alphanumeric runs collapsed to a single '-', edge dashes
/// trimmed — titles are the unique key, so this is also the filename.
fn slugify(title: &str) -> String {
  let mut slug = String::new();
  let mut last_was_dash = true; // suppresses a leading dash
  for c in title.chars() {
    if c.is_ascii_alphanumeric() {
      slug.push(c.to_ascii_lowercase());
      last_was_dash = false;
    } else if !last_was_dash {
      slug.push('-');
      last_was_dash = true;
    }
  }
  while slug.ends_with('-') {
    slug.pop();
  }
  if slug.is_empty() {
    "untitled".to_string()
  } else {
    slug
  }
}

fn doc_path(title: &str) -> Result<PathBuf, String> {
  Ok(todos_dir()?.join(format!("{}.json", slugify(title))))
}

fn load_doc(title: &str) -> Result<TodoDoc, String> {
  let path = doc_path(title)?;
  let content = fs::read_to_string(&path).map_err(|_| {
    format!(
      "No TODO named '{}' found. Use todo_list to see existing TODOs.",
      title
    )
  })?;
  serde_json::from_str(&content).map_err(|e| format!("Failed to parse TODO '{}': {}", title, e))
}

fn write_atomically(path: &std::path::Path, contents: &str) -> std::io::Result<()> {
  let tmp = path.with_extension("json.tmp");
  {
    let mut f = fs::File::create(&tmp)?;
    f.write_all(contents.as_bytes())?;
    f.sync_all()?;
  }
  fs::rename(&tmp, path)
}

fn save_doc(doc: &TodoDoc) -> Result<(), String> {
  let dir = todos_dir()?;
  fs::create_dir_all(&dir).map_err(|e| format!("Failed to create TODO directory: {}", e))?;
  let path = dir.join(format!("{}.json", slugify(&doc.title)));
  let contents = serde_json::to_string_pretty(doc)
    .map_err(|e| format!("Failed to serialize TODO '{}': {}", doc.title, e))?;
  write_atomically(&path, &contents)
    .map_err(|e| format!("Failed to write TODO '{}': {}", doc.title, e))
}

fn delete_doc(title: &str) -> Result<(), String> {
  let path = doc_path(title)?;
  fs::remove_file(&path).map_err(|_| {
    format!(
      "No TODO named '{}' found. Use todo_list to see existing TODOs.",
      title
    )
  })
}

/// Every persisted TODO, most-recently-modified first.
fn list_docs() -> Vec<(TodoDoc, std::time::SystemTime)> {
  let dir = match todos_dir() {
    Ok(d) => d,
    Err(_) => return Vec::new(),
  };
  let mut out = Vec::new();
  if let Ok(entries) = fs::read_dir(&dir) {
    for entry in entries.flatten() {
      let path = entry.path();
      if path.extension().and_then(|e| e.to_str()) != Some("json") {
        continue;
      }
      let modified = entry
        .metadata()
        .and_then(|m| m.modified())
        .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
      if let Ok(content) = fs::read_to_string(&path) {
        if let Ok(doc) = serde_json::from_str::<TodoDoc>(&content) {
          out.push((doc, modified));
        }
      }
    }
  }
  out.sort_by(|a, b| b.1.cmp(&a.1));
  out
}

// Tree navigation, addressed by an array of 0-based indices (e.g. [1, 0] =
// subtask 0 of top-level task 1).
// ------------------------------------------------------------------

fn get_item<'a>(tasks: &'a [TodoItem], path: &[usize]) -> Result<&'a TodoItem, String> {
  let (&idx, rest) = path
    .split_first()
    .ok_or("'path' must not be empty".to_string())?;
  let item = tasks
    .get(idx)
    .ok_or_else(|| format!("No task at path {:?}", path))?;
  if rest.is_empty() {
    Ok(item)
  } else {
    get_item(&item.subtasks, rest)
  }
}

fn get_item_mut<'a>(tasks: &'a mut [TodoItem], path: &[usize]) -> Result<&'a mut TodoItem, String> {
  let (&idx, rest) = path
    .split_first()
    .ok_or("'path' must not be empty".to_string())?;
  let item = tasks
    .get_mut(idx)
    .ok_or_else(|| format!("No task at path {:?}", path))?;
  if rest.is_empty() {
    Ok(item)
  } else {
    get_item_mut(&mut item.subtasks, rest)
  }
}

/// The sibling list `parent_path` addresses: the root list if empty, else
/// the subtasks of the item at that path.
fn get_container_mut<'a>(
  tasks: &'a mut Vec<TodoItem>,
  parent_path: &[usize],
) -> Result<&'a mut Vec<TodoItem>, String> {
  if parent_path.is_empty() {
    Ok(tasks)
  } else {
    Ok(&mut get_item_mut(tasks, parent_path)?.subtasks)
  }
}

fn parse_path(tool_call_args: &Value) -> Result<Vec<usize>, String> {
  tool_call_args
    .get("path")
    .and_then(|v| v.as_array())
    .ok_or("Missing 'path' argument (array of 0-based indices)".to_string())?
    .iter()
    .map(|v| {
      v.as_u64()
        .map(|n| n as usize)
        .ok_or_else(|| "'path' must be an array of non-negative integers".to_string())
    })
    .collect()
}

fn parse_task(v: &Value) -> Result<TodoItem, String> {
  let text = v
    .get("text")
    .and_then(|x| x.as_str())
    .ok_or("Each task requires a 'text' field")?
    .to_string();
  let status = match v.get("status").and_then(|x| x.as_str()) {
    Some(s) => TodoStatus::parse(s)?,
    None => TodoStatus::Pending,
  };
  let subtasks = match v.get("subtasks").and_then(|x| x.as_array()) {
    Some(arr) => arr.iter().map(parse_task).collect::<Result<Vec<_>, _>>()?,
    None => Vec::new(),
  };
  Ok(TodoItem {
    text,
    status,
    subtasks,
  })
}

// todo_update operations
// ------------------------------------------------------------------

#[derive(Deserialize)]
struct Operation {
  op: String,
  #[serde(default)]
  path: Vec<usize>,
  text: Option<String>,
  status: Option<String>,
  index: Option<usize>,
}

fn apply_operation(tasks: &mut Vec<TodoItem>, op_idx: usize, op: &Operation) -> Result<(), String> {
  let wrap = |e: String| format!("Operation {} ({}) failed: {}", op_idx, op.op, e);
  match op.op.as_str() {
    "add" => {
      let text = op
        .text
        .clone()
        .ok_or_else(|| wrap("'add' requires 'text'".to_string()))?;
      let status = match &op.status {
        Some(s) => TodoStatus::parse(s).map_err(wrap)?,
        None => TodoStatus::Pending,
      };
      let container = get_container_mut(tasks, &op.path).map_err(wrap)?;
      let insert_at = op.index.unwrap_or(container.len()).min(container.len());
      container.insert(
        insert_at,
        TodoItem {
          text,
          status,
          subtasks: Vec::new(),
        },
      );
      Ok(())
    }
    "edit" => {
      let text = op
        .text
        .clone()
        .ok_or_else(|| wrap("'edit' requires 'text'".to_string()))?;
      let item = get_item_mut(tasks, &op.path).map_err(wrap)?;
      item.text = text;
      Ok(())
    }
    "remove" => {
      let (&idx, parent_path) = op
        .path
        .split_last()
        .ok_or_else(|| wrap("'remove' requires a non-empty 'path'".to_string()))?;
      let container = get_container_mut(tasks, parent_path).map_err(wrap)?;
      if idx >= container.len() {
        return Err(wrap(format!("No task at path {:?}", op.path)));
      }
      container.remove(idx);
      Ok(())
    }
    "reorder" => {
      let new_index = op
        .index
        .ok_or_else(|| wrap("'reorder' requires 'index'".to_string()))?;
      let (&idx, parent_path) = op
        .path
        .split_last()
        .ok_or_else(|| wrap("'reorder' requires a non-empty 'path'".to_string()))?;
      let container = get_container_mut(tasks, parent_path).map_err(wrap)?;
      if idx >= container.len() {
        return Err(wrap(format!("No task at path {:?}", op.path)));
      }
      let item = container.remove(idx);
      container.insert(new_index.min(container.len()), item);
      Ok(())
    }
    other => Err(wrap(format!("Unknown operation '{}'", other))),
  }
}

// Tool impls
// ------------------------------------------------------------------

impl Tool for TodoDefineTool {
  fn name(&self) -> &str {
    "todo_define"
  }

  fn handle(
    &self,
    tool_call_args: &Value,
  ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let title = tool_call_args
      .get("title")
      .and_then(|v| v.as_str())
      .ok_or("Missing 'title' argument")?
      .to_string();
    let description = tool_call_args
      .get("description")
      .and_then(|v| v.as_str())
      .ok_or("Missing 'description' argument")?
      .to_string();
    let tasks_val = tool_call_args
      .get("tasks")
      .and_then(|v| v.as_array())
      .ok_or("Missing 'tasks' argument")?;
    let tasks = tasks_val
      .iter()
      .map(parse_task)
      .collect::<Result<Vec<_>, _>>()?;

    if doc_path(&title)?.exists() {
      return Err(
        format!(
          "A TODO named '{}' already exists. Use todo_update/todo_set_status to continue it, or pick a different title.",
          title
        )
        .into(),
      );
    }

    let doc = TodoDoc {
      title,
      description,
      tasks,
    };
    save_doc(&doc)?;
    Ok(render(&doc))
  }

  fn json_schema(&self) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
    Ok(json!({
      "type": "function",
      "function": {
        "name": "todo_define",
        "description": "Create a new named TODO list with a title, a description, and its initial tasks (which may have nested subtasks). Fails if a TODO with this title already exists — use todo_update or todo_set_status to continue an existing one instead.",
        "parameters": {
          "type": "object",
          "properties": {
            "title": { "type": "string", "description": "Unique name identifying this TODO list" },
            "description": { "type": "string", "description": "Short description of what this TODO list is for" },
            "tasks": {
              "type": "array",
              "description": "Initial top-level tasks",
              "items": {
                "type": "object",
                "properties": {
                  "text": { "type": "string", "description": "The task's text" },
                  "status": { "type": "string", "enum": ["pending", "in_progress", "done"], "description": "Defaults to 'pending' if omitted" },
                  "subtasks": { "type": "array", "description": "Nested subtasks, each with the same shape as a task (text, optional status, optional subtasks)" }
                },
                "required": ["text"]
              }
            }
          },
          "required": ["title", "description", "tasks"]
        }
      }
    }))
  }
}

impl Tool for TodoUpdateTool {
  fn name(&self) -> &str {
    "todo_update"
  }

  fn handle(
    &self,
    tool_call_args: &Value,
  ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let title = tool_call_args
      .get("title")
      .and_then(|v| v.as_str())
      .ok_or("Missing 'title' argument")?;
    let operations_val = tool_call_args
      .get("operations")
      .and_then(|v| v.as_array())
      .ok_or("Missing 'operations' argument")?;
    let operations: Vec<Operation> = operations_val
      .iter()
      .cloned()
      .map(serde_json::from_value)
      .collect::<Result<_, _>>()
      .map_err(|e| format!("Invalid operation: {}", e))?;

    let doc = load_doc(title)?;
    let mut tasks = doc.tasks.clone();
    // Applied to a clone, saved only once every operation has succeeded — an
    // update batch is all-or-nothing, so a failing operation never leaves the
    // list half-changed.
    for (i, op) in operations.iter().enumerate() {
      apply_operation(&mut tasks, i, op)?;
    }
    let new_doc = TodoDoc {
      title: doc.title,
      description: doc.description,
      tasks,
    };
    save_doc(&new_doc)?;
    Ok(render(&new_doc))
  }

  fn json_schema(&self) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
    Ok(json!({
      "type": "function",
      "function": {
        "name": "todo_update",
        "description": "Apply an ordered list of operations to an existing TODO's tasks, addressed by tree position. Applied atomically: if any operation fails, nothing is changed.",
        "parameters": {
          "type": "object",
          "properties": {
            "title": { "type": "string", "description": "Name of the TODO list to update" },
            "operations": {
              "type": "array",
              "description": "Operations applied in order. Each is {op, path, text?, status?, index?}: op is 'add'|'edit'|'remove'|'reorder'; path is an array of 0-based indices, e.g. [1, 0] = subtask 0 of top-level task 1. 'add': path is the PARENT path ([] for a new top-level task), text is required, status optional (default 'pending'), index optional insert position (default: append). 'edit': path + text (required). 'remove': path (required). 'reorder': path + index, the new position among its current siblings (both required).",
              "items": { "type": "object" }
            }
          },
          "required": ["title", "operations"]
        }
      }
    }))
  }
}

impl Tool for TodoListTool {
  fn name(&self) -> &str {
    "todo_list"
  }

  fn handle(
    &self,
    _tool_call_args: &Value,
  ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let docs = list_docs();
    if docs.is_empty() {
      return Ok("No TODO lists yet. Use todo_define to create one.".to_string());
    }
    let mut out = String::new();
    for (doc, _) in &docs {
      out.push_str(&format!(
        "- {} — {} ({})\n",
        doc.title,
        doc.description,
        progress_summary(doc)
      ));
    }
    Ok(out)
  }

  fn json_schema(&self) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
    Ok(json!({
      "type": "function",
      "function": {
        "name": "todo_list",
        "description": "List every persisted TODO list (title, description, progress), most recently touched first. Use this to discover in-progress work and resume it.",
        "parameters": { "type": "object", "properties": {} }
      }
    }))
  }
}

impl Tool for TodoGetTool {
  fn name(&self) -> &str {
    "todo_get"
  }

  fn handle(
    &self,
    tool_call_args: &Value,
  ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let title = tool_call_args
      .get("title")
      .and_then(|v| v.as_str())
      .ok_or("Missing 'title' argument")?;
    let doc = load_doc(title)?;
    Ok(render(&doc))
  }

  fn json_schema(&self) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
    Ok(json!({
      "type": "function",
      "function": {
        "name": "todo_get",
        "description": "Get the full contents of one named TODO list.",
        "parameters": {
          "type": "object",
          "properties": {
            "title": { "type": "string", "description": "Name of the TODO list to read" }
          },
          "required": ["title"]
        }
      }
    }))
  }
}

impl Tool for TodoGetItemTool {
  fn name(&self) -> &str {
    "todo_get_item"
  }

  fn handle(
    &self,
    tool_call_args: &Value,
  ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let title = tool_call_args
      .get("title")
      .and_then(|v| v.as_str())
      .ok_or("Missing 'title' argument")?;
    let path = parse_path(tool_call_args)?;
    let doc = load_doc(title)?;
    let item = get_item(&doc.tasks, &path)?;
    Ok(render_item(item))
  }

  fn json_schema(&self) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
    Ok(json!({
      "type": "function",
      "function": {
        "name": "todo_get_item",
        "description": "Get one task from a named TODO list, along with all of its subtasks.",
        "parameters": {
          "type": "object",
          "properties": {
            "title": { "type": "string", "description": "Name of the TODO list" },
            "path": {
              "type": "array",
              "description": "0-based tree position, e.g. [1, 0] = subtask 0 of top-level task 1",
              "items": { "type": "integer" }
            }
          },
          "required": ["title", "path"]
        }
      }
    }))
  }
}

impl Tool for TodoSetStatusTool {
  fn name(&self) -> &str {
    "todo_set_status"
  }

  fn handle(
    &self,
    tool_call_args: &Value,
  ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let title = tool_call_args
      .get("title")
      .and_then(|v| v.as_str())
      .ok_or("Missing 'title' argument")?;
    let path = parse_path(tool_call_args)?;
    let status_str = tool_call_args
      .get("status")
      .and_then(|v| v.as_str())
      .ok_or("Missing 'status' argument")?;
    let status = TodoStatus::parse(status_str)?;

    let mut doc = load_doc(title)?;
    {
      let item = get_item_mut(&mut doc.tasks, &path)?;
      item.status = status;
    }
    save_doc(&doc)?;
    Ok(render(&doc))
  }

  fn json_schema(&self) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
    Ok(json!({
      "type": "function",
      "function": {
        "name": "todo_set_status",
        "description": "Set one task's status in a named TODO list: 'pending', 'in_progress', or 'done'.",
        "parameters": {
          "type": "object",
          "properties": {
            "title": { "type": "string", "description": "Name of the TODO list" },
            "path": {
              "type": "array",
              "description": "0-based tree position, e.g. [1, 0] = subtask 0 of top-level task 1",
              "items": { "type": "integer" }
            },
            "status": { "type": "string", "enum": ["pending", "in_progress", "done"] }
          },
          "required": ["title", "path", "status"]
        }
      }
    }))
  }
}

impl Tool for TodoDeleteTool {
  fn name(&self) -> &str {
    "todo_delete"
  }

  fn handle(
    &self,
    tool_call_args: &Value,
  ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let title = tool_call_args
      .get("title")
      .and_then(|v| v.as_str())
      .ok_or("Missing 'title' argument")?;
    delete_doc(title)?;
    Ok(format!("Deleted TODO '{}'.", title))
  }

  fn json_schema(&self) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
    Ok(json!({
      "type": "function",
      "function": {
        "name": "todo_delete",
        "description": "Delete a named TODO list entirely.",
        "parameters": {
          "type": "object",
          "properties": {
            "title": { "type": "string", "description": "Name of the TODO list to delete" }
          },
          "required": ["title"]
        }
      }
    }))
  }
}
