// ------------------------------------------------------------------
//  LLM CLI integrations
//
//  A second way to reach a provider: instead of an api key over HTTP (see
//  `llm.rs`), shell out to that provider's own CLI and let it use whatever
//  the user is already logged into (a Claude Pro/Max, ChatGPT, Gemini,
//  Copilot, Amazon Q/Kiro, Mistral, Nous Portal, xAI subscription, ...).
//  vtmate only ever wants the model's prose answer for TTS, so every
//  invocation asks for a single non-interactive turn with tool use turned
//  off (or as close to off as that CLI allows) - no file edits, no shell
//  commands, just an answer.
//
//  Each CLI is a fresh subprocess per turn: none of them are driven as a
//  stateful chat session here, so the whole conversation (system prompt and
//  prior turns) is flattened into one prompt string per call, the same way
//  a fresh browser tab would paste in the transcript so far.
//
//  How much of each CLI's own output format is *verified* varies a lot -
//  see the comment on `Spec` below - so every parser fails safe: a line it
//  cannot make sense of is dropped rather than spoken, and if a whole turn
//  produces no text at all that is reported as an error instead of silence.
// ------------------------------------------------------------------

use serde_json::Value;
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};

// API
// ------------------------------------------------------------------

/// Provider keys shown in the settings popup and accepted in the settings
/// file, one per integrated CLI.
pub const CLI_PROVIDERS: &[&str] = &[
  "claude-cli",
  "codex-cli",
  "gemini-cli",
  "copilot-cli",
  "kiro-cli",
  "vibe-cli",
  "hermes-cli",
  "opencode-cli",
  "pi-cli",
  "aichat-cli",
  "grok-cli",
];

pub fn is_cli_provider(provider: &str) -> bool {
  CLI_PROVIDERS.contains(&provider.trim().to_lowercase().as_str())
}

/// Shown in the settings form next to each cli provider's label, and used to
/// name the binary vtmate actually runs (`kiro-cli` for the "amazon q"
/// integration: Amazon Q Developer CLI is end of life and AWS's replacement,
/// same maker, is `kiro-cli`).
pub fn binary_for(provider: &str) -> Option<&'static str> {
  spec(&provider.trim().to_lowercase()).map(|s| s.binary)
}

/// Human hint appended to error logs, the cli equivalent of the api-key
/// hosted providers' hint in `llm.rs`.
pub fn cli_hint(provider: &str) -> String {
  match spec(provider) {
    Some(s) => format!(
      "Make sure '{}' is installed, on PATH, and logged in (run it once by hand to log in)",
      s.binary
    ),
    None => "Check that the cli is installed and logged in".to_string(),
  }
}

/// Whether `provider`'s cli binary is actually on PATH right now - not
/// whether it is logged in, just whether it exists to run at all.
pub fn is_installed(provider: &str) -> bool {
  match spec(&provider.trim().to_lowercase()) {
    Some(s) => !which_missing(s.binary),
    None => false,
  }
}

/// Whether `provider` has a real model-listing command of its own. Several
/// clis have no such command at all (checked against their own `--help` or
/// documentation - see `spec`); for those, `static_models` seeds a cyclable
/// starting point instead, and the Model field stays freely typable either
/// way so any other id can still be entered directly.
pub fn has_model_listing(provider: &str) -> bool {
  spec(&provider.trim().to_lowercase())
    .map(|s| s.list_cmd.is_some())
    .unwrap_or(false)
}

/// A curated starting point for ←/→ to cycle through, for the clis that
/// have no listing command of their own (empty otherwise). Not a claim
/// that these are the only models the cli accepts - see `Spec::static_models`.
pub fn static_models(provider: &str) -> &'static [&'static str] {
  spec(&provider.trim().to_lowercase())
    .map(|s| s.static_models)
    .unwrap_or(&[])
}

