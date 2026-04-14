// ------------------------------------------------------------------
//  Config
// ------------------------------------------------------------------

use crate::tts;
use crate::util::get_user_home_path;
use anyhow::Error;
use clap::Parser;
use cpal::traits::DeviceTrait;
use cpal::Device;
use serde::Deserialize;
use serde_ini::from_str;
use std::fs::{create_dir_all, read_to_string, File};
use std::io::Write;
use std::panic;
use std::process;
use std::thread::{self};
use std::time::Duration;
use url::Url;

// API
// ------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct Settings {
  pub agents: Vec<AgentSettings>,
  pub default_agent: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct AgentSettings {
  pub name: String,
  pub language: String,
  pub tts: String,
  pub voice: String,
  pub provider: String,
  pub baseurl: String,
  pub model: String,
  pub system_prompt: String,
  #[serde(deserialize_with = "bool_from_str_or_bool")]
  pub ptt: bool,
  pub whisper_model_path: String,
  pub sound_threshold_peak: f32,
  pub end_silence_ms: u64,
}

#[derive(Parser, Debug, Clone)]
#[clap(after_help = r#"
Settings file is at ~/.ai-mate/settings

Explanation on the fields:

  * name:                 a short name for the agent
  * language:             any of the languages available used
                          for speech recognition and tts
  * voice:                the voice name to use by the
                          agent (see available voices for each
                          language and tts system running
                          `ai-mate --list-voices`)
  * provider:             the system it will use to query
                          the llm, it can be 'ollama' or
                          'llama-server'
  * baseurl:              the base url used to contact the
                          provider (it needs to be without path)
  * model:                the model name to use in ollama
                          (some llama-server versions will
                          ignore this option as llama-server
                          runs for a single model)
  * system_prompt:        the system prompt to be sent to
                          the llm when querying it.
                          Use \n for new lines
  * sound_threshold_peak: a value between 0 and 1 which will
                          be used as a peak base to detect
                          user speech
  * end_silence_ms:       the milliseconds of silence below
                          sound_threshold_peak level that
                          have to elapse for user speech
                          to be submitted
  * tts:                  the tts system to use, it can be
                          'kokoro' or 'opentts'
  * ptt:                  push to talk mode, when its set
                          to true you have to keep the space
                          pushed while speaking, then release.
  * whisper_model_path:   the path to the whisper model.
                          ai-mate unzips 2 models in
                          ~/.whisper-models, tiny and small.
                          You can download bigger models and
                          point to them here

"#)]
pub struct Args {
  #[arg(long, action = clap::ArgAction::SetTrue)]
  pub verbose: bool,

  #[arg(long, action=clap::ArgAction::SetTrue)]
  pub list_voices: bool,

  #[arg(long, value_parser=validate_agent_name)]
  pub agent: Option<String>,

  #[arg(long)]
  pub ptt: Option<bool>,

  #[arg(long, num_args=3.., value_name = "AGENT1 AGENT2 SUBJECT")]
  pub debate: Option<Vec<String>>,

  #[arg(short = 'r', long = "read-file", value_name = "FILENAME")]
  pub read_file: Option<String>,
}

// internal static values
pub const HANGOVER_MS_DEFAULT: u64 = 300;
pub const MIN_UTTERANCE_MS_DEFAULT: u64 = 300;
pub const OPENTTS_BASE_URL_DEFAULT: &str = "http://127.0.0.1:5500/api/tts?&vocoder=high&denoiserStrength=0.005&&speakerId=&ssml=false&ssmlNumbers=true&ssmlDates=true&ssmlCurrency=true&cache=false";

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

