// ------------------------------------------------------------------
//  Config
// ------------------------------------------------------------------

use crate::tts;
use crate::util::get_user_home_path;
use crate::util::terminate;
use anyhow::Error;
use clap::Parser;
use cpal::Device;
use cpal::traits::DeviceTrait;
use serde::{Deserialize, Serialize};
use serde_ini::from_str;
use std::collections::HashMap;
use std::fs::{File, create_dir_all, read_to_string};
use std::io::Write;
use std::panic;
use std::thread::{self};
use std::time::Duration;
use url::Url;

// API
// ------------------------------------------------------------------

#[derive(Debug, Deserialize, Serialize, Clone, Default, PartialEq)]
pub struct AgentSettings {
  pub name: String,
  pub language: String,
  pub tts: String,
  pub voice: String,
  pub provider: String,
  pub baseurl: String,
  pub model: String,
  #[serde(default)]
  pub api_key: String,
  pub system_prompt: String,
  #[serde(deserialize_with = "bool_from_str_or_bool")]
  pub ptt: bool,
  pub whisper_model_path: String,
  pub sound_threshold_peak: f32,
  pub end_silence_ms: u64,
  pub voice_speed: f32,
  /// Name of the `[system_prompt]` block this prompt was pulled from, when
  /// the agent wrote `system_prompt = @<name>`. `save_settings` writes the
  /// prompt back under the same name instead of inventing a new one. Never
  /// read from, nor written to, the file itself.
  #[serde(skip)]
  pub system_prompt_name: Option<String>,
}

#[derive(Parser, Debug, Clone)]
#[clap(version = env!("CARGO_PKG_VERSION"))]
#[command(group(clap::ArgGroup::new("daemon_cmd").multiple(false)))]
#[clap(after_help = r#"
Settings file is at ~/.vtmate/settings

The file starts with a [general] section, then a [daemon]
section, then one [agent] section per agent. Press Ctrl+S
during a conversation to edit the agents from the terminal
instead: what you save is written back to this file and
applies straight away.

[general]
  * selected_agent:       name of the agent vtmate starts with.
                          Updated automatically every time you
                          switch agents with LEFT/RIGHT (in the
                          terminal or while attached to the
                          daemon). `-a` overrides it for one run.

[daemon]  (global shortcuts used by `vtmate --daemon`)
  * llm_background_ptt_combo:          hold to talk; on release the
                                       speech plus any selected text
                                       is sent to the agent and the
                                       reply is spoken (ctrl+alt+a)
  * tts_background_combo:              read the selected text aloud;
                                       press again to stop (ctrl+alt+r)
  * stt_and_paste_background_ptt_combo: hold to talk; on release the
                                       transcript is pasted at the
                                       cursor (ctrl+alt+s)
  * llm_background_reset:              stop the speech; twice in a row
                                       resets the conversation (ctrl+q)
  Combos are written as modifiers joined by '+', e.g. ctrl+alt+a,
  shift+f5, cmd+alt+r (modifiers: ctrl, alt/option, shift,
  cmd/super, cmdorctrl).

[system_prompt]  (optional, as many as you want)
  A named multiline system prompt that agents pull in with
  `system_prompt = @<name>`. The block holds a `name` and then
  the prompt body fenced between two lines of three or more
  dashes:

      [system_prompt]
      name = planner
      ---
      You assist the user in the creation of a plan.

      Follow these format standards:
        1. The plan is composed by tasks and subtasks.
        2. Each task has the format: "[ ] <task name>".
      ---

  The body is taken verbatim: blank lines, indentation and
  lines starting with '[' are kept, and no \n escape is
  expanded (it already has real new lines). Close a body that
  itself contains '---' with a longer fence ('----').

Explanation on the [agent] fields:

  * name:                 a short name for the agent
  ------------------------------------------------------------
  * language:             any of the languages available used
                          for speech recognition and tts
  ------------------------------------------------------------
  * voice:                the voice name to use by the
                          agent (see available voices for each
                          language and tts system running
                          `vtmate --list-voices`).

                          Voice mixing:

                            when using 'kokoro' tts you can mix
                            2 voices. example:
                            
                               "bm_daniel.5+am_puck.5"

                            (50% of bm_daniel and 50% of am_puck)
                            
  ------------------------------------------------------------
  * voice_speed:          the voice speed from 1.0 to 9.0
  ------------------------------------------------------------
  * provider:             the system it will use to query
                          the llm.

                          Local servers (no api key needed):
                            'ollama' (0.13 or newer),
                            'llama-server',
                            'openai-compatible' (LM Studio,
                            vLLM or any server exposing
                            /v1/chat/completions)

                          Hosted providers (api key needed):
                            'openai', 'anthropic', 'google',
                            'groq', 'mistral', 'openrouter',
                            'deepseek', 'xai'
  ------------------------------------------------------------
  * baseurl:              the base url used to contact the
                          provider. For local servers it is
                          the host and port without path, e.g.
                          http://127.0.0.1:11434
                          For hosted providers leave it empty
                          to use the provider's default url.
  ------------------------------------------------------------
  * model:                the model name to use
                          (some llama-server versions will
                          ignore this option as llama-server
                          runs for a single model)
  ------------------------------------------------------------
  * api_key:              the api key for hosted providers.
                          Optional: when empty, the provider's
                          environment variable is used instead
                          (OPENAI_API_KEY, ANTHROPIC_API_KEY,
                          GOOGLE_API_KEY, GROQ_API_KEY,
                          MISTRAL_API_KEY, OPENROUTER_API_KEY,
                          DEEPSEEK_API_KEY, XAI_API_KEY)
  ------------------------------------------------------------
  * system_prompt:        the system prompt to be sent to
                          the llm when querying it.
                          Use \n for new lines.
                          Write `@<name>` instead to use a
                          [system_prompt] block, which keeps
                          its new lines as written (start an
                          inline prompt with `@@` for a
                          literal '@').
  ------------------------------------------------------------
  * sound_threshold_peak: a value between 0 and 1 which will
                          be used as a peak base to detect
                          user speech
  ------------------------------------------------------------
  * end_silence_ms:       the milliseconds of silence below
                          sound_threshold_peak level that
                          have to elapse for user speech
                          to be submitted.
                          in ptt mode, this option is ignored,
                          the program will wait for SPACE key
                          to be released to submit the audio.
  ------------------------------------------------------------
  * tts:                  the tts system to use, it can be
                          'supertonic' (default, 31 languages),
                          'supersonic2', 'kokoro' or 'opentts'.

                            - opentts requires opentts docker
                            container to be running:
                            docker run -p 5500:5500 synesthesiam/opentts:all
  ------------------------------------------------------------
  * ptt:                  push to talk mode, when its set
                          to true you have to keep the space
                          pushed while speaking, then release.
  ------------------------------------------------------------
  * whisper_model_path:   the path to the whisper model.
                          vtmate unzips 2 models in
                          ~/.whisper-models: ggml-tiny.bin and
                          ggml-small-q5_1.bin (quantized small).
                          You can download bigger models and
                          point to them here