/// Retrieve the models a cli reports, blocking with a short timeout so the
/// settings popup never hangs on a cli that stalls (e.g. waiting on a login
/// prompt it cannot show non-interactively). Only meaningful when
/// `has_model_listing` is true; there is no invented fallback list for the
/// clis that have no listing command of their own.
pub fn list_models(provider: &str) -> Result<Vec<String>, String> {
  let provider = provider.trim().to_lowercase();
  let Some(spec) = spec(&provider) else {
    return Err(format!("unknown cli provider '{}'", provider));
  };
  if which_missing(spec.binary) {
    return Err(format!("'{}' is not installed or not on PATH", spec.binary));
  }
  let Some(list_args) = spec.list_cmd else {
    return Err(format!(
      "'{}' has no command to list its models; type the model name",
      spec.binary
    ));
  };

  let (tx, rx) = std::sync::mpsc::channel();
  let binary = spec.binary;
  let args: Vec<String> = list_args.iter().map(|s| s.to_string()).collect();
  std::thread::spawn(move || {
    let out = std::process::Command::new(binary)
      .args(&args)
      .stdin(Stdio::null())
      .output();
    let _ = tx.send(out);
  });
  let output = match rx.recv_timeout(Duration::from_secs(15)) {
    Ok(Ok(o)) => o,
    Ok(Err(e)) => return Err(format!("running '{} {}': {}", binary, list_args.join(" "), e)),
    Err(_) => return Err(format!("'{}' timed out listing models", binary)),
  };
  if !output.status.success() {
    return Err(first_error_line(&output.stdout, &output.stderr));
  }
  let stdout = String::from_utf8_lossy(&output.stdout);
  let models: Vec<String> = stdout
    .lines()
    .map(|l| l.trim())
    .filter(|l| !l.is_empty() && !l.starts_with('#'))
    .map(|l| l.to_string())
    .collect();
  if models.is_empty() {
    return Err(format!("'{}' reported no models (log in first?)", binary));
  }
  // a login/setup notice printed one-line-per-sentence would otherwise be
  // mistaken for a list of models; treat it as the error it actually is
  if models.iter().any(|m| looks_like_login_error(m)) {
    return Err(models.join(" "));
  }
  Ok(models)
}

/// Stream one non-interactive turn from a cli provider into `on_piece`, the
/// cli equivalent of `llm::stream_response_into`'s http path. Cancels the
/// subprocess (rather than merely dropping the connection) when interrupted.
pub async fn stream_cli_response_into(
  messages: &[crate::conversation::ChatMessage],
  target: &crate::llm::LlmTarget,
  interrupt_counter: Arc<AtomicU64>,
  expected_interrupt: u64,
  on_piece: &mut dyn FnMut(&str),
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
  let provider = target.provider.trim().to_lowercase();
  let Some(spec) = spec(&provider) else {
    return Err(format!("Unsupported cli provider '{}'", target.provider).into());
  };
  if which_missing(spec.binary) {
    return Err(format!("'{}' is not installed or not on PATH", spec.binary).into());
  }

  let (system, transcript) = flatten_prompt(messages);
  let args = chat_args(&provider, target.model.trim(), system.as_deref(), &transcript);

  let mut cmd = tokio::process::Command::new(spec.binary);
  cmd
    .args(&args)
    .stdin(Stdio::null())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .kill_on_drop(true);
  if provider == "vibe-cli" {
    // vibe has no --model flag; the active model is only settable through
    // config.toml or this env var
    cmd.env("VIBE_ACTIVE_MODEL", target.model.trim());
  }

  let mut child = cmd
    .spawn()
    .map_err(|e| format!("Failed to start '{}': {}", spec.binary, e))?;
  let stdout = child.stdout.take().expect("piped stdout");
  let stderr = child.stderr.take().expect("piped stderr");

  let mut out_lines = BufReader::new(stdout).lines();
  let stderr_buf = Arc::new(std::sync::Mutex::new(String::new()));
  let stderr_buf_task = stderr_buf.clone();
  let stderr_task = tokio::spawn(async move {
    let mut lines = BufReader::new(stderr).lines();
    while let Ok(Some(line)) = lines.next_line().await {
      if let Ok(mut buf) = stderr_buf_task.lock() {
        buf.push_str(&line);
        buf.push('\n');
      }
    }
  });

  let interrupted =
    || interrupt_counter.load(Ordering::SeqCst) != expected_interrupt;
  // agentic clis can sit quietly for a while before their first token,
  // longer than the plain http path's stall timeout allows for
  let stall_timeout = Duration::from_secs(180);
  let poll_interval = Duration::from_millis(250);
  let mut since_last = Duration::ZERO;
  let mut got_any_text = false;
  let mut hard_error: Option<String> = None;
  let mut dedupe: HashMap<String, usize> = HashMap::new();

  loop {
    if interrupted() {
      let _ = child.start_kill();
      let _ = child.wait().await;
      stderr_task.abort();
      return Ok(());
    }
    match tokio::time::timeout(poll_interval, out_lines.next_line()).await {
      Ok(Ok(Some(line))) => {
        since_last = Duration::ZERO;
        match parse_line(spec.output, &line, &mut dedupe) {
          LineResult::Piece(p) => {
            if !p.is_empty() {
              got_any_text = true;
              on_piece(&p);
            }
          }
          LineResult::Error(e) => hard_error = Some(e),
          LineResult::Ignore => {}
        }
      }
      Ok(Ok(None)) => break,
      Ok(Err(e)) => {
        hard_error = Some(format!("reading {} output: {}", spec.binary, e));
        break;
      }
      Err(_) => {
        since_last += poll_interval;
        if since_last >= stall_timeout {
          let _ = child.start_kill();
          let _ = child.wait().await;
          stderr_task.abort();
          return Err(
            format!(
              "{} produced no output for {}s",
              spec.binary,
              stall_timeout.as_secs()
            )
            .into(),
          );
        }
      }
    }
  }

  let status = child
    .wait()
    .await
    .map_err(|e| format!("waiting for {}: {}", spec.binary, e))?;
  let _ = stderr_task.await;
  let stderr_text = stderr_buf.lock().map(|b| b.clone()).unwrap_or_default();

  if let Some(e) = hard_error {
    return Err(friendly_cli_error(&provider, &e, &stderr_text).into());
  }
  if !status.success() {
    let raw = first_error_line(&[], stderr_text.as_bytes());
    return Err(friendly_cli_error(&provider, &raw, &stderr_text).into());
  }
  if !got_any_text {
    let raw = if stderr_text.trim().is_empty() {
      "produced no answer text".to_string()
    } else {
      first_error_line(&[], stderr_text.as_bytes())
    };
    return Err(friendly_cli_error(&provider, &raw, &stderr_text).into());
  }
  Ok(())
}

