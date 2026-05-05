// ------------------------------------------------------------------
//  Tool: Grep
// ------------------------------------------------------------------

use super::Tool;
use glob::{glob_with, MatchOptions};
use regex::RegexBuilder;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fs;

// API
// ------------------------------------------------------------------

pub struct GrepTool;

impl GrepTool {
  pub fn new() -> Self {
    GrepTool
  }
}

impl Tool for GrepTool {
  fn name(&self) -> &str {
    "grep"
  }

  fn handle(
    &self,
    tool_call_args: &Value,
  ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let pattern_str = tool_call_args
      .get("pattern")
      .and_then(|v| v.as_str())
      .ok_or("Missing 'pattern' argument")?;

    let path = tool_call_args
      .get("path")
      .and_then(|v| v.as_str())
      .unwrap_or(".");

    let file_pattern = tool_call_args
      .get("file_pattern")
      .and_then(|v| v.as_str())
      .unwrap_or("**/*");

    let case_insensitive = tool_call_args
      .get("case_insensitive")
      .and_then(|v| v.as_bool())
      .unwrap_or(false);

    let max_results = tool_call_args
      .get("max_results")
      .and_then(|v| v.as_u64())
      .unwrap_or(500);

    // Compile the regex
    let re = RegexBuilder::new(pattern_str)
      .case_insensitive(case_insensitive)
      .dot_matches_new_line(true)
      .build()
      .map_err(|e| format!("Invalid regex '{}': {}", pattern_str, e))?;

    // Resolve file list via glob
    let full_glob = format!("{}/{}", path, file_pattern);
    let match_opts = MatchOptions {
      case_sensitive: true,
      require_literal_separator: false,
      require_literal_leading_dot: false,
    };

    let entries = glob_with(&full_glob, match_opts)
      .map_err(|e| format!("Invalid file pattern '{}': {}", file_pattern, e))?;

    // Collect regular files only
    let mut files: Vec<String> = Vec::new();
    for entry in entries {
      match entry {
        Ok(fp) => {
          if let Ok(meta) = fs::metadata(&fp) {
            if meta.is_file() {
              files.push(fp.to_string_lossy().to_string());
            }
          }
        }
        Err(_) => continue,
      }
    }

    // Search each file
    let mut results: BTreeMap<String, Vec<(usize, usize, String)>> = BTreeMap::new();
    let mut total_matches: u64 = 0;

    for filepath in &files {
      // Skip binary files: try reading as UTF-8, skip on failure
      let content = match fs::read_to_string(filepath) {
        Ok(c) => c,
        Err(_) => continue,
      };

      // Quick heuristic: skip files that look binary (contain null bytes)
      if content.contains('\0') {
        continue;
      }

      for (line_num, line) in content.lines().enumerate() {
        if let Some(m) = re.find(line) {
          let line_1based = line_num + 1;
          let col_1based: usize = m.start() + 1;
          results
            .entry(filepath.clone())
            .or_default()
            .push((line_1based, col_1based, line.trim_end_matches('\r').to_string()));

          total_matches += 1;
          if total_matches >= max_results {
            break;
          }
        }
      }
      if total_matches >= max_results {
        break;
      }
    }

    // Format output
    if results.is_empty() {
      return Ok(format!(
        "No matches found for '{}' in '{}'",
        pattern_str, path
      ));
    }

    let mut output = String::new();
    let mut file_count = 0;
    let path_prefix = if path == "." {
      String::new()
    } else {
      // Strip trailing slash if present for display
      let p = path.strip_suffix('/').unwrap_or(path);
      format!("{}/", p)
    };

    for (filepath, matches) in &results {
      // Strip the search path prefix for cleaner display
      let display_path = filepath.strip_prefix(&path_prefix).unwrap_or(filepath.as_str());
      file_count += 1;
      output.push_str(&format!("[{}]\n", display_path));
      for (line, col, text) in matches {
        output.push_str(&format!("{},{}:{}\n", line, col, text));
      }
      output.push('\n');
    }

    let summary = if total_matches >= max_results {
      format!(
        "Found {}+ matches in {} file(s) (limited to {} results)",
        total_matches, file_count, max_results
      )
    } else {
      format!(
        "Found {} matches in {} file(s)",
        total_matches, file_count
      )
    };

    Ok(format!("{}\n{}", output.trim_end(), summary))
  }

  fn json_schema(&self) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
    Ok(json!({
      "type": "function",
      "function": {
        "name": "grep",
        "description": "Fast content search across files in a directory using full regex syntax. Returns matching files with line and column positions for each match.",
        "parameters": {
          "type": "object",
          "properties": {
            "pattern": {
              "type": "string",
              "description": "Regex pattern to search for"
            },
            "path": {
              "type": "string",
              "description": "Directory to search in (defaults to current directory)"
            },
            "file_pattern": {
              "type": "string",
              "description": "Glob pattern to filter files (e.g. '**/*.rs', '*.txt'). Defaults to all files."
            },
            "case_insensitive": {
              "type": "boolean",
              "description": "Whether to perform case-insensitive matching (defaults to false)"
            },
            "max_results": {
              "type": "integer",
              "description": "Maximum number of matches to return (defaults to 500)"
            }
          },
          "required": ["pattern"]
        }
      }
    }))
  }
}