"#)]
pub struct Args {
  #[arg(
    short = 'p',
    long = "prompt",
    value_name = "PROMPT",
    help = "initialize with a text prompt"
  )]
  pub prompt: Option<String>,

  #[arg(
    short = 'i',
    long = "prompt-file",
    value_name = "FILE",
    default_missing_value = "-",
    help = "initialize with a file prompt (use '-' for STDIN (runs in quiet mode))"
  )]
  pub prompt_file: Option<String>,

  #[arg(long, action = clap::ArgAction::SetTrue, help = "run the program in verbose mode")]
  pub verbose: bool,

  #[arg(long, action=clap::ArgAction::SetTrue, help = "list all voices for all languages and tts systems")]
  pub list_voices: bool,

  #[arg(
    short = 'c',
    long = "config",
    value_name = "CONFIG_FILE",
    help = "use a specific settings file"
  )]
  pub config: Option<String>,

  #[arg(short = 'a', long = "agent", value_parser=validate_agent_name, help = "set a specific initial agent")]
  pub agent: Option<String>,

  #[arg(
    long,
    help = "override for this session the ptt setting for all agents independently of its settings"
  )]
  pub ptt: Option<bool>,

  #[arg(long, num_args=2.., value_name = "AGENT1 AGENT2 SUBJECT", help = "enable debate mode with two agents and a subject")]
  pub debate: Option<Vec<String>>,

  #[arg(
    short = 'r',
    long = "read-file",
    value_name = "FILENAME",
    default_missing_value = "-",
    help = "read a file with voice, phrase by phrase (no llm involved). Use '-' for STDIN (runs in quiet mode))"
  )]
  pub read_file: Option<String>,

  #[arg(short = 'q', long = "quiet", action = clap::ArgAction::SetTrue, help = "produce a single response and exit (requires `-p` or `-i`)")]
  pub quiet: bool,

  #[arg(short = 's', long = "save", action = clap::ArgAction::SetTrue, help = "save the conversation to text and audio file in ~/.vtmate/conversations")]
  pub save: bool,

  #[arg(
    long,
    action = clap::ArgAction::SetTrue,
    group = "daemon_cmd",
    conflicts_with_all = ["read_file", "quiet", "prompt", "prompt_file", "debate", "list_voices"],
    help = "start vtmate in the background (global hotkeys, voice only). Run `vtmate` again to attach"
  )]
  pub daemon: bool,

  #[arg(
    long,
    action = clap::ArgAction::SetTrue,
    group = "daemon_cmd",
    hide = true,
    conflicts_with_all = ["read_file", "quiet", "prompt", "prompt_file", "debate", "list_voices"]
  )]
  pub daemon_foreground: bool,

  #[arg(long, action = clap::ArgAction::SetTrue, group = "daemon_cmd", help = "stop the running daemon")]
  pub daemon_stop: bool,

  #[arg(long, action = clap::ArgAction::SetTrue, group = "daemon_cmd", help = "show whether a daemon is running and its hotkeys")]
  pub daemon_status: bool,
}

// internal static values
pub const HANGOVER_MS_DEFAULT: u64 = 300;
pub const MIN_UTTERANCE_MS_DEFAULT: u64 = 300;
pub const OPENTTS_BASE_URL_DEFAULT: &str = "http://127.0.0.1:5500/api/tts?&vocoder=high&denoiserStrength=0.005&&speakerId=&ssml=false&ssmlNumbers=true&ssmlDates=true&ssmlCurrency=true&cache=false";

pub const DAEMON_LLM_PTT_DEFAULT: &str = "ctrl+alt+a";
pub const DAEMON_TTS_DEFAULT: &str = "ctrl+alt+r";
pub const DAEMON_PASTE_PTT_DEFAULT: &str = "ctrl+alt+s";
pub const DAEMON_RESET_DEFAULT: &str = "ctrl+q";

/// `[general]` section of the settings file.
#[derive(Debug, Deserialize, Clone, Default)]
pub struct GeneralSettings {
  /// Agent vtmate starts with. Empty (or unknown) means the first agent.
  #[serde(default)]
  pub selected_agent: String,
}

/// `[daemon]` section of the settings file: the global shortcuts.
#[derive(Debug, Deserialize, Clone)]
pub struct DaemonSettings {
  #[serde(default = "d_llm_ptt")]
  pub llm_background_ptt_combo: String,
  #[serde(default = "d_tts")]
  pub tts_background_combo: String,
  #[serde(default = "d_paste_ptt")]
  pub stt_and_paste_background_ptt_combo: String,
  #[serde(default = "d_reset")]
  pub llm_background_reset: String,
}

fn d_llm_ptt() -> String {
  DAEMON_LLM_PTT_DEFAULT.to_string()
}
fn d_tts() -> String {
  DAEMON_TTS_DEFAULT.to_string()
}
fn d_paste_ptt() -> String {
  DAEMON_PASTE_PTT_DEFAULT.to_string()
}
fn d_reset() -> String {
  DAEMON_RESET_DEFAULT.to_string()
}

impl Default for DaemonSettings {
  fn default() -> Self {
    Self {
      llm_background_ptt_combo: d_llm_ptt(),
      tts_background_combo: d_tts(),
      stt_and_paste_background_ptt_combo: d_paste_ptt(),
      llm_background_reset: d_reset(),
    }
  }
}