// PRIVATE
// ------------------------------------------------------------------

/// How a cli's stdout is read for the spoken answer.
#[derive(Clone, Copy, PartialEq, Eq)]
enum OutputKind {
  /// Directly tested against a real `claude` install: `--output-format
  /// stream-json --include-partial-messages` gives one JSON object per line,
  /// `content_block_delta`/`text_delta` events carry token-by-token text,
  /// and a final `result` line with `is_error: true` carries a clean message
  /// such as "Not logged in - Please run /login".
  ClaudeStreamJson,
  /// `codex exec --json` gives JSON-lines events; parsed against the
  /// documented `item.completed` / `turn.failed` event names (OpenAI's
  /// established convention for its agent clis), not directly tested here.
  CodexJson,
  /// Directly tested against a real `opencode` install: `--format json`
  /// gives JSON-lines events, a `"type":"text"` event carrying the answer
  /// under `part.text`.
  OpencodeJson,
  /// `--mode json` gives JSON-lines events per pi's own help text, but its
  /// exact schema was not directly observed (needs a logged-in account);
  /// parsed with a handful of the field names agent clis commonly use, and
  /// any line that matches none of them is silently dropped rather than
  /// spoken.
  PiJson,
  /// No structured output mode used at all: whatever the cli prints to
  /// stdout in its default/plain mode is treated as the answer verbatim,
  /// including any login-needed message it might print instead.
  PlainText,
}

struct Spec {
  binary: &'static str,
  output: OutputKind,
  /// Args (after the binary) that print one model id per line, when the cli
  /// has a real listing command. `None` for a cli with no such command; the
  /// Model field is then a cyclable but still freely typable input seeded
  /// from `static_models` instead of a picker fed by a live fetch.
  list_cmd: Option<&'static [&'static str]>,
  /// Seed models offered by ←/→ when `list_cmd` is `None` - a starting
  /// point since there is no way to ask the cli itself, not a claim that
  /// these are the only models it accepts; the field stays editable so any
  /// other model id can be typed directly. Empty when `list_cmd` is `Some`,
  /// unused there.
  static_models: &'static [&'static str],
}