pub fn load_settings(
  settings_path: &std::path::Path,
  args: &Args,
) -> Result<Vec<AgentSettings>, Error> {
  // Read the whole INI file
  let ini_contents = read_to_string(settings_path)?;
  // Split on the section header "[agent]"
  let blocks: Vec<&str> = ini_contents
    .split("[agent]")
    .filter(|b| !b.trim().is_empty())
    .collect();

  let mut agents = Vec::new();
  let mut errors: Vec<String> = Vec::new();
  for block in blocks {
    // Preprocess the block to remove surrounding quotes from values
    let mut clean_section = String::new();
    // Track ptt value string if present
    let _ptt_value_str: Option<String> = None;
    for line in block.lines() {
      if let Some(idx) = line.find('=') {
        let (key, val_part) = line.split_at(idx);
        // trim whitespace around key and value
        let key = key.trim();
        // val_part includes the '=' at start
        let val = &val_part[1..].trim();
        let val_trimmed = if val.starts_with('"') && val.ends_with('"') {
          &val[1..val.len() - 1]
        } else {
          val
        };
        clean_section.push_str(key);
        clean_section.push('=');
        clean_section.push_str(val_trimmed);
        clean_section.push('\n');
      }
      // skip lines without '=' (e.g., empty lines)
    }

    let section = clean_section.trim();

    // println!("DEBUG section: {}", section);
    // println!("DEBUG parsing section: {}", section);
    let mut agent: AgentSettings = match panic::catch_unwind(|| from_str::<AgentSettings>(&section))
    {
      Ok(Ok(a)) => a,
      Ok(Err(e)) => {
        print!("❌ Failed to parse agent's settings section: {}", e);
        thread::sleep(Duration::from_millis(30));
        return Err(e.into());
      }
      Err(_) => {
        print!("❌ Panic while parsing agent's section");
        thread::sleep(Duration::from_millis(30));
        return Err(Error::msg("panic while parsing agent's section"));
      }
    };
    // Sanitize quoted string values in AgentSettings before validation
    sanitize_agent_settings(&mut agent);

    // Validate individual agent
    if let Err(e) =
      validate_agent_name(&agent.name).map_err(|e: std::io::Error| -> Error { Error::new(e) })
    {
      errors.push(format!("Agent {}: {}", agent.name, e));
    }

    if let Err(e) =
      validate_provider(&agent.provider).map_err(|e: std::io::Error| -> Error { Error::new(e) })
    {
      errors.push(format!("Agent {}: {}", agent.name, e));
    }

    if let Err(e) =
      validate_model(&agent.model).map_err(|e: std::io::Error| -> Error { Error::new(e) })
    {
      errors.push(format!("Agent {}: {}", agent.name, e));
    }

    if let Err(e) =
      validate_baseurl(&agent.baseurl).map_err(|e: std::io::Error| -> Error { Error::new(e) })
    {
      errors.push(format!("Agent {}: {}", agent.name, e));
    }

    if let Err(e) = validate_system_prompt(&agent.system_prompt)
      .map_err(|e: std::io::Error| -> Error { Error::new(e) })
    {
      errors.push(format!("Agent {}: {}", agent.name, e));
    }

    if let Err(e) = validate_sound_threshold_peak(agent.sound_threshold_peak)
      .map_err(|e: std::io::Error| -> Error { Error::new(e) })
    {
      errors.push(format!("Agent {}: {}", agent.name, e));
    }

    if let Err(e) = validate_end_silence_ms(agent.end_silence_ms)
      .map_err(|e: std::io::Error| -> Error { Error::new(e) })
    {
      errors.push(format!("Agent {}: {}", agent.name, e));
    }

    if let Err(e) = validate_end_silence_ms(agent.end_silence_ms)
      .map_err(|e: std::io::Error| -> Error { Error::new(e) })
    {
      errors.push(format!("Agent {}: {}", agent.name, e));
    }

    if let Err(e) = validate_tts(&agent.tts).map_err(|e: std::io::Error| -> Error { Error::new(e) })
    {
      errors.push(format!("Agent {}: {}", agent.name, e));
    }

    if let Err(e) = validate_language(&agent.language, &agent.tts)
      .map_err(|e: std::io::Error| -> Error { Error::new(e) })
    {
      errors.push(format!("Agent {}: {}", agent.name, e));
    }

    if let Err(e) = validate_voice(&agent.voice, &agent.language, &agent.tts)
      .map_err(|e: std::io::Error| -> Error { Error::new(e) })
    {
      errors.push(format!("Agent {}: {}", agent.name, e));
    }

    agents.push(agent);
  }

  if !errors.is_empty() {
    print!("❌ {}", &errors.join("\n").to_string());
    thread::sleep(Duration::from_millis(30));
    process::exit(1);
  }

  if agents.is_empty() {
    return Err(Error::msg("No [agent] sections found in settings file"));
  }

  // Validate CLI args
  if let Some(ref agent_name) = args.agent {
    if let Err(e) =
      validate_agent_name(agent_name).map_err(|e: std::io::Error| -> Error { Error::new(e) })
    {
      return Err(e);
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

pub fn ensure_settings_file() -> Result<(), Error> {
  // Determine home directory
  let home =
    get_user_home_path().ok_or_else(|| Error::msg("Unable to determine home directory"))?;

  let ai_mate_dir = home.join(".ai-mate");
  // Ensure directory exists
  if !ai_mate_dir.exists() {
    create_dir_all(&ai_mate_dir)?;
  }
  let settings_path = ai_mate_dir.join("settings");
  // If file already exists, skip writing
  if settings_path.exists() {
    return Ok(());
  }
  let content = r#"[agent]
name = main agent
language = en
tts = kokoro
voice = bf_alice
provider = ollama
baseurl = http://127.0.0.1:11434
model = llama3.2:3b
system_prompt = You are a smart ai assistant. You reply to the user with the necessary information following the next rules:\n 1-Avoid suggestions unless they contribute to the specific user request.\n 2-If the user hasn't requested anything specific ask the exact questions to find out exactly what he needs assistance with.\n 3-Replies are no longer than 20 words unless a longer explanation is required.
sound_threshold_peak = 0.12
end_silence_ms = 2000
ptt = false
whisper_model_path = ~/.whisper-models/ggml-tiny.bin

[agent]
name = planner
language = en
tts = kokoro
voice = bm_daniel
provider = ollama
baseurl = http://127.0.0.1:11434
model = llama3.2:3b
system_prompt = You are an ai assistant which assist the user in the creation of a plan based on user's goal. When defining the plan follow the next format standards:\n 1-The plan is composed by tasks and subtasks.\n 2-Each task has the next format: "[ ] <task name>".\n 3-Subtasks are indented with 2 spaces below the parent task.\n 4-Before defining a plan, make sure you have the relevant information from the user.
sound_threshold_peak = 0.12
end_silence_ms = 2000
ptt = false
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

// PRIVATE
// ------------------------------------------------------------------

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
  if !voices.iter().any(|v| *v == voice_clean) {
    return Err(std::io::Error::new(
      std::io::ErrorKind::Other,
      format!("Unsupported voice '{}' for language {}", voice, language),
    ));
  } else {
    Ok(())
  }
}

fn validate_tts(tts: &str) -> Result<(), std::io::Error> {
  if tts != "kokoro" && tts != "opentts" {
    return Err(std::io::Error::new(
      std::io::ErrorKind::Other,
      format!("Invalid tts '{}' . Must be 'kokoro' or 'opentts'", tts),
    ));
  }
  Ok(())
}

fn validate_provider(provider: &str) -> Result<(), std::io::Error> {
  if provider != "ollama" && provider != "llama-server" {
    return Err(std::io::Error::new(
      std::io::ErrorKind::Other,
      format!(
        "Invalid provider '{}' . Must be 'ollama' or 'llama-server'",
        provider
      ),
    ));
  }
  Ok(())
}

fn validate_baseurl(baseurl: &str) -> Result<(), std::io::Error> {
  let url = Url::parse(baseurl).map_err(|e| {
    std::io::Error::new(
      std::io::ErrorKind::Other,
      format!("Invalid baseurl '{}' : {}", baseurl, e),
    )
  })?;
  if url.path() != "/" || !url.has_host() {
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
  agent.system_prompt = agent.system_prompt.trim_matches('"').to_string();
  // agent.ptt is a bool; no trimming needed
  agent.whisper_model_path = agent.whisper_model_path.trim_matches('"').to_string();
}