impl DaemonSettings {
  /// (setting name, combo) pairs, in a fixed order.
  pub fn combos(&self) -> [(&'static str, &str); 4] {
    [
      ("llm_background_ptt_combo", self.llm_background_ptt_combo.as_str()),
      ("tts_background_combo", self.tts_background_combo.as_str()),
      (
        "stt_and_paste_background_ptt_combo",
        self.stt_and_paste_background_ptt_combo.as_str(),
      ),
      ("llm_background_reset", self.llm_background_reset.as_str()),
    ]
  }
}

/// The `[general]`, `[daemon]` and `[system_prompt]` blocks cut out of a
/// settings file, plus everything else (the `[agent]` sections) untouched.
#[derive(Debug, Clone, Default)]
pub struct LeadingSections {
  pub general: Option<String>,
  pub daemon: Option<String>,
  pub rest: String,
  /// Bodies of the `[system_prompt]` blocks, keyed by their `name`. Agents
  /// pull them in with `system_prompt = @<name>`.
  pub prompts: HashMap<String, String>,
  /// Malformed `[system_prompt]` blocks (no name, no fence, duplicated
  /// name...). Reported by `load_settings`.
  pub prompt_errors: Vec<String>,
  /// Section headers that are none of [general], [daemon], [system_prompt],
  /// [agent] (typos, wrong case...). Reported by `load_settings`.
  pub unknown: Vec<String>,
}

/// Separate the `[general]`, `[daemon]` and `[system_prompt]` sections from
/// the `[agent]` sections. A section starts at a line that is exactly its
/// header and ends at the next line starting with '['. Text before any header
/// stays in `rest`.
///
/// A `[system_prompt]` block is `key = value` lines (only `name` is read)
/// followed by the prompt body fenced between two lines of three or more
/// dashes. The body is taken verbatim and is never scanned for headers, so it
/// may hold lines starting with '['. The closing fence must be at least as
/// long as the opening one, so a body containing `---` opens with `----`.
pub fn split_leading_sections(text: &str) -> LeadingSections {
  #[derive(PartialEq)]
  enum Cur {
    Rest,
    General,
    Daemon,
    /// Inside a [system_prompt], before its opening fence.
    PromptKeys,
    /// Inside the fenced body of a [system_prompt].
    PromptBody,
  }
  let mut out = LeadingSections::default();
  let mut cur = Cur::Rest;
  // The [system_prompt] block being read.
  let mut prompt_name: Option<String> = None;
  let mut prompt_body = String::new();
  let mut fence_len = 0usize;

  for line in text.split_inclusive('\n') {
    let t = line.trim();
    // Body lines are verbatim: only the closing fence ends them, so a '['
    // at the start of a line is just text.
    if cur == Cur::PromptBody {
      if is_fence(t) && t.len() >= fence_len {
        finish_prompt(
          &mut out,
          prompt_name.take(),
          take_body(&mut prompt_body),
          true,
        );
        cur = Cur::Rest;
      } else if let Some(without_crlf) = line.strip_suffix("\r\n") {
        // a body is text for the llm: keep it CRLF-free even in a file
        // written on windows
        prompt_body.push_str(without_crlf);
        prompt_body.push('\n');
      } else {
        prompt_body.push_str(line);
      }
      continue;
    }
    if cur == Cur::PromptKeys && is_fence(t) {
      fence_len = t.len();
      cur = Cur::PromptBody;
      continue;
    }
    if t.starts_with('[') && t.ends_with(']') {
      if cur == Cur::PromptKeys {
        // a new section started before the body fence
        finish_prompt(&mut out, prompt_name.take(), String::new(), false);
      }
      // headers are matched case-insensitively and re-emitted canonical
      cur = match t.to_ascii_lowercase().as_str() {
        "[general]" => {
          out.general.get_or_insert_with(String::new);
          Cur::General
        }
        "[daemon]" => {
          out.daemon.get_or_insert_with(String::new);
          Cur::Daemon
        }
        "[system_prompt]" => {
          prompt_name = None;
          prompt_body.clear();
          Cur::PromptKeys
        }
        "[agent]" => {
          let body_len = line.trim_end_matches(['\r', '\n']).len();
          out.rest.push_str("[agent]");
          out.rest.push_str(&line[body_len..]); // keep the original line ending
          Cur::Rest
        }
        _ => {
          out.unknown.push(t.to_string());
          out.rest.push_str(line);
          Cur::Rest
        }
      };
      continue;
    }
    match cur {
      Cur::Rest => out.rest.push_str(line),
      Cur::General => out.general.as_mut().unwrap().push_str(line),
      Cur::Daemon => out.daemon.as_mut().unwrap().push_str(line),
      Cur::PromptKeys => {
        if let Some(v) = prompt_key_value(line, "name") {
          prompt_name = Some(v);
        }
      }
      Cur::PromptBody => unreachable!("handled above"),
    }
  }
  if cur == Cur::PromptKeys || cur == Cur::PromptBody {
    finish_prompt(
      &mut out,
      prompt_name.take(),
      take_body(&mut prompt_body),
      false,
    );
  }
  out
}

/// A fence line: three or more dashes and nothing else.
fn is_fence(trimmed_line: &str) -> bool {
  trimmed_line.len() >= 3 && trimmed_line.bytes().all(|b| b == b'-')
}

/// `key = value` inside a [system_prompt] header, quotes and comments handled
/// like everywhere else. `None` when the line is not that key.
fn prompt_key_value(line: &str, key: &str) -> Option<String> {
  let trimmed = line.trim_start();
  if trimmed.starts_with('#') || trimmed.starts_with(';') {
    return None;
  }
  let (k, v) = line.split_once('=')?;
  if k.trim() != key {
    return None;
  }
  let v = v.trim();
  let v = if v.len() >= 2 && v.starts_with('"') && v.ends_with('"') {
    &v[1..v.len() - 1]
  } else {
    v
  };
  Some(v.trim().to_string())
}

/// Take the accumulated body, dropping the line ending that belongs to the
/// closing fence. Everything else is kept byte for byte.
fn take_body(body: &mut String) -> String {
  let mut out = std::mem::take(body);
  if out.ends_with('\n') {
    out.pop();
  }
  out
}

/// Record a finished `[system_prompt]` block, or why it was rejected.
fn finish_prompt(out: &mut LeadingSections, name: Option<String>, body: String, terminated: bool) {
  if !terminated {
    out.prompt_errors.push(format!(
      "[system_prompt] {} is not closed: its body must be fenced between two lines of three or more dashes",
      name
        .as_deref()
        .map(|n| format!("'{}'", n))
        .unwrap_or_else(|| "block".to_string())
    ));
    return;
  }
  let Some(name) = name else {
    out
      .prompt_errors
      .push("a [system_prompt] block has no 'name'".to_string());
    return;
  };
  if !is_valid_prompt_name(&name) {
    out.prompt_errors.push(format!(
      "invalid [system_prompt] name '{}': use letters, digits, '_' or '-'",
      name
    ));
    return;
  }
  if out.prompts.insert(name.clone(), body).is_some() {
    out
      .prompt_errors
      .push(format!("duplicated [system_prompt] name '{}'", name));
  }
}

/// Expand an agent's `system_prompt` value. `@name` is the body of the
/// matching `[system_prompt]` block, taken verbatim: it already holds real
/// new lines, so no escape is expanded in it. `@@` starts an inline prompt
/// with a literal '@'. Anything else is an inline prompt, where `\n` becomes
/// a new line as it always has.
pub fn resolve_system_prompt(
  value: &str,
  prompts: &HashMap<String, String>,
) -> Result<String, String> {
  let trimmed = value.trim();
  if let Some(rest) = trimmed.strip_prefix("@@") {
    return Ok(format!("@{}", rest).replace("\\n", "\n"));
  }
  if let Some(name) = trimmed.strip_prefix('@') {
    let name = name.trim();
    return prompts.get(name).cloned().ok_or_else(|| {
      let mut defined: Vec<&str> = prompts.keys().map(|s| s.as_str()).collect();
      defined.sort_unstable();
      if defined.is_empty() {
        format!(
          "unknown system_prompt '@{}': no [system_prompt] section is defined",
          name
        )
      } else {
        format!(
          "unknown system_prompt '@{}': defined ones are {}",
          name,
          defined.join(", ")
        )
      }
    });
  }
  Ok(value.replace("\\n", "\n"))
}

/// Rewrite an INI block as `key=value` lines: whitespace trimmed, one layer
/// of surrounding double quotes removed, comments and lines without '='
/// dropped. This is what serde_ini gets to parse.
fn clean_ini_block(block: &str) -> String {
  let mut clean_section = String::new();
  for line in block.lines() {
    let trimmed = line.trim_start();
    if trimmed.starts_with('#') || trimmed.starts_with(';') {
      continue;
    }
    if let Some(idx) = line.find('=') {
      let (key, val_part) = line.split_at(idx);
      let key = key.trim();
      let val = &val_part[1..].trim();
      let val_trimmed = if val.len() >= 2 && val.starts_with('"') && val.ends_with('"') {
        &val[1..val.len() - 1]
      } else {
        val
      };
      clean_section.push_str(key);
      clean_section.push('=');
      clean_section.push_str(val_trimmed);
      clean_section.push('\n');
    }
  }
  clean_section
}

/// Parse the `[general]` section of a settings file (defaults when absent).
pub fn load_general_settings(settings_path: &std::path::Path) -> Result<GeneralSettings, Error> {
  let text = read_to_string(settings_path)?;
  let sections = split_leading_sections(&text);
  let Some(block) = sections.general else {
    return Ok(GeneralSettings::default());
  };
  let clean = clean_ini_block(&block);
  if clean.trim().is_empty() {
    return Ok(GeneralSettings::default());
  }
  let mut g: GeneralSettings = from_str(clean.trim())
    .map_err(|e| Error::msg(format!("Failed to parse [general] section: {}", e)))?;
  g.selected_agent = g.selected_agent.trim_matches('"').trim().to_string();
  Ok(g)
}

/// Parse and validate the `[daemon]` section (defaults when absent): every
/// combo must parse as a global shortcut and the four must be distinct.
pub fn load_daemon_settings(settings_path: &std::path::Path) -> Result<DaemonSettings, Error> {
  let text = read_to_string(settings_path)?;
  let sections = split_leading_sections(&text);
  let mut d = match sections.daemon {
    None => DaemonSettings::default(),
    Some(block) => {
      let clean = clean_ini_block(&block);
      if clean.trim().is_empty() {
        DaemonSettings::default()
      } else {
        from_str(clean.trim())
          .map_err(|e| Error::msg(format!("Failed to parse [daemon] section: {}", e)))?
      }
    }
  };
  for field in [
    &mut d.llm_background_ptt_combo,
    &mut d.tts_background_combo,
    &mut d.stt_and_paste_background_ptt_combo,
    &mut d.llm_background_reset,
  ] {
    *field = field.trim_matches('"').trim().to_lowercase();
  }
  validate_daemon_settings(&d)?;
  Ok(d)
}

pub fn validate_daemon_settings(d: &DaemonSettings) -> Result<(), Error> {
  use global_hotkey::hotkey::HotKey;
  use std::str::FromStr;
  let mut seen: Vec<(u32, &str, &str)> = Vec::new();
  let mut errors: Vec<String> = Vec::new();
  for (name, combo) in d.combos() {
    if combo.is_empty() {
      errors.push(format!("[daemon] {}: shortcut cannot be empty", name));
      continue;
    }
    match HotKey::from_str(combo) {
      Ok(hk) => {
        if let Some((_, other_name, other_combo)) = seen.iter().find(|(id, _, _)| *id == hk.id())
        {
          errors.push(format!(
            "[daemon] {} ({}) is the same shortcut as {} ({})",
            name, combo, other_name, other_combo
          ));
        }
        seen.push((hk.id(), name, combo));
      }
      Err(e) => errors.push(format!(
        "[daemon] {}: '{}' is not a valid shortcut ({}). Example: ctrl+alt+a",
        name, combo, e
      )),
    }
  }
  if errors.is_empty() {
    Ok(())
  } else {
    Err(Error::msg(errors.join("\n")))
  }
}

/// Pick the agent to start with: `-a` wins (error when unknown), then
/// `[general] selected_agent` (warning + first agent when unknown), then the
/// first agent.
pub fn select_agent(
  agents: &[AgentSettings],
  cli: Option<&str>,
  general: &GeneralSettings,
) -> Result<AgentSettings, String> {
  let available = || {
    agents
      .iter()
      .map(|a| a.name.as_str())
      .collect::<Vec<&str>>()
      .join(", ")
  };
  if let Some(name) = cli {
    return agents
      .iter()
      .find(|a| a.name == name)
      .cloned()
      .ok_or_else(|| format!("Agent '{}' not found. Available agents: {}", name, available()));
  }
  let first = agents
    .first()
    .cloned()
    .ok_or_else(|| "No agents configured".to_string())?;
  if general.selected_agent.is_empty() {
    return Ok(first);
  }
  match agents.iter().find(|a| a.name == general.selected_agent) {
    Some(a) => Ok(a.clone()),
    None => {
      crate::log::log(
        "warning",
        &format!(
          "[general] selected_agent '{}' not found (available: {}); using '{}'",
          general.selected_agent,
          available(),
          first.name
        ),
      );
      Ok(first)
    }
  }
}

/// Write `selected_agent = <name>` into the `[general]` section of the
/// settings file, touching nothing else. The section is created at the top of
/// the file when missing. The file is replaced atomically (tmp + rename).
pub fn persist_selected_agent(settings_path: &std::path::Path, name: &str) -> std::io::Result<()> {
  let text = read_to_string(settings_path)?;
  let new_line = format!("selected_agent = {}", name);
  let mut out = String::with_capacity(text.len() + 64);
  let mut in_general = false;
  let mut found_section = false;
  let mut replaced = false;
  // [system_prompt] bodies are copied through untouched: a line of theirs
  // starting with '[' is text, not a section header.
  let mut in_prompt_keys = false;
  let mut prompt_fence: Option<usize> = None;
  for line in text.split_inclusive('\n') {
    let t = line.trim();
    if let Some(len) = prompt_fence {
      if is_fence(t) && t.len() >= len {
        prompt_fence = None;
      }
      out.push_str(line);
      continue;
    }
    if in_prompt_keys && is_fence(t) {
      prompt_fence = Some(t.len());
      in_prompt_keys = false;
      out.push_str(line);
      continue;
    }
    let ending = if line.ends_with("\r\n") {
      "\r\n"
    } else if line.ends_with('\n') {
      "\n"
    } else {
      ""
    };
    if t.starts_with('[') {
      if in_general && !replaced {
        out.push_str(&new_line);
        out.push('\n');
        replaced = true;
      }
      in_general = t == "[general]";
      in_prompt_keys = t.eq_ignore_ascii_case("[system_prompt]");
      if in_general {
        found_section = true;
      }
      out.push_str(line);
      continue;
    }
    if in_general && !replaced {
      let key = line.split('=').next().map(|k| k.trim()).unwrap_or("");
      if key == "selected_agent" {
        out.push_str(&new_line);
        out.push_str(ending);
        replaced = true;
        continue;
      }
    }
    out.push_str(line);
  }
  if found_section && !replaced {
    // section at the end of the file without the key
    if !out.ends_with('\n') && !out.is_empty() {
      out.push('\n');
    }
    out.push_str(&new_line);
    out.push('\n');
  }
  if !found_section {
    let mut fresh = String::with_capacity(text.len() + 64);
    fresh.push_str("[general]\n");
    fresh.push_str(&new_line);
    fresh.push_str("\n\n");
    fresh.push_str(text.trim_start_matches(['\n', '\r']));
    out = fresh;
  }
  write_atomically(settings_path, &out)
}

/// Replace `settings_path` with `contents` in one step: write a sibling
/// `.tmp`, flush it to disk and rename it over the file, so a full disk or a
/// crash halfway through never leaves a truncated settings file behind.
pub fn write_atomically(settings_path: &std::path::Path, contents: &str) -> std::io::Result<()> {
  let tmp = settings_path.with_file_name(format!(
    "{}.tmp",
    settings_path
      .file_name()
      .map(|f| f.to_string_lossy().into_owned())
      .unwrap_or_else(|| "settings".to_string())
  ));
  {
    let mut f = File::create(&tmp)?;
    f.write_all(contents.as_bytes())?;
    f.sync_all()?;
  }
  std::fs::rename(&tmp, settings_path)
}

/// The command line as vtmate sees it with no option at all: what reading the
/// settings file needs when no override should apply to what is on disk.
pub fn plain_args() -> Args {
  Args::parse_from(["vtmate"])
}

/// Path of the settings file: `-c` (with `~` expanded) or `~/.vtmate/settings`.
pub fn resolve_settings_path(args: &Args) -> Result<std::path::PathBuf, Error> {
  if let Some(ref cfg) = args.config {
    let mut path = std::path::PathBuf::from(cfg.as_str());
    if path.starts_with("~") {
      if let Some(home) = get_user_home_path() {
        let rel = path.strip_prefix("~").unwrap_or(&path).to_path_buf();
        path = home.join(rel);
      }
    }
    return Ok(path);
  }
  Ok(
    get_user_home_path()
      .ok_or_else(|| Error::msg("Unable to determine home directory"))?
      .join(".vtmate")
      .join("settings"),
  )
}

fn bool_from_str_or_bool<'de, D>(deserializer: D) -> Result<bool, D::Error>
where
  D: serde::de::Deserializer<'de>,
{
  struct BoolVisitor;
  impl<'de> serde::de::Visitor<'de> for BoolVisitor {
    type Value = bool;
    fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
      formatter.write_str("a boolean or string representing a boolean")
    }
    fn visit_bool<E>(self, v: bool) -> Result<bool, E> {
      Ok(v)
    }
    fn visit_str<E>(self, v: &str) -> Result<bool, E>
    where
      E: serde::de::Error,
    {
      v.parse::<bool>().map_err(serde::de::Error::custom)
    }
    fn visit_string<E>(self, v: String) -> Result<bool, E>
    where
      E: serde::de::Error,
    {
      v.parse::<bool>().map_err(serde::de::Error::custom)
    }
  }
  deserializer.deserialize_any(BoolVisitor)
}