fn spec(provider: &str) -> Option<Spec> {
  Some(match provider {
    "claude-cli" => Spec {
      binary: "claude",
      output: OutputKind::ClaudeStreamJson,
      list_cmd: None,
      static_models: &[
        "sonnet",
        "opus",
        "haiku",
        "fable",
        "claude-opus-4-7",
        "claude-sonnet-4-6",
        "claude-haiku-4-5",
      ],
    },
    "codex-cli" => Spec {
      binary: "codex",
      output: OutputKind::CodexJson,
      list_cmd: None,
      static_models: &["gpt-5.1-codex", "gpt-5.1", "gpt-5-codex", "o3"],
    },
    "gemini-cli" => Spec {
      binary: "gemini",
      output: OutputKind::PlainText,
      list_cmd: None,
      static_models: &[
        "gemini-3-pro",
        "gemini-3-flash",
        "gemini-2.5-pro",
        "gemini-2.5-flash",
      ],
    },
    "copilot-cli" => Spec {
      binary: "copilot",
      output: OutputKind::PlainText,
      list_cmd: None,
      static_models: &[
        "claude-sonnet-4.5",
        "claude-sonnet-4",
        "claude-opus-4.5",
        "gpt-5",
      ],
    },
    "kiro-cli" => Spec {
      binary: "kiro-cli",
      output: OutputKind::PlainText,
      list_cmd: None,
      static_models: &["claude-sonnet-4.6", "claude-opus-4.7"],
    },
    "vibe-cli" => Spec {
      binary: "vibe",
      output: OutputKind::PlainText,
      list_cmd: None,
      static_models: &["mistral-medium-3.5", "mistral-large-3", "codestral-3"],
    },
    "hermes-cli" => Spec {
      binary: "hermes",
      output: OutputKind::PlainText,
      list_cmd: None,
      static_models: &["anthropic/claude-sonnet-4.6", "hermes-4-405b", "hermes-4-70b"],
    },
    "opencode-cli" => Spec {
      binary: "opencode",
      output: OutputKind::OpencodeJson,
      list_cmd: Some(&["models"]),
      static_models: &[],
    },
    "pi-cli" => Spec {
      binary: "pi",
      output: OutputKind::PiJson,
      list_cmd: Some(&["--list-models"]),
      static_models: &[],
    },
    "aichat-cli" => Spec {
      binary: "aichat",
      output: OutputKind::PlainText,
      list_cmd: Some(&["--list-models"]),
      static_models: &[],
    },
    "grok-cli" => Spec {
      binary: "grok",
      output: OutputKind::PlainText,
      list_cmd: None,
      static_models: &["grok-4.6", "grok-build-0.1"],
    },
    _ => return None,
  })
}

/// The argv (after the binary) for one non-interactive turn: the prompt,
/// the model, and each cli's closest thing to "answer only, don't touch
/// anything" - see the module doc.
///
/// `system`, when the agent has one, goes through a cli's own system-prompt
/// flag where one is confirmed to exist (claude, pi, aichat). A cli without
/// one gets it prepended to the prompt text as plain context instead - not
/// under a label like "Instructions:", which reads as an injected system
/// directive smuggled into user text; claude itself refused to treat a
/// prompt built that way as real for exactly that reason before this.
fn chat_args(provider: &str, model: &str, system: Option<&str>, transcript: &str) -> Vec<String> {
  let m = model.to_string();
  let system = system.map(str::trim).filter(|s| !s.is_empty());
  let combined = || match system {
    Some(s) => format!("{}\n\n{}", s, transcript),
    None => transcript.to_string(),
  };
  match provider {
    "claude-cli" => {
      let mut args = vec![
        "-p".to_string(),
        transcript.to_string(),
        "--model".to_string(),
        m,
        "--output-format".to_string(),
        "stream-json".to_string(),
        "--include-partial-messages".to_string(),
        "--verbose".to_string(),
        "--no-session-persistence".to_string(),
        "--permission-mode".to_string(),
        "plan".to_string(),
      ];
      if let Some(s) = system {
        // `--system-prompt` replaces the session's prompt outright; the
        // similarly named `--append-system-prompt` only adds to Claude
        // Code's own default coding-assistant persona, which is not what an
        // agent's system prompt is meant to do here
        args.push("--system-prompt".to_string());
        args.push(s.to_string());
      }
      args
    }
    "codex-cli" => vec![
      "exec".into(),
      "-m".into(),
      m,
      "--json".into(),
      "--skip-git-repo-check".into(),
      "--sandbox".into(),
      "read-only".into(),
      combined(),
    ],
    "gemini-cli" => vec![
      "-p".into(),
      combined(),
      "-m".into(),
      m,
      "--approval-mode".into(),
      "plan".into(),
    ],
    "copilot-cli" => vec![
      "-p".into(),
      combined(),
      "--model".into(),
      m,
      "--available-tools=".into(),
      "--allow-all-tools".into(),
      "-s".into(),
    ],
    "kiro-cli" => vec![
      "chat".into(),
      combined(),
      "--no-interactive".into(),
      "--model".into(),
      m,
    ],
    "vibe-cli" => vec![
      "-p".into(),
      combined(),
      "--output".into(),
      "text".into(),
      "--agent".into(),
      "plan".into(),
      "--trust".into(),
    ],
    "hermes-cli" => vec!["-z".into(), combined(), "--model".into(), m],
    "opencode-cli" => vec![
      "run".into(),
      "-m".into(),
      m,
      "--format".into(),
      "json".into(),
      combined(),
    ],
    "pi-cli" => {
      let mut args = vec![
        "-p".to_string(),
        "--mode".to_string(),
        "json".to_string(),
        "--model".to_string(),
        m,
        "--no-tools".to_string(),
        "--no-session".to_string(),
      ];
      if let Some(s) = system {
        // replaces pi's default coding-assistant persona rather than
        // appending to it, matching what vtmate's system prompt is for
        args.push("--system-prompt".to_string());
        args.push(s.to_string());
      }
      args.push(transcript.to_string());
      args
    }
    "aichat-cli" => {
      let mut args = vec!["-m".to_string(), m];
      if let Some(s) = system {
        args.push("--prompt".to_string());
        args.push(s.to_string());
      }
      args.push(transcript.to_string());
      args
    }
    "grok-cli" => vec!["-p".into(), combined(), "--model".into(), m],
    _ => vec![combined()],
  }
}

