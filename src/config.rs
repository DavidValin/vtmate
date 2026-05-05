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
use serde::Deserialize;
use serde_ini::from_str;
use std::fs::{File, create_dir_all, read_to_string};
use std::io::Write;
use std::panic;
use std::thread::{self};
use std::time::Duration;
use url::Url;

// API
// ------------------------------------------------------------------

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
  pub voice_speed: f32,
  #[serde(default, deserialize_with = "parse_tools")]
  pub tools: Vec<String>,
}

#[derive(Parser, Debug, Clone)]
#[clap(version = env!("CARGO_PKG_VERSION"))]
#[clap(after_help = r#"
Settings file is at ~/.vtmate/settings

Explanation on the fields:

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
                          the llm, it can be 'ollama' or
                          'llama-server'
  ------------------------------------------------------------
  * baseurl:              the base url used to contact the
                          provider (it needs to be without path)
  ------------------------------------------------------------
  * model:                the model name to use in ollama
                          (some llama-server versions will
                          ignore this option as llama-server
                          runs for a single model)
  ------------------------------------------------------------
  * system_prompt:        the system prompt to be sent to
                          the llm when querying it.
                          Use \n for new lines
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
                          'kokoro' or 'opentts'.

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
                          ~/.whisper-models, tiny and small.
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
}

// internal static values
pub const HANGOVER_MS_DEFAULT: u64 = 300;
pub const MIN_UTTERANCE_MS_DEFAULT: u64 = 300;
pub const OPENTTS_BASE_URL_DEFAULT: &str = "http://127.0.0.1:5500/api/tts?&vocoder=high&denoiserStrength=0.005&&speakerId=&ssml=false&ssmlNumbers=true&ssmlDates=true&ssmlCurrency=true&cache=false";

/// Parse a comma-separated string into a Vec<String>. Empty values are filtered out.
fn parse_tools<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
  D: serde::de::Deserializer<'de>,
{
  let s = String::deserialize(deserializer)?;
  let tools: Vec<String> = s
    .split(',')
    .map(|t| t.trim().to_string())
    .filter(|t| !t.is_empty())
    .collect();
  Ok(tools)
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

    if let Err(e) = validate_voice_speed(agent.voice_speed)
      .map_err(|e: std::io::Error| -> Error { Error::new(e) })
    {
      errors.push(format!("Agent {}: {}", agent.name, e));
    }

    if let Err(e) =
      validate_tools(&agent.tools).map_err(|e: std::io::Error| -> Error { Error::new(e) })
    {
      errors.push(format!("Agent {}: {}", agent.name, e));
    }

    agents.push(agent);
  }

  if !errors.is_empty() {
    print!("❌ {}", &errors.join("\n").to_string());
    thread::sleep(Duration::from_millis(30));
    terminate(1);
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
  let content = r#"
[agent]
name = main agent
language = en
tts = supersonic2
voice = M1
voice_speed = 1.1
provider = ollama
baseurl = http://127.0.0.1:11434
model = llama3.2:3b
system_prompt = "You are a neutral, helpful AI assistant. Follow the subject of the conversation with special attention to the user request. Provide accurate, concise answers. Keep replies ≤30 words; if a longer answer is required, limit it to 250 words. Assume no prior context unless the user supplies it, and do not mention yourself."
sound_threshold_peak = 0.12
end_silence_ms = 2500
ptt = true
whisper_model_path = ~/.whisper-models/ggml-tiny.bin
tools = web_fetch

[agent]
name = explainer
language = en
tts = supersonic2
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
tools = web_fetch

[agent]
name = explainer
language = en
tts = supersonic2
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
tools = web_fetch

[agent]
name = Ptahhotep
language = en
tts = supersonic2
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
tools = web_fetch

[agent]
name = Aristoteles
language = en
tts = supersonic2
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
tools = web_fetch

[agent]
name = Budda
language = en
tts = supersonic2
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
tools = web_fetch

[agent]
name = Jesus Christ
language = en
tts = supersonic2
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
  // Validate voice format, supports mix of two voices
  let lang_clean = language.trim_matches('"');
  let voices_raw = tts::get_voices_for(tts, lang_clean);
  let voices: Vec<String> = voices_raw.iter().map(|s| s.to_string()).collect();
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
  validate_voice_value(voice_clean, &voices, language)
}

fn validate_tts(tts: &str) -> Result<(), std::io::Error> {
  if tts != "kokoro" && tts != "opentts" && tts != "supersonic2" {
    return Err(std::io::Error::new(
      std::io::ErrorKind::Other,
      format!(
        "Invalid tts '{}' . Must be 'kokoro', 'opentts', or 'supersonic2'",
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

fn validate_tools(tools: &[String]) -> Result<(), std::io::Error> {
  // Collect valid static tool names
  let mut valid_tools: Vec<String> = vec![
    "web_fetch".to_string(),
    "bash_command".to_string(),
    "glob".to_string(),
    "grep".to_string(),
    "read_file".to_string(),
    "search".to_string(),
    "apply_patch".to_string(),
  ];
  // Add dynamically loaded HTTP request tool names
  for def in crate::tools::http_request::load_http_request_definitions() {
    valid_tools.push(def.tool_definition.name);
  }

  for tool in tools {
    if !valid_tools.iter().any(|t| t == tool) {
      return Err(std::io::Error::new(
        std::io::ErrorKind::Other,
        format!(
          "Unknown tool '{}'. Valid tools: {}",
          tool,
          valid_tools.join(", ")
        ),
      ));
    }
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
  // tools is Vec<String> from the deserializer, no trimming needed
}
