// ------------------------------------------------------------------
//  Config
// ------------------------------------------------------------------

use crate::tts;
use crate::util::get_user_home_path;
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
  pub memory_enabled: String,
  pub memory_available_predicates: String,
  pub available_tools: String,
  pub ptt: String,
  pub whisper_model_path: String,
  pub sound_threshold_peak: f32,
  pub end_silence_ms: u64,
}

#[derive(Parser, Debug, Clone)]
pub struct Args {
  #[arg(long, action = clap::ArgAction::SetTrue)]
  pub verbose: bool,

  #[arg(long, default_value = "main agent", value_parser=validate_agent_name)]
  pub agent: String,

  #[arg(long)]
  pub llm: Option<String>,

  #[arg(long)]
  pub tts: Option<String>,

  #[arg(long)]
  pub whisper_model_path: Option<String>,

  #[arg(long)]
  pub language: Option<String>,

  #[arg(long)]
  pub voice: Option<String>,

  #[arg(long)]
  pub sound_threshold_peak: Option<f32>,

  #[arg(long)]
  pub end_silence_ms: Option<u64>,

  #[arg(long)]
  pub ollama_url: Option<String>,

  #[arg(long)]
  pub model: Option<String>,

  #[arg(long)]
  pub llama_server_url: Option<String>,

  #[arg(long)]
  pub opentts_base_url: Option<String>,

  #[arg(long, action=clap::ArgAction::SetTrue)]
  pub list_voices: bool,

  #[arg(long, action=clap::ArgAction::SetTrue)]
  pub ptt: bool,
}

// internal static values
pub const HANGOVER_MS_DEFAULT: u64 = 300;
pub const MIN_UTTERANCE_MS_DEFAULT: u64 = 300;
pub const OPENTTS_BASE_URL_DEFAULT: &str = "http://127.0.0.1:5500/api/tts?&vocoder=high&denoiserStrength=0.005&&speakerId=&ssml=false&ssmlNumbers=true&ssmlDates=true&ssmlCurrency=true&cache=false";

