// ------------------------------------------------------------------
//  Tool: Read File
// ------------------------------------------------------------------

use super::Tool;
use serde_json::{Value, json};

// API
// ------------------------------------------------------------------

pub struct ReadFileTool;

// PRIVATE
// ------------------------------------------------------------------

/// A parsed line range like "1-2" or "200-204".
#[derive(Debug)]
pub struct LineRange {
  pub start: usize,
  pub end: usize,
}

impl LineRange {
  /// Parse a string like "1-2" into a LineRange.
  /// Returns an error if the format is invalid or start > end.
  pub fn parse(s: &str) -> Result<Self, String> {
    let parts: Vec<&str> = s.split('-').collect();
    if parts.len() != 2 {
      return Err(format!(
        "Invalid range '{}': expected 'start-end' format",
        s
      ));
    }

    let start: usize = parts[0].parse().map_err(|_| {
      format!(
        "Invalid range start '{}': must be a positive integer",
        parts[0]
      )
    })?;

    let end: usize = parts[1].parse().map_err(|_| {
      format!(
        "Invalid range end '{}': must be a positive integer",
        parts[1]
      )
    })?;

    if start == 0 {
      return Err(format!("Invalid range '{}': line numbers must be >= 1", s));
    }

    if start > end {
      return Err(format!(
        "Invalid range '{}': end line ({}) must be >= start line ({})",
        s, end, start
      ));
    }

    Ok(LineRange { start, end })
  }
}

impl ReadFileTool {
  pub fn new() -> Self {
    ReadFileTool
  }
}

impl Tool for ReadFileTool {
  fn name(&self) -> &str {
    "read_file"
  }

  fn handle(
    &self,
    tool_call_args: &Value,
  ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let file_path = tool_call_args
      .get("file_path")
      .and_then(|v| v.as_str())
      .ok_or("Missing 'file_path' argument")?;

    let ranges_str = tool_call_args
      .get("ranges")
      .and_then(|v| v.as_str())
      .ok_or("Missing 'ranges' argument")?;

    // Parse comma-separated ranges like "1-2,200-204"
    let ranges: Result<Vec<LineRange>, _> = ranges_str
      .split(',')
      .map(|s| s.trim())
      .filter(|s| !s.is_empty())
      .map(LineRange::parse)
      .collect();

    let ranges = ranges?;

    if ranges.is_empty() {
      return Err("At least one range must be provided".into());
    }

    // Read the file
    let content = std::fs::read_to_string(file_path)
      .map_err(|e| format!("Failed to read file '{}': {}", file_path, e))?;

    let lines: Vec<&str> = content.lines().collect();
    let total_lines = lines.len();

    // Collect output for each range
    let mut parts = Vec::new();

    for range in &ranges {
      let mut range_lines = Vec::new();
      let mut truncated = false;

      for line_num in range.start..=range.end {
        let idx = line_num - 1;
        if idx < total_lines {
          range_lines.push(format!("{}:{}", line_num, lines[idx]));
        } else {
          // File ended before the requested range
          if !truncated {
            range_lines.push(format!("(file ended at {}, truncating...)", total_lines));
            truncated = true;
          }
        }
      }

      parts.push(range_lines.join("\n"));
    }

    // Join ranges with a blank line separator
    Ok(parts.join("\n\n"))
  }

  fn json_schema(&self) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
    Ok(json!({
      "type": "function",
      "function": {
        "name": "read_file",
        "description": "Reads specific line ranges from a file. Supports multiple comma-separated ranges in 'start-end' format (1-based, inclusive). Each range must have end >= start. Returns content with line numbers prefixed.",
        "parameters": {
          "type": "object",
          "properties": {
            "file_path": {
              "type": "string",
              "description": "Absolute path to the file to read"
            },
            "ranges": {
              "type": "string",
              "description": "Comma-separated line ranges in 'start-end' format (1-based, inclusive). Example: '1-2,200-204'"
            }
          },
          "required": ["file_path", "ranges"]
        }
      }
    }))
  }
}