enum LineResult {
  Piece(String),
  Error(String),
  Ignore,
}

fn parse_line(kind: OutputKind, line: &str, dedupe: &mut HashMap<String, usize>) -> LineResult {
  let line = line.trim_end();
  if line.is_empty() {
    return LineResult::Ignore;
  }
  match kind {
    OutputKind::PlainText => LineResult::Piece(format!("{} ", line)),

    OutputKind::ClaudeStreamJson => {
      let Ok(v) = serde_json::from_str::<Value>(line) else {
        return LineResult::Ignore;
      };
      match v.get("type").and_then(|t| t.as_str()) {
        Some("stream_event") => {
          if let Some(delta) = v.pointer("/event/delta") {
            if delta.get("type").and_then(|t| t.as_str()) == Some("text_delta") {
              if let Some(text) = delta.get("text").and_then(|t| t.as_str()) {
                return LineResult::Piece(text.to_string());
              }
            }
          }
          LineResult::Ignore
        }
        Some("result") => {
          if v.get("is_error").and_then(|b| b.as_bool()).unwrap_or(false) {
            let msg = v
              .get("result")
              .and_then(|r| r.as_str())
              .unwrap_or("claude returned an error");
            LineResult::Error(msg.to_string())
          } else {
            LineResult::Ignore
          }
        }
        _ => LineResult::Ignore,
      }
    }

    OutputKind::CodexJson => {
      let Ok(v) = serde_json::from_str::<Value>(line) else {
        return LineResult::Ignore;
      };
      match v.get("type").and_then(|t| t.as_str()) {
        Some("item.completed") => {
          let item = v.get("item");
          let is_message =
            item.and_then(|i| i.get("type")).and_then(|t| t.as_str()) == Some("agent_message");
          if is_message {
            if let Some(text) = item.and_then(|i| i.get("text")).and_then(|t| t.as_str()) {
              return LineResult::Piece(text.to_string());
            }
          }
          LineResult::Ignore
        }
        Some("turn.failed") => {
          let msg = v
            .pointer("/error/message")
            .and_then(|m| m.as_str())
            .or_else(|| v.get("message").and_then(|m| m.as_str()))
            .unwrap_or("codex turn failed");
          LineResult::Error(msg.to_string())
        }
        _ => LineResult::Ignore,
      }
    }

    OutputKind::OpencodeJson => {
      let Ok(v) = serde_json::from_str::<Value>(line) else {
        return LineResult::Ignore;
      };
      if v.get("type").and_then(|t| t.as_str()) != Some("text") {
        return LineResult::Ignore;
      }
      let part = v.get("part");
      let id = part
        .and_then(|p| p.get("id"))
        .and_then(|i| i.as_str())
        .unwrap_or("")
        .to_string();
      let text = part
        .and_then(|p| p.get("text"))
        .and_then(|t| t.as_str())
        .unwrap_or("");
      grow_dedupe(dedupe, id, text)
    }

    OutputKind::PiJson => {
      let Ok(v) = serde_json::from_str::<Value>(line) else {
        return LineResult::Ignore;
      };
      if v.get("type").and_then(|t| t.as_str()) == Some("error") {
        let msg = v
          .get("message")
          .and_then(|m| m.as_str())
          .unwrap_or("pi returned an error");
        return LineResult::Error(msg.to_string());
      }
      let text = v
        .get("text")
        .and_then(|t| t.as_str())
        .or_else(|| v.pointer("/delta/text").and_then(|t| t.as_str()))
        .or_else(|| v.pointer("/part/text").and_then(|t| t.as_str()))
        .or_else(|| v.pointer("/message/content").and_then(|t| t.as_str()));
      let Some(text) = text else {
        return LineResult::Ignore;
      };
      let id = v
        .get("id")
        .and_then(|i| i.as_str())
        .or_else(|| v.pointer("/part/id").and_then(|i| i.as_str()))
        .unwrap_or("")
        .to_string();
      grow_dedupe(dedupe, id, text)
    }
  }
}