pub fn resolved_whisper_model_path(args: &Args) -> String {
  let path = args
    .whisper_model_path
    .clone()
    .unwrap_or_else(|| "~/.whisper-models/ggml-tiny.bin".to_string());
  if path.starts_with("~") {
    if let Some(home) = get_user_home_path() {
      let rel = path.trim_start_matches("~");
      let mut p = home;
      p.push(&rel[1..]);
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
) -> Result<Vec<AgentSettings>, Box<dyn std::error::Error + Send + Sync>> {
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
    // prepend a dummy header so serde_ini can parse it
    let section = block.trim();
    let mut agent: AgentSettings = match panic::catch_unwind(|| from_str::<AgentSettings>(&section))
    {
      Ok(Ok(a)) => a,
      Ok(Err(e)) => {
        crate::log::log("error", &format!("Failed to parse agent section: {}\n", e));
        thread::sleep(Duration::from_millis(30));
        return Err(Box::new(e));
      }
      Err(_) => {
        crate::log::log("error", "Panic while parsing agent section");
        thread::sleep(Duration::from_millis(30));
        return Err("panic while parsing agent section".into());
      }
    };
    // Sanitize quoted string values in AgentSettings before validation
    sanitize_agent_settings(&mut agent);

    // Validate individual agent
    if let Err(e) = validate_agent_name(&agent.name) {
      crate::log::log("error", &format!("Agent {}: {}", agent.name, e));
      thread::sleep(Duration::from_millis(30));
      process::exit(0);
    }
    if let Some(tts) = &args.tts {
      if let Err(e) = validate_language(&agent.language, tts) {
        crate::log::log("error", &format!("Agent {}: {}", agent.name, e));
        thread::sleep(Duration::from_millis(30));
        process::exit(0);
      }
    }
    if let Some(tts) = &args.tts {
      if let Err(e) = validate_voice(&agent.voice, &agent.language, tts) {
        crate::log::log("error", &format!("Agent {}: {}", agent.name, e));
        thread::sleep(Duration::from_millis(30));
        process::exit(0);
      }
    }

    if let Err(e) = validate_provider(&agent.provider) {
      crate::log::log("error", &format!("Agent {}: {}", agent.name, e));
      thread::sleep(Duration::from_millis(30));
      process::exit(0);
    }
    if let Err(e) = validate_baseurl(&agent.baseurl) {
      crate::log::log("error", &format!("Agent {}: {}", agent.name, e));
      thread::sleep(Duration::from_millis(30));
      process::exit(0);
    }
    if let Err(e) = validate_model(&agent.model) {
      crate::log::log("error", &format!("Agent {}: {}", agent.name, e));
      thread::sleep(Duration::from_millis(30));
      process::exit(0);
    }
    if let Err(e) = validate_system_prompt(&agent.system_prompt) {
      crate::log::log("error", &format!("Agent {}: {}", agent.name, e));
      thread::sleep(Duration::from_millis(30));
      process::exit(0);
    }
    if let Err(e) = validate_sound_threshold_peak(agent.sound_threshold_peak) {
      crate::log::log("error", &format!("Agent {}: {}", agent.name, e));
      thread::sleep(Duration::from_millis(30));
      process::exit(0);
    }
    if let Err(e) = validate_end_silence_ms(agent.end_silence_ms) {
      crate::log::log("error", &format!("Agent {}: {}", agent.name, e));
      thread::sleep(Duration::from_millis(30));
      process::exit(0);
    }

    if let Err(e) = validate_tts(&agent.tts) {
      crate::log::log("error", &format!("Agent {}: {}", agent.name, e));
      thread::sleep(Duration::from_millis(30));
      process::exit(0);
    }

    agents.push(agent);
  }

  if !errors.is_empty() {
    for err in &errors {
      crate::log::log("error", &format!("Error: {}", err));
    }
    return Err(errors.join("\n").into());
  }

  if agents.is_empty() {
    return Err("No [agent] sections found in settings file".into());
  }

  // Validate CLI args against the loaded agents
  if let Err(e) = validate_agent_name(&args.agent) {
    return Err(e.into());
  }
  if let Some(lang) = &args.language {
    if let Some(tts) = &args.tts {
      if let Err(e) = validate_language(lang, tts) {
        return Err(e.into());
      }
    }
  }
  if let Some(v) = &args.voice {
    if let Some(lang) = &args.language {
      if let Some(tts) = &args.tts {
        if let Err(e) = validate_voice(v, lang, tts) {
          return Err(e.into());
        }
      }
    }
  }
  if let Some(v) = &args.llm {
    if let Err(e) = validate_provider(v) {
      return Err(e.into());
    }
  }
  if let Some(v) = &args.llm {
    if v == "ollama" {
      if let Some(u) = &args.ollama_url {
        if let Err(e) = validate_baseurl(u) {
          return Err(e.into());
        }
      }
    } else {
      if let Some(u) = &args.llama_server_url {
        if let Err(e) = validate_baseurl(u) {
          return Err(e.into());
        }
      }
    }
  }
  // validate optional model if provided
  if let Some(v) = &args.model {
    if let Err(e) = validate_model(v) {
      return Err(e.into());
    }
  }
  // Validate optional CLI arguments if provided
  if let Some(v) = args.sound_threshold_peak {
    if let Err(e) = validate_sound_threshold_peak(v) {
      return Err(e.into());
    }
  }
  if let Some(v) = args.end_silence_ms {
    if let Err(e) = validate_end_silence_ms(v) {
      return Err(e.into());
    }
  }
  if let Some(v) = &args.tts {
    if let Err(e) = validate_tts(v) {
      return Err(e.into());
    }
  }
  if let Some(v) = &args.llm {
    if let Err(e) = validate_provider(v) {
      return Err(e.into());
    }
  }
  if let Some(v) = &args.llm {
    if v == "ollama" {
      if let Some(u) = &args.ollama_url {
        if let Err(e) = validate_baseurl(u) {
          return Err(e.into());
        }
      }
    } else {
      if let Some(u) = &args.llama_server_url {
        if let Err(e) = validate_baseurl(u) {
          return Err(e.into());
        }
      }
    }
  }
  if let Some(v) = &args.language {
    if let Some(tts) = &args.tts {
      if let Err(e) = validate_language(v, tts) {
        return Err(e.into());
      }
    }
  }
  // Merge args into each agent's settings
  for agent in agents.iter_mut() {
    if let Some(v) = args.language.clone() {
      agent.language = v;
    }
    if let Some(v) = args.tts.clone() {
      agent.tts = v;
    }
    if let Some(v) = args.llm.clone() {
      agent.provider = v;
    }
    if let Some(v) = args.llm.clone() {
      if v == "ollama" {
        if let Some(u) = args.ollama_url.clone() {
          agent.baseurl = u;
        }
      } else {
        if let Some(u) = args.llama_server_url.clone() {
          agent.baseurl = u;
        }
      }
    }
    if let Some(v) = args.model.clone() {
      agent.model = v;
    }
    let ptt = args.ptt.clone();
    if ptt {
      agent.ptt = ptt.to_string();
    }
    if let Some(v) = args.whisper_model_path.clone() {
      agent.whisper_model_path = v;
    }
    if let Some(v) = args.sound_threshold_peak {
      agent.sound_threshold_peak = v;
    }
    if let Some(v) = args.end_silence_ms {
      agent.end_silence_ms = v;
    }
    if let Some(v) = &args.voice {
      agent.voice = v.clone();
    }
  }

  Ok(agents)
}

pub fn ensure_settings_file() -> Result<(), Box<dyn std::error::Error>> {
  // Determine home directory
  let home = get_user_home_path().ok_or("Unable to determine home directory")?;
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
voice = bf_alice
provider = ollama
baseurl = http://127.0.0.1:11434
model = llama3.2:3b
system_prompt = You are a smart ai assistant. You reply to the user with the necessary information following the next rules: Avoid suggestions unless they contribute to the specific user request. If the user hasn't requested anything specific ask the exact questions to find out exactly what he needs assistance with. Replies are no longer than 20 words unless a longer explanation is required.
sound_threshold_peak = 0.1
end_silence_ms = 2000
tts = kokoro
ptt = false
whisper_model_path = ~/.whisper-models/ggml-tiny.bin
memory_enabled = true
memory_available_predicates = 
available_tools = read_file, list_files, find_in_files, webfetch

[agent]
name = planner
language = en
voice = bf_alice
provider = ollama
baseurl = http://127.0.0.1:11434
model = llama3:8b
system_prompt = You are an ai assistant which assist the user in the creation of a plan based on user's goal. The plan is composed by tasks and subtasks. Each task has the next format: "[ ] <task name>". Subtasks are indented with 2 spaces below the parent task. Before defining a plan, make sure you have the relevant information from the user.
sound_threshold_peak = 0.1
end_silence_ms = 2000
tts = kokoro
ptt = false
whisper_model_path = ~/.whisper-models/ggml-tiny.bin
memory_enabled = true
memory_available_predicates = 
available_tools = read_file, list_files, find_in_files, webfetch
"#;
  let mut file = File::create(&settings_path)?;
  file.write_all(content.as_bytes())?;
  Ok(())
}

pub fn pick_input_config(
  device: &Device,
  preferred_sr: u32,
) -> Result<cpal::SupportedStreamConfig, Box<dyn std::error::Error + Send + Sync>> {
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
    .ok_or_else(|| "no supported input configs".into())
}

// PRIVATE
// ------------------------------------------------------------------

fn validate_agent_name(name: &str) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
  let len = name.chars().count();
  if len < 1 || len > 200 {
    Err(Box::new(std::io::Error::new(
      std::io::ErrorKind::InvalidInput,
      "agent must be between 1 and 200 characters",
    )))
  } else {
    Ok(name.to_string())
  }
}

