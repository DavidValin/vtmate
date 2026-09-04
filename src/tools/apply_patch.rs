// ------------------------------------------------------------------
//  Tool: Apply Patch
// ------------------------------------------------------------------

use super::Tool;
use serde_json::Value;
use serde_json::json;

// API
// ------------------------------------------------------------------

pub struct ApplyPatchTool;

// PRIVATE
// ------------------------------------------------------------------

/// A single hunk from a unified diff.
pub struct Hunk {
  /// 1-based line number in the original file where this hunk starts.
  old_start: usize,
  /// Number of lines in the original file covered by this hunk (parsed for validation).
  #[allow(dead_code)]
  _old_lines: usize,
  /// Parsed operations: Context, Remove, or Add.
  lines: Vec<HunkLine>,
}

enum HunkLine {
  Context(String),
  Remove(String),
  Add(String),
}

impl ApplyPatchTool {
  pub fn new() -> Self {
    ApplyPatchTool
  }

  /// Parse a unified diff string into a list of hunks.
  pub fn parse_hunks(patch: &str) -> Result<Vec<Hunk>, String> {
    let mut hunks = Vec::new();
    let mut current_hunk: Option<Hunk> = None;

    for line in patch.lines() {
      // Detect hunk header: @@ -old_start,old_lines +new_start,new_lines @@
      if line.starts_with("@@") {
        // Finalize previous hunk
        if let Some(h) = current_hunk.take() {
          hunks.push(h);
        }

        let header = line.strip_prefix("@@").unwrap_or(line).trim();
        let (old_start, old_lines) = parse_hunk_header(header)?;
        current_hunk = Some(Hunk {
          old_start,
          _old_lines: old_lines,
          lines: Vec::new(),
        });
        continue;
      }

      // Skip file headers (--- a/..., +++ b/...)
      if line.starts_with("---") || line.starts_with("+++") {
        continue;
      }

      // Skip "No newline at end of file" markers
      if line == "\\ No newline at end of file" {
        continue;
      }

      // Parse hunk content lines
      if let Some(ref mut hunk) = current_hunk {
        let hline = match line.chars().next() {
          Some('+') => HunkLine::Add(line[1..].to_string()),
          Some('-') => HunkLine::Remove(line[1..].to_string()),
          Some(' ') => HunkLine::Context(line[1..].to_string()),
          _ => continue, // Skip unrecognized lines (e.g., index, diff lines)
        };
        hunk.lines.push(hline);
      }
    }

    // Finalize last hunk
    if let Some(h) = current_hunk.take() {
      hunks.push(h);
    }

    if hunks.is_empty() {
      return Err("Patch contains no hunks".into());
    }

    Ok(hunks)
  }

  /// Apply parsed hunks to the file content, returning the patched content.
  pub fn apply_hunks(content: &str, hunks: &[Hunk]) -> Result<String, String> {
    let lines: Vec<&str> = content.lines().collect();
    let mut result = Vec::new();
    let mut file_pos: usize = 0; // 0-based position in the original file

    for hunk in hunks {
      let hunk_start = hunk.old_start.saturating_sub(1); // Convert 1-based to 0-based

      // Insert any untouched lines between this hunk and the previous one
      if hunk_start > file_pos {
        for line in &lines[file_pos..hunk_start] {
          result.push(line.to_string());
        }
      } else if hunk_start < file_pos {
        return Err(format!(
          "Hunk at line {} overlaps with previous hunk (expected at line {})",
          hunk.old_start,
          file_pos + 1
        ));
      }

      let mut hunk_file_pos = hunk_start;

      for hline in &hunk.lines {
        match hline {
          HunkLine::Context(text) | HunkLine::Remove(text) => {
            // Verify the line in the file matches the patch expectation
            if hunk_file_pos < lines.len() {
              let file_line = lines[hunk_file_pos];
              if file_line != *text {
                // Allow fuzzy matching: if content doesn't match exactly, still
                // try to apply (the patch may have minor whitespace differences).
                // But log a warning-style error for context lines.
                if let HunkLine::Context(_) = hline {
                  // Context line mismatch — file content differs from patch
                  if file_line.trim() == text.trim() {
                    // Whitespace-only difference, accept it
                    result.push(file_line.to_string());
                  } else {
                    return Err(format!(
                      "Context mismatch at line {}: expected {:?}, got {:?}",
                      hunk_file_pos + 1,
                      text,
                      file_line
                    ));
                  }
                } else {
                  // Remove line mismatch
                  return Err(format!(
                    "Line mismatch at line {}: expected {:?}, got {:?}",
                    hunk_file_pos + 1,
                    text,
                    file_line
                  ));
                }
              } else {
                // For context lines, keep the original file line
                if let HunkLine::Context(_) = hline {
                  result.push(file_line.to_string());
                }
                // For Remove lines, we simply don't emit anything (line is deleted)
              }
            } else if let HunkLine::Context(_) = hline {
              result.push(text.to_string());
            }
            hunk_file_pos += 1;
          }
          HunkLine::Add(text) => {
            result.push(text.to_string());
          }
        }
      }

      file_pos = hunk_file_pos;
    }

    // Append any remaining lines after the last hunk
    if file_pos < lines.len() {
      for line in &lines[file_pos..] {
        result.push(line.to_string());
      }
    }

    // Preserve trailing newline if the original had one
    let mut output = result.join("\n");
    if content.ends_with('\n') && !output.ends_with('\n') {
      output.push('\n');
    }

    Ok(output)
  }
}

