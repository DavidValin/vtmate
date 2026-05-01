// ------------------------------------------------------------------
//  Tool: Bash Command
// ------------------------------------------------------------------

use super::Tool;
use serde_json::Value;
use serde_json::json;

// API
// ------------------------------------------------------------------

pub struct BashCommandTool;

// PRIVATE
// ------------------------------------------------------------------

impl BashCommandTool {
  pub fn new() -> Self {
    BashCommandTool
  }
}

impl Tool for BashCommandTool {
  fn name(&self) -> &str {
    "bash_command"
  }

  fn handle(
    &self,
    tool_call_args: &Value,
  ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let command = tool_call_args
      .get("command")
      .and_then(|v| v.as_str())
      .ok_or("Missing 'command' argument")?;

    // Block the dangerous "rm" command
    let trimmed = command.trim();
    if trimmed == "rm" || trimmed.starts_with("rm ") || trimmed.starts_with("rm\t") {
      return Err("The 'rm' command is not allowed via this tool".into());
    }

    // Get the process working directory
    let cwd =
      std::env::current_dir().map_err(|e| format!("Failed to get current directory: {}", e))?;
    let cwd_str = cwd.to_string_lossy();

    // Prepend cd into the working directory so the command runs there
    let full_command = format!("cd {} && {}", cwd_str, command);

    let output = std::process::Command::new("bash")
      .arg("-c")
      .arg(&full_command)
      .output()
      .map_err(|e| format!("Failed to execute command: {}", e))?;

    if output.status.success() {
      let stdout = String::from_utf8_lossy(&output.stdout).to_string();
      Ok(stdout)
    } else {
      let stderr = String::from_utf8_lossy(&output.stderr).to_string();
      Err(
        format!(
          "Command exited with code {}:\n{}",
          output
            .status
            .code()
            .map(|c| c.to_string())
            .unwrap_or("?".to_string()),
          stderr,
        )
        .into(),
      )
    }
  }

  fn json_schema() -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
    let cwd = std::env::current_dir()
      .map(|d| d.to_string_lossy().to_string())
      .unwrap_or_else(|_| "?".to_string());

    Ok(json!({
      "type": "function",
      "function": {
        "name": "bash_command",
        "description": "Executes a bash command on the host system",
        "parameters": {
          "type": "object",
          "properties": {
            "command": {
              "type": "string",
              "description": "The bash command to execute"
            },
            "working_dir": {
              "type": "string",
              "description": "Working directory where the command will run",
              "const": cwd
            }
          },
          "required": [
            "command"
          ]
        }
      }
    }))
  }
}