fn validate_language(
  language: &str,
  tts: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
  let lang_clean = language.trim_matches('"');
  let langs = tts::get_all_available_languages();
  if !langs.contains(&lang_clean) {
    let err = format!("Unsupported language: {}", language);
    crate::log::log("error", &err);
    return Err(err.into());
  }
  let voices = tts::get_voices_for(tts, lang_clean);
  if voices.is_empty() {
    let err = format!("No voices for language {} and TTS {}", language, tts);
    crate::log::log("error", &err);
    return Err(err.into());
  }

  // Ensure the selected TTS engine supports this language
  let voices = tts::get_voices_for(tts, lang_clean);
  if voices.is_empty() {
    let err = format!(
      "No available voices for TTS '{}' and language '{}'",
      tts, language
    );
    crate::log::log("error", &err);
    return Err(err.into());
  }
  Ok(())
}

fn validate_voice(
  voice: &str,
  language: &str,
  tts: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
  let lang_clean = language.trim_matches('"');
  let voices = tts::get_voices_for(tts, lang_clean);
  if voices.is_empty() {
    return Err(
      format!(
        "No available voices for TTS '{}' and language '{}'",
        tts, language,
      )
      .into(),
    );
  }

  let voice_clean = voice.trim_matches('"');
  if !voices.iter().any(|v| *v == voice_clean) {
    return Err(format!("Unsupported voice '{}' for language {}", voice, language).into());
  }
  Ok(())
}