pub fn resolved_whisper_model_path(whisper_model_path: &str) -> String {
  let path = if whisper_model_path.is_empty() {
    "~/.whisper-models/ggml-tiny.bin".to_string()
  } else {
    whisper_model_path.to_string()
  };
  if path.starts_with("~") {
    if let Some(home) = get_user_home_path() {
      let rel = path.trim_start_matches("~").trim_start_matches("/");
      let mut p = home;
      p.push(rel);
      p.to_string_lossy().into_owned()
    } else {
      path
    }
  } else {
    path
  }
}

/// Why a settings file could not be turned into a list of agents.
#[derive(Debug)]
pub enum LoadError {
  /// The file cannot be read, or a section of it cannot be parsed.
  Syntax(String),
  /// The file parses but one or more agents hold invalid values.
  Invalid(String),
}

impl std::fmt::Display for LoadError {
  fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
    match self {
      LoadError::Syntax(m) | LoadError::Invalid(m) => f.write_str(m),
    }
  }
}

impl std::error::Error for LoadError {}

/// Read, expand and validate every `[agent]` section, reporting problems
/// instead of writing to the terminal. `load_settings` is the startup
/// entry point; the settings popup uses this one to reload after saving.
pub fn try_load_settings(
  settings_path: &std::path::Path,
  args: &Args,
) -> Result<Vec<AgentSettings>, LoadError> {
  // Read the whole INI file; the [general] and [daemon] sections are parsed
  // separately (load_general_settings / load_daemon_settings).
  let text = read_to_string(settings_path)
    .map_err(|e| LoadError::Syntax(format!("{}: {}", settings_path.display(), e)))?;
  let sections = split_leading_sections(&text);
  if !sections.unknown.is_empty() {
    return Err(LoadError::Syntax(format!(
      "unknown section {} in {}: expected [general], [daemon], [system_prompt] or [agent]",
      sections.unknown.join(", "),
      settings_path.display()
    )));
  }
  // Malformed [system_prompt] blocks are a file syntax problem, like an
  // unknown section: report them all and give up before parsing agents.
  if !sections.prompt_errors.is_empty() {
    return Err(LoadError::Syntax(format!(
      "in {}:\n{}",
      settings_path.display(),
      sections.prompt_errors.join("\n")
    )));
  }
  let prompts = sections.prompts;
  let ini_contents = sections.rest;
  // Split on the section header "[agent]"
  let blocks: Vec<&str> = ini_contents
    .split("[agent]")
    .filter(|b| !b.trim().is_empty())
    .collect();

  let mut agents = Vec::new();
  let mut errors: Vec<String> = Vec::new();
  let mut prompt_ref_errors: Vec<String> = Vec::new();
  for block in blocks {
    // Preprocess the block to remove surrounding quotes from values
    let clean_section = clean_ini_block(block);
    let section = clean_section.trim();

    let mut agent: AgentSettings = match panic::catch_unwind(|| from_str::<AgentSettings>(&section))
    {
      Ok(Ok(a)) => a,
      Ok(Err(e)) => {
        return Err(LoadError::Syntax(format!(
          "Failed to parse agent's settings section: {}",
          e
        )));
      }
      Err(_) => {
        return Err(LoadError::Syntax(
          "panic while parsing agent's section".to_string(),
        ));
      }
    };
    // Sanitize quoted string values in AgentSettings before validation
    sanitize_agent_settings(&mut agent);

    // Expand `system_prompt = @name` into the body of its [system_prompt]
    // block, or the `\n` escapes of an inline prompt, before validating it.
    let block_name = system_prompt_block_name(&agent.system_prompt);
    match resolve_system_prompt(&agent.system_prompt, &prompts) {
      Ok(prompt) => {
        agent.system_prompt = prompt;
        agent.system_prompt_name = block_name;
      }
      Err(e) => prompt_ref_errors.push(format!("Agent {}: {}", agent.name, e)),
    }

    for problem in validate_agent(&agent) {
      errors.push(format!("Agent {}: {}", agent.name, problem));
    }

    agents.push(agent);
  }

  if !prompt_ref_errors.is_empty() {
    return Err(LoadError::Syntax(prompt_ref_errors.join("\n")));
  }

  if !errors.is_empty() {
    return Err(LoadError::Invalid(errors.join("\n")));
  }

  if agents.is_empty() {
    return Err(LoadError::Syntax(
      "No [agent] sections found in settings file".to_string(),
    ));
  }

  // Validate CLI args
  if let Some(ref agent_name) = args.agent {
    if let Err(e) = validate_agent_name(agent_name) {
      return Err(LoadError::Invalid(e.to_string()));
    }
  }

  // Merge args into each agent's settings
  for agent in agents.iter_mut() {
    if let Some(ptt_val) = args.ptt {
      agent.ptt = ptt_val;
    }
  }

  Ok(agents)
}