/// Several of the JSON event streams repeat the same growing string under
/// the same id as it is generated. Emit only the newly appended suffix, or
/// the whole thing once if it only ever arrives complete.
fn grow_dedupe(dedupe: &mut HashMap<String, usize>, id: String, text: &str) -> LineResult {
  let seen = dedupe.entry(id).or_insert(0);
  if text.len() > *seen {
    let piece = text[*seen..].to_string();
    *seen = text.len();
    LineResult::Piece(piece)
  } else {
    LineResult::Ignore
  }
}

fn flatten_prompt(messages: &[crate::conversation::ChatMessage]) -> (Option<String>, String) {
  let mut system_parts: Vec<String> = Vec::new();
  let mut transcript = String::new();
  for m in messages {
    let content = m.content.trim();
    if content.is_empty() {
      continue;
    }
    match m.role.as_str() {
      "system" => system_parts.push(content.to_string()),
      "assistant" => {
        transcript.push_str("Assistant: ");
        transcript.push_str(content);
        transcript.push_str("\n\n");
      }
      _ => {
        transcript.push_str("User: ");
        transcript.push_str(content);
        transcript.push_str("\n\n");
      }
    }
  }
  let system = if system_parts.is_empty() {
    None
  } else {
    Some(system_parts.join("\n"))
  };
  (system, transcript.trim_end().to_string())
}

fn which_missing(binary: &str) -> bool {
  let finder = if cfg!(windows) { "where" } else { "which" };
  std::process::Command::new(finder)
    .arg(binary)
    .stdout(Stdio::null())
    .stderr(Stdio::null())
    .status()
    .map(|s| !s.success())
    .unwrap_or(true)
}

fn looks_like_login_error(text: &str) -> bool {
  let t = text.to_lowercase();
  [
    "not logged in",
    "please log in",
    "please login",
    "/login",
    "not authenticated",
    "authentication required",
    "no credentials",
    "unauthorized",
    "no models available",
    "log in with",
    "auth required",
    "please run",
  ]
  .iter()
  .any(|needle| t.contains(needle))
}

fn first_error_line(stdout: &[u8], stderr: &[u8]) -> String {
  let stderr = String::from_utf8_lossy(stderr);
  let stdout = String::from_utf8_lossy(stdout);
  let text = if !stderr.trim().is_empty() { &stderr } else { &stdout };
  text
    .lines()
    .map(|l| l.trim())
    .find(|l| !l.is_empty())
    .unwrap_or("no output")
    .to_string()
}

fn friendly_cli_error(provider: &str, primary: &str, stderr_text: &str) -> String {
  let primary = primary.trim();
  let extra_context = stderr_text
    .lines()
    .map(|l| l.trim())
    .find(|l| !l.is_empty() && *l != primary);
  match extra_context {
    Some(extra) => format!("{}: {} ({})", provider, primary, extra),
    None => format!("{}: {}", provider, primary),
  }
}