fn validate_tts(tts: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
  if tts != "kokoro" && tts != "opentts" {
    return Err(format!("Invalid tts '{}' . Must be 'kokoro' or 'opentts'", tts).into());
  }
  Ok(())
}

fn validate_provider(provider: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
  if provider != "ollama" && provider != "llama-server" {
    return Err(
      format!(
        "Invalid provider '{}' . Must be 'ollama' or 'llama-server'",
        provider
      )
      .into(),
    );
  }
  Ok(())
}

fn validate_baseurl(baseurl: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
  let url = Url::parse(baseurl).map_err(|e| format!("Invalid baseurl '{}': {}", baseurl, e))?;
  if url.path() != "/" || !url.has_host() {
    return Err(format!("baseurl must have a host and no path: {}", baseurl).into());
  }
  Ok(())
}

fn validate_model(model: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
  if model.is_empty() || model.len() > 200 {
    return Err("'model' must be 1-200 characters".into());
  }
  Ok(())
}

fn validate_system_prompt(prompt: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
  if prompt.is_empty() || prompt.len() > 20000 {
    return Err("'system_prompt' must be 0-20000 characters".into());
  }
  Ok(())
}

fn validate_sound_threshold_peak(
  value: f32,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
  if value < 0.0 || value > 1.0 {
    return Err("'sound_threshold_peak' must be between 0.0 and 1.0".into());
  }
  let scaled = (value * 1000.0).round();
  if (scaled / 1000.0 - value).abs() > 1e-6 {
    return Err("'sound_threshold_peak' must have at most 3 decimal places".into());
  }
  Ok(())
}

fn validate_end_silence_ms(value: u64) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
  if value < 1 || value > 20000 {
    return Err("'end_silence_ms' must be between 1 and 20000".into());
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
  agent.memory_enabled = agent.memory_enabled.trim_matches('"').to_string();
  agent.memory_available_predicates = agent
    .memory_available_predicates
    .trim_matches('"')
    .to_string();
  agent.available_tools = agent.available_tools.trim_matches('"').to_string();
  agent.ptt = agent.ptt.trim_matches('"').to_string();
  agent.whisper_model_path = agent.whisper_model_path.trim_matches('"').to_string();
}