pub fn load_settings(
  settings_path: &std::path::Path,
  args: &Args,
) -> Result<Vec<AgentSettings>, Error> {
  match try_load_settings(settings_path, args) {
    Ok(agents) => Ok(agents),
    // A file we cannot parse is reported to the caller, which decides how to
    // give up; values we can parse but cannot accept stop vtmate right here,
    // with the whole list of what is wrong.
    Err(e @ LoadError::Syntax(_)) => {
      print!("\u{274c} {}", e);
      thread::sleep(Duration::from_millis(30));
      Err(Error::msg(e.to_string()))
    }
    Err(e @ LoadError::Invalid(_)) => {
      print!("\u{274c} {}", e);
      thread::sleep(Duration::from_millis(30));
      terminate(1);
    }
  }
}

pub fn ensure_settings_file() -> Result<(), Error> {
  // Determine home directory
  let home =
    get_user_home_path().ok_or_else(|| Error::msg("Unable to determine home directory"))?;

  let ai_mate_dir = home.join(".vtmate");
  // Ensure directory exists
  if !ai_mate_dir.exists() {
    create_dir_all(&ai_mate_dir)?;
  }
  let settings_path = ai_mate_dir.join("settings");
  // If file already exists, skip writing
  if settings_path.exists() {
    return Ok(());
  }
  let content = r#"[general]
selected_agent = main agent

[daemon]
llm_background_ptt_combo = ctrl+alt+a
tts_background_combo = ctrl+alt+r
stt_and_paste_background_ptt_combo = ctrl+alt+s
llm_background_reset = ctrl+q

[system_prompt]
name = concise_assistant
---
You are a neutral, helpful AI assistant.
Follow the subject of the conversation with special attention to the user
request. Provide accurate, concise answers.

Rules:
  1. Keep replies under 30 words.
  2. If a longer answer is required, limit it to 250 words.
  3. Assume no prior context unless the user supplies it.
  4. Do not mention yourself.
---

[agent]
name = main agent
language = en
tts = supertonic
voice = M1
voice_speed = 1.1
provider = ollama
baseurl = http://127.0.0.1:11434
model = llama3.2:3b
system_prompt = @concise_assistant
sound_threshold_peak = 0.12
end_silence_ms = 2500
ptt = true
whisper_model_path = ~/.whisper-models/ggml-tiny.bin

[agent]
name = explainer
language = en
tts = supertonic
voice = F1
voice_speed = 1.1
provider = ollama
baseurl = http://127.0.0.1:11434
model = llama3.2:3b
system_prompt = "You are a helpful AI assistant. Your only funcion is to explain things as simple as possible in no more than 150 words or 450 words if the user asks for a longer explanation."
sound_threshold_peak = 0.12
end_silence_ms = 2500
ptt = true
whisper_model_path = ~/.whisper-models/ggml-tiny.bin

[agent]
name = planner
language = en
tts = supertonic
voice = F3
voice_speed = 1.1
provider = ollama
baseurl = http://127.0.0.1:11434
model = llama3.2:3b
system_prompt = You are an ai assistant which assist the user in the creation of a plan based on user's goal. When defining the plan follow the next format standards:\n 1-The plan is composed by tasks and subtasks.\n 2-Each task has the next format: "[ ] <task name>".\n 3-Subtasks are indented with 2 spaces below the parent task.\n 4-Before defining a plan, make sure you have the relevant information from the user.
sound_threshold_peak = 0.12
end_silence_ms = 2000
ptt = true
whisper_model_path = ~/.whisper-models/ggml-tiny.bin

[agent]
name = Ptahhotep
language = en
tts = supertonic
voice = M2
voice_speed = 1.1
provider = ollama
baseurl = http://127.0.0.1:11434
model = llama3.2:3b
system_prompt = You are Ptahhotep, an ancient Egyptian advisor. Follow the subject of the conversation with special attention to the user request. Provide concise, culturally informed wisdom; 30 words or fewer, max 250 words if detail needed.
sound_threshold_peak = 0.12
end_silence_ms = 2500
ptt = true
whisper_model_path = ~/.whisper-models/ggml-tiny.bin

[agent]
name = Aristoteles
language = en
tts = supertonic
voice = M3
voice_speed = 1.1
provider = ollama
baseurl = http://127.0.0.1:11434
model = llama3.2:3b
system_prompt = "You are Aristoteles, a creative thinker. Follow the subject of the conversation with special attention to the user request. Give clear, imaginative responses; keep them ≤30 words, with a maximum of 250 words when elaboration is necessary."
sound_threshold_peak = 0.12
end_silence_ms = 2500
ptt = true
whisper_model_path = ~/.whisper-models/ggml-tiny.bin

[agent]
name = Budda
language = en
tts = supertonic
voice = M4
voice_speed = 1.1
provider = ollama
baseurl = http://127.0.0.1:11434
model = llama3.2:3b
system_prompt = You are Budda, a serene guide. Follow the subject of the conversation with special attention to the user request. Offer tranquil, succinct answers; 30 words or less, extending to 250 words only when required.
sound_threshold_peak = 0.12
end_silence_ms = 2500
ptt = true
whisper_model_path = ~/.whisper-models/ggml-tiny.bin

[agent]
name = Jesus Christ
language = en
tts = supertonic
voice = M5
voice_speed = 1.1
provider = ollama
baseurl = http://127.0.0.1:11434
model = llama3.2:3b
system_prompt = You are Jesus Christ, a compassionate teacher. Follow the subject of the conversation with special attention to the user request. Provide gentle, clear answers; 30 words or fewer, with a ceiling of 250 words for longer explanations.
sound_threshold_peak = 0.12
end_silence_ms = 2500
ptt = true
whisper_model_path = ~/.whisper-models/ggml-tiny.bin


"#;
  let mut file = File::create(&settings_path)?;
  file.write_all(content.as_bytes())?;
  Ok(())
}