impl Tool for ApplyPatchTool {
  fn name(&self) -> &str {
    "apply_patch"
  }

  fn handle(
    &self,
    tool_call_args: &Value,
  ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let file_path = tool_call_args
      .get("file_path")
      .and_then(|v| v.as_str())
      .ok_or("Missing 'file_path' argument")?;

    let patch = tool_call_args
      .get("patch")
      .and_then(|v| v.as_str())
      .ok_or("Missing 'patch' argument")?;

    // Read the target file
    let content = std::fs::read_to_string(file_path)
      .map_err(|e| format!("Failed to read file '{}': {}", file_path, e))?;

    // Parse and apply the patch
    let hunks = Self::parse_hunks(patch)?;
    let new_content = Self::apply_hunks(&content, &hunks)?;

    // Write the patched content back
    std::fs::write(file_path, &new_content)
      .map_err(|e| format!("Failed to write file '{}': {}", file_path, e))?;

    Ok(format!("Successfully applied patch to '{}'", file_path))
  }

  fn json_schema(&self) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
    Ok(json!({
      "type": "function",
      "function": {
        "name": "apply_patch",
        "description": "Applies a unified diff patch to a file. The patch should use standard unified diff format with @@ hunk headers, '-' for removed lines, '+' for added lines, and ' ' for context lines.",
        "parameters": {
          "type": "object",
          "properties": {
            "file_path": {
              "type": "string",
              "description": "Absolute path to the file to patch"
            },
            "patch": {
              "type": "string",
              "description": "Unified diff patch content to apply"
            }
          },
          "required": ["file_path", "patch"]
        }
      }
    }))
  }
}

// ------------------------------------------------------------------
//  Header parsing helpers
// ------------------------------------------------------------------

/// Parse a hunk header like "-3,5 +10,7" into (old_start, old_lines).
fn parse_hunk_header(header: &str) -> Result<(usize, usize), String> {
  // Split on ' +' to separate old and new ranges
  let parts: Vec<&str> = header.splitn(2, " +").collect();
  if parts.len() < 2 {
    return Err(format!("Invalid hunk header: {}", header));
  }

  let old_part = parts[0].trim_start_matches('-');
  let (start, count) = parse_range(old_part)?;
  Ok((start, count))
}

/// Parse a range like "3,5" or "3" into (start, count).
pub fn parse_range(s: &str) -> Result<(usize, usize), String> {
  let parts: Vec<&str> = s.splitn(2, ',').collect();
  match parts.as_slice() {
    [start] => {
      let n: usize = start
        .parse()
        .map_err(|_| format!("Invalid range start: {}", start))?;
      Ok((n, 1))
    }
    [start, count] => {
      let s: usize = start
        .parse()
        .map_err(|_| format!("Invalid range start: {}", start))?;
      let c: usize = count
        .parse()
        .map_err(|_| format!("Invalid range count: {}", count))?;
      Ok((s, c))
    }
    _ => Err(format!("Invalid range: {}", s)),
  }
}
