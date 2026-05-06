// ------------------------------------------------------------------
//  Tool: Glob
// ------------------------------------------------------------------

use super::Tool;
use glob::{MatchOptions, glob_with};
use serde_json::{Value, json};
use std::fs;
use std::time::SystemTime;

// API
// ------------------------------------------------------------------

pub struct GlobTool;

impl GlobTool {
  pub fn new() -> Self {
    GlobTool
  }
}

impl Tool for GlobTool {
  fn name(&self) -> &str {
    "glob"
  }

  fn handle(
    &self,
    tool_call_args: &Value,
  ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let pattern = tool_call_args
      .get("pattern")
      .and_then(|v| v.as_str())
      .ok_or("Missing 'pattern' argument")?;

    let root = tool_call_args
      .get("root")
      .and_then(|v| v.as_str())
      .unwrap_or(".");

    // Build the full glob pattern: root/pattern
    let full_pattern = format!("{}/{}", root, pattern);

    // Use case-insensitive matching for better cross-platform behavior
    let options = MatchOptions {
      case_sensitive: true,
      require_literal_separator: false,
      require_literal_leading_dot: false,
    };

    let entries = glob_with(&full_pattern, options)
      .map_err(|e| format!("Invalid glob pattern '{}': {}", pattern, e))?;

    // Collect matching paths with their modification times
    let mut matches: Vec<(String, fs::Metadata)> = Vec::new();

    for entry in entries {
      match entry {
        Ok(path) => {
          if let Ok(meta) = fs::metadata(&path) {
            let path_str = path.to_string_lossy().to_string();
            matches.push((path_str, meta));
          }
        }
        Err(e) => {
          // Skip entries that can't be read (e.g., permission issues)
          eprintln!("Warning: could not access path: {}", e);
        }
      }
    }

    // Sort by modification time, newest first
    matches.sort_by(|a, b| {
      let mod_a = a.1.modified().unwrap_or(SystemTime::UNIX_EPOCH);
      let mod_b = b.1.modified().unwrap_or(SystemTime::UNIX_EPOCH);
      mod_b.cmp(&mod_a)
    });

    // Format output
    if matches.is_empty() {
      return Ok(format!(
        "No files matched pattern '{}' in '{}'",
        pattern, root
      ));
    }

    let mut result = format!(
      "Found {} file(s) matching '{}' in '{}', sorted by modification time (newest first):\n\n",
      matches.len(),
      pattern,
      root
    );

    for (path, meta) in &matches {
      let modified = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
      let time_str = chrono::DateTime::<chrono::Local>::from(modified)
        .format("%Y-%m-%d %H:%M:%S")
        .to_string();

      // Use relative-looking path for readability
      result.push_str(&format!("  {}  ({})\n", path, time_str));
    }

    Ok(result)
  }

  fn json_schema(&self) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
    Ok(json!({
      "type": "function",
      "function": {
        "name": "glob",
        "description": "Search for files using glob patterns like **/*.js or src/**/*.ts. Returns matching file paths sorted by modification time (newest first).",
        "parameters": {
          "type": "object",
          "properties": {
            "pattern": {
              "type": "string",
              "description": "Glob pattern to match (e.g. '**/*.js', 'src/**/*.ts')"
            },
            "root": {
              "type": "string",
              "description": "Root directory to search from (defaults to current directory)"
            }
          },
          "required": ["pattern"]
        }
      }
    }))
  }
}