pub fn pick_input_config(
  device: &Device,
  preferred_sr: u32,
) -> Result<cpal::SupportedStreamConfig, Error> {
  use cpal::SampleFormat;

  let mut candidates: Vec<cpal::SupportedStreamConfig> = Vec::new();
  for range in device.supported_input_configs()? {
    let min_sr = range.min_sample_rate().0;
    let max_sr = range.max_sample_rate().0;
    let chosen_sr = preferred_sr.clamp(min_sr, max_sr);
    candidates.push(range.with_sample_rate(cpal::SampleRate(chosen_sr)));
  }

  candidates.sort_by_key(|cfg| {
    let fmt_rank = match cfg.sample_format() {
      SampleFormat::F32 => 0,
      SampleFormat::I16 => 1,
      SampleFormat::U16 => 2,
      _ => 9,
    };
    let ch_rank = match cfg.channels() {
      1 => 0,
      2 => 1,
      _ => 5,
    };
    let sr_rank = cfg.sample_rate().0.abs_diff(preferred_sr);
    (fmt_rank, ch_rank, sr_rank)
  });

  candidates
    .into_iter()
    .next()
    .ok_or_else(|| Error::msg("no supported input configs"))
}

// Writing the settings file back
// ------------------------------------------------------------------

/// How many lines a prompt may have before it is written as a
/// `[system_prompt]` block instead of an inline `system_prompt = ...` value.
pub const INLINE_PROMPT_MAX_LINES: usize = 5;

/// The `@<name>` an agent's raw `system_prompt` value refers to, if any.
/// `@@...` is an inline prompt starting with a literal '@', not a reference.
pub fn system_prompt_block_name(raw_value: &str) -> Option<String> {
  let trimmed = raw_value.trim();
  if trimmed.starts_with("@@") {
    return None;
  }
  trimmed
    .strip_prefix('@')
    .map(|name| name.trim().to_string())
    .filter(|name| is_valid_prompt_name(name))
}

/// Every check `try_load_settings` runs on one agent, as a list of messages
/// (empty when the agent is good). The settings popup runs the same ones
/// before it writes the file, so what it accepts is what vtmate can load.
pub fn validate_agent(agent: &AgentSettings) -> Vec<String> {
  let checks: [Result<(), std::io::Error>; 12] = [
    validate_agent_name(&agent.name).map(|_| ()),
    validate_provider(&agent.provider),
    validate_model(&agent.model),
    validate_baseurl(&agent.baseurl, &agent.provider),
    validate_api_key(&agent.api_key, &agent.provider),
    validate_system_prompt(&agent.system_prompt),
    validate_sound_threshold_peak(agent.sound_threshold_peak),
    validate_end_silence_ms(agent.end_silence_ms),
    validate_tts(&agent.tts),
    validate_language(&agent.language, &agent.tts),
    validate_voice(&agent.voice, &agent.language, &agent.tts),
    validate_voice_speed(agent.voice_speed),
  ];
  checks
    .into_iter()
    .filter_map(|c| c.err().map(|e| e.to_string()))
    .collect()
}

/// Write the whole settings file from `agents`: the `[general]` section (with
/// `selected_agent` set to `selected`) and the `[daemon]` section are carried
/// over from the file as they were, then come the `[system_prompt]` blocks the
/// prompts need, then one `[agent]` section per agent, in order.
///
/// A prompt is written inline (`\n` for its line breaks) while it fits in
/// `INLINE_PROMPT_MAX_LINES` lines and survives the round trip through the
/// INI syntax; anything longer, or holding text the inline form would eat
/// (a literal `\n`, surrounding spaces or quotes, a leading comment mark),
/// becomes a fenced `[system_prompt]` block. A prompt that came from a block
/// keeps that block's name.
///
/// The file is replaced atomically, so an interrupted save never leaves a
/// half-written settings file.
pub fn save_settings(
  settings_path: &std::path::Path,
  agents: &[AgentSettings],
  selected: &str,
) -> std::io::Result<()> {
  let previous = read_to_string(settings_path).unwrap_or_default();
  let sections = split_leading_sections(&previous);

  // one [system_prompt] block per prompt that cannot be written inline,
  // and the value the agent's `system_prompt` key gets
  let mut blocks: Vec<(String, String)> = Vec::new();
  let mut prompt_values: Vec<String> = Vec::with_capacity(agents.len());
  for agent in agents {
    match inline_prompt_value(&agent.system_prompt) {
      Some(inline) => prompt_values.push(inline),
      None => {
        let wanted = agent
          .system_prompt_name
          .clone()
          .filter(|n| is_valid_prompt_name(n))
          .unwrap_or_else(|| prompt_name_for(&agent.name));
        let name = claim_prompt_name(&mut blocks, wanted, &agent.system_prompt);
        prompt_values.push(format!("@{}", name));
      }
    }
  }

  let mut out = String::with_capacity(previous.len() + 512);

  // [general]: selected_agent first, then whatever else the user had there
  out.push_str("[general]\n");
  out.push_str(&format!("selected_agent = {}\n", selected.trim()));
  if let Some(block) = &sections.general {
    for line in block.lines() {
      let key = line.split('=').next().map(str::trim).unwrap_or("");
      if key == "selected_agent" || line.trim().is_empty() {
        continue;
      }
      out.push_str(line.trim_end_matches('\r'));
      out.push('\n');
    }
  }

  // [daemon]: carried over untouched (it is not edited here)
  if let Some(block) = &sections.daemon {
    out.push_str("\n[daemon]\n");
    for line in block.lines() {
      if line.trim().is_empty() {
        continue;
      }
      out.push_str(line.trim_end_matches('\r'));
      out.push('\n');
    }
  }

  for (name, body) in &blocks {
    let fence = "-".repeat(fence_len_for(body));
    out.push_str("\n[system_prompt]\n");
    out.push_str(&format!("name = {}\n", name));
    out.push_str(&fence);
    out.push('\n');
    out.push_str(body);
    out.push('\n');
    out.push_str(&fence);
    out.push('\n');
  }

  for (agent, prompt_value) in agents.iter().zip(prompt_values) {
    out.push_str("\n[agent]\n");
    out.push_str(&format!("name = {}\n", agent.name));
    out.push_str(&format!("language = {}\n", agent.language));
    out.push_str(&format!("tts = {}\n", agent.tts));
    out.push_str(&format!("voice = {}\n", agent.voice));
    out.push_str(&format!("voice_speed = {:.1}\n", agent.voice_speed));
    out.push_str(&format!("provider = {}\n", agent.provider));
    out.push_str(&format!("baseurl = {}\n", agent.baseurl));
    out.push_str(&format!("model = {}\n", agent.model));
    if !agent.api_key.trim().is_empty() {
      out.push_str(&format!("api_key = {}\n", agent.api_key.trim()));
    }
    out.push_str(&format!("system_prompt = {}\n", prompt_value));
    out.push_str(&format!(
      "sound_threshold_peak = {}\n",
      trim_float(agent.sound_threshold_peak, 3)
    ));
    out.push_str(&format!("end_silence_ms = {}\n", agent.end_silence_ms));
    out.push_str(&format!("ptt = {}\n", agent.ptt));
    out.push_str(&format!(
      "whisper_model_path = {}\n",
      agent.whisper_model_path
    ));
  }

  write_atomically(settings_path, &out)
}

// PRIVATE
// ------------------------------------------------------------------

/// A `[system_prompt]` name vtmate accepts: letters, digits, '_' and '-'.
fn is_valid_prompt_name(name: &str) -> bool {
  !name.is_empty()
    && name
      .chars()
      .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// The one-line `system_prompt = ...` value for `body`, or `None` when the
/// prompt has to be written as a `[system_prompt]` block instead: too many
/// lines, or text the inline syntax would not give back unchanged.
fn inline_prompt_value(body: &str) -> Option<String> {
  if body.is_empty()
    || body.lines().count() > INLINE_PROMPT_MAX_LINES
    || body != body.trim()
    || body.contains('\r')
    // a literal backslash-n would come back as a line break
    || body.contains("\\n")
    // a value starting with these is a comment, or loses its quotes
    || body.starts_with('#')
    || body.starts_with(';')
    || (body.len() >= 2 && body.starts_with('"') && body.ends_with('"'))
  {
    return None;
  }
  let escaped = body.replace('\n', "\\n");
  // `@name` pulls in a block, so an inline prompt starting with '@' doubles it
  Some(if escaped.starts_with('@') {
    format!("@{}", escaped)
  } else {
    escaped
  })
}

/// A `[system_prompt]` name made out of an agent name.
fn prompt_name_for(agent_name: &str) -> String {
  let mut slug = String::new();
  let mut pending_separator = false;
  for c in agent_name.chars() {
    if c.is_ascii_alphanumeric() || c == '-' {
      slug.push(c.to_ascii_lowercase());
      pending_separator = false;
    } else if !pending_separator {
      slug.push('_');
      pending_separator = true;
    }
  }
  let slug: String = slug.chars().take(40).collect();
  let slug = slug.trim_matches('_');
  if slug.is_empty() {
    "agent_prompt".to_string()
  } else {
    format!("{}_prompt", slug)
  }
}

/// Reserve a name for `body` in `blocks`: the wanted one when it is free,
/// the same one when it already holds this very prompt (agents sharing a
/// prompt keep sharing its block), a numbered variant otherwise.
fn claim_prompt_name(blocks: &mut Vec<(String, String)>, wanted: String, body: &str) -> String {
  if blocks
    .iter()
    .any(|(name, existing)| *name == wanted && existing.as_str() == body)
  {
    return wanted;
  }
  let mut name = wanted.clone();
  let mut suffix = 2;
  while blocks.iter().any(|(existing, _)| *existing == name) {
    name = format!("{}_{}", wanted, suffix);
    suffix += 1;
  }
  blocks.push((name.clone(), body.to_string()));
  name
}

/// Length of the fence around `body`: longer than any line of dashes in it,
/// so the body cannot close itself early.
fn fence_len_for(body: &str) -> usize {
  let mut len = 3;
  for line in body.lines() {
    let trimmed = line.trim();
    if is_fence(trimmed) {
      len = len.max(trimmed.len() + 1);
    }
  }
  len
}

/// `value` with at most `decimals` decimals and no trailing zeros (but always
/// one decimal, so the file keeps reading as a float).
fn trim_float(value: f32, decimals: usize) -> String {
  let mut out = format!("{:.*}", decimals, value);
  if out.contains('.') {
    while out.ends_with('0') {
      out.pop();
    }
    if out.ends_with('.') {
      out.push('0');
    }
  }
  out
}

fn validate_agent_name(name: &str) -> Result<String, std::io::Error> {
  let len = name.chars().count();
  if len < 1 || len > 200 {
    return Err(std::io::Error::new(
      std::io::ErrorKind::InvalidInput,
      "agent must be between 1 and 200 characters",
    ));
  } else {
    Ok(name.to_string())
  }
}

fn validate_language(language: &str, tts: &str) -> Result<(), std::io::Error> {
  let lang_clean = language.trim_matches('"');
  let langs = tts::get_all_available_languages();
  if !langs.contains(&lang_clean) {
    let err = format!("Unsupported language: {}", language);
    crate::log::log("error", &err);
    return Err(std::io::Error::new(std::io::ErrorKind::Other, err));
  }
  let voices = tts::get_voices_for(tts, lang_clean);
  if voices.is_empty() {
    let err = format!("No voices for language {} and TTS {}", language, tts);
    crate::log::log("error", &err);
    return Err(std::io::Error::new(std::io::ErrorKind::Other, err));
  }
  // Ensure the selected TTS engine supports this language
  let voices = tts::get_voices_for(tts, lang_clean);
  if voices.is_empty() {
    let err = format!(
      "No available voices for TTS '{}' and language '{}'",
      tts, language
    );
    crate::log::log("error", &err);
    return Err(std::io::Error::new(std::io::ErrorKind::Other, err));
  }
  Ok(())
}

fn validate_voice(voice: &str, language: &str, tts: &str) -> Result<(), std::io::Error> {
  // Validate voice format, supports mix of two voices
  let lang_clean = language.trim_matches('"');
  let voices = tts::get_voices_for(tts, lang_clean);
  if voices.is_empty() {
    return Err(std::io::Error::new(
      std::io::ErrorKind::Other,
      format!(
        "No available voices for TTS '{}' and language '{}'",
        tts, language
      ),
    ));
  }

  let voice_clean = voice.trim_matches('"');
  // Call helper for validation
  validate_voice_value(voice_clean, &voices, language).map_err(|e| {
    let mut msg = format!("{}. Available: {}", e, voices.join(", "));
    if let Some(dir) = tts::voice_styles_dir_for(tts) {
      msg.push_str(&format!(
        ". Add your own as <name>.json in {}",
        dir.display()
      ));
    }
    std::io::Error::new(std::io::ErrorKind::Other, msg)
  })
}

fn validate_tts(tts: &str) -> Result<(), std::io::Error> {
  if tts != "kokoro" && tts != "opentts" && tts != "supersonic2" && tts != "supertonic" {
    return Err(std::io::Error::new(
      std::io::ErrorKind::Other,
      format!(
        "Invalid tts '{}' . Must be 'kokoro', 'opentts', 'supersonic2', or 'supertonic'",
        tts
      ),
    ));
  }
  Ok(())
}

// Voice mix validation helper
fn validate_voice_value(
  voice: &str,
  voices: &Vec<String>,
  language: &str,
) -> Result<(), std::io::Error> {
  // If no mix, validate single voice
  if !voice.contains('+') {
    if voices.iter().any(|v| v.as_str() == voice) {
      return Ok(());
    } else {
      return Err(std::io::Error::new(
        std::io::ErrorKind::Other,
        format!("Unsupported voice '{}' for language {}", voice, language),
      ));
    }
  }

  // Parse mix format <v1>.<w1>+<v2>.<w2>
  let parts: Vec<&str> = voice.split('+').collect();
  if parts.len() != 2 {
    return Err(std::io::Error::new(
      std::io::ErrorKind::Other,
      format!("Unsupported voice '{}' for language {}", voice, language),
    ));
  }
  let mut total_weight = 0u32;
  for part in parts {
    let subparts: Vec<&str> = part.split('.').collect();
    if subparts.len() != 2 {
      return Err(std::io::Error::new(
        std::io::ErrorKind::Other,
        format!("Unsupported voice '{}' for language {}", voice, language),
      ));
    }
    let name = subparts[0];
    let weight_str = subparts[1];
    if weight_str.len() != 1 || !weight_str.chars().all(|c| c.is_ascii_digit()) {
      return Err(std::io::Error::new(
        std::io::ErrorKind::Other,
        format!("Unsupported voice '{}' for language {}", voice, language),
      ));
    }
    let weight: u32 = weight_str.parse().unwrap();
    total_weight += weight;
    if !voices.iter().any(|v| v.as_str() == name) {
      return Err(std::io::Error::new(
        std::io::ErrorKind::Other,
        format!("Unsupported voice '{}' for language {}", voice, language),
      ));
    }
  }
  if total_weight != 10 {
    return Err(std::io::Error::new(
      std::io::ErrorKind::Other,
      format!("Voice mix '{}' does not sum to 100%", voice),
    ));
  }
  Ok(())
}

fn validate_provider(provider: &str) -> Result<(), std::io::Error> {
  if !crate::llm::is_supported_provider(provider) {
    return Err(std::io::Error::new(
      std::io::ErrorKind::Other,
      format!(
        "Invalid provider '{}' . Must be one of {}",
        provider,
        crate::llm::supported_providers_list()
      ),
    ));
  }
  Ok(())
}

fn validate_api_key(api_key: &str, provider: &str) -> Result<(), std::io::Error> {
  if crate::llm::is_cloud_provider(provider)
    && crate::llm::resolve_api_key(provider, api_key).is_none()
  {
    return Err(std::io::Error::new(
      std::io::ErrorKind::Other,
      format!(
        "provider '{}' needs an api_key (set it in the settings file or via the {} environment variable)",
        provider,
        crate::llm::api_key_env_var(provider).unwrap_or("provider")
      ),
    ));
  }
  Ok(())
}

fn validate_baseurl(baseurl: &str, provider: &str) -> Result<(), std::io::Error> {
  if baseurl.trim().is_empty() {
    // hosted providers have a default endpoint, local servers must be addressed
    if crate::llm::is_cloud_provider(provider) {
      return Ok(());
    }
    return Err(std::io::Error::new(
      std::io::ErrorKind::Other,
      format!("baseurl is required for provider '{}'", provider),
    ));
  }
  let url = Url::parse(baseurl).map_err(|e| {
    std::io::Error::new(
      std::io::ErrorKind::Other,
      format!("Invalid baseurl '{}' : {}", baseurl, e),
    )
  })?;
  if !url.has_host() {
    return Err(std::io::Error::new(
      std::io::ErrorKind::Other,
      format!("baseurl must have a host: {}", baseurl),
    ));
  }
  // local servers are addressed by host and port, vtmate appends /v1 itself
  if crate::llm::is_local_provider(provider) && url.path() != "/" && url.path() != "/v1" {
    return Err(std::io::Error::new(
      std::io::ErrorKind::Other,
      format!("baseurl must have a host and no path: {}", baseurl),
    ));
  }
  Ok(())
}

fn validate_model(model: &str) -> Result<(), std::io::Error> {
  if model.is_empty() || model.len() > 200 {
    return Err(std::io::Error::new(
      std::io::ErrorKind::Other,
      "'model' must be 1-200 characters",
    ));
  }
  Ok(())
}

fn validate_system_prompt(prompt: &str) -> Result<(), std::io::Error> {
  if prompt.is_empty() || prompt.len() > 20000 {
    return Err(std::io::Error::new(
      std::io::ErrorKind::Other,
      "'system_prompt' must be 0-20000 characters",
    ));
  }
  Ok(())
}

fn validate_sound_threshold_peak(value: f32) -> Result<(), std::io::Error> {
  // Voice speed is not validated here
  if value < 0.0 || value > 1.0 {
    return Err(std::io::Error::new(
      std::io::ErrorKind::Other,
      "'sound_threshold_peak' must be between 0.0 and 1.0",
    ));
  }
  let scaled = (value * 1000.0).round();
  if (scaled / 1000.0 - value).abs() > 1e-6 {
    return Err(std::io::Error::new(
      std::io::ErrorKind::Other,
      "'sound_threshold_peak' must have at most 3 decimal places",
    ));
  }
  Ok(())
}

fn validate_end_silence_ms(value: u64) -> Result<(), std::io::Error> {
  if value < 1 || value > 20000 {
    return Err(std::io::Error::new(
      std::io::ErrorKind::Other,
      "'end_silence_ms' must be between 1 and 20000",
    ));
  }
  Ok(())
}

fn validate_voice_speed(value: f32) -> Result<(), std::io::Error> {
  if value < 1.0 || value > 9.0 {
    return Err(std::io::Error::new(
      std::io::ErrorKind::Other,
      "'voice_speed' must be between 1.0 and 9.0",
    ));
  }
  // Ensure one decimal place only
  let scaled = (value * 10.0).round();
  if (scaled / 10.0 - value).abs() > 1e-6 {
    return Err(std::io::Error::new(
      std::io::ErrorKind::Other,
      "'voice_speed' must have one decimal place",
    ));
  }
  Ok(())
}

// PRIVATE
// ------------------------------------------------------------------

// Sanitizes quoted string values in AgentSettings
fn sanitize_agent_settings(agent: &mut AgentSettings) {
  agent.name = agent.name.trim_matches('"').to_string();
  agent.language = agent.language.trim_matches('"').to_string();
  agent.tts = agent.tts.trim_matches('"').to_string();
  agent.voice = agent.voice.trim_matches('"').to_string();
  agent.provider = agent.provider.trim_matches('"').to_string();
  agent.baseurl = agent.baseurl.trim_matches('"').to_string();
  agent.model = agent.model.trim_matches('"').to_string();
  agent.api_key = agent.api_key.trim_matches('"').to_string();
  agent.system_prompt = agent.system_prompt.trim_matches('"').to_string();
  // agent.ptt is a bool; no trimming needed
  agent.whisper_model_path = agent.whisper_model_path.trim_matches('"').to_string();
}
