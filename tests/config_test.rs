use std::env::temp_dir;
use std::fs::File;
use std::io::Write;

// --- Stubs for binary modules ---------------------------------
mod tts {
  pub fn get_all_available_languages() -> Vec<&'static str> {
    vec!["en"]
  }
  pub fn get_voices_for(_tts: &str, lang: &str) -> Vec<String> {
    vec![format!("voice-{}", lang)]
  }
}

mod util {
  use std::path::PathBuf;
  pub fn get_user_home_path() -> Option<PathBuf> {
    Some(PathBuf::from("/tmp"))
  }
}

mod log {
  pub fn log(_level: &str, _msg: &str) {}
}

#[path = "../src/config.rs"]
mod config;

use config::{load_settings, AgentSettings, Args};

#[test]
fn test_load_settings_with_double_quotes() {
  // Create a temporary config file with quoted values
  let mut path = temp_dir();
  path.push("ai_mate_test_config.ini");

  let contents = r#"
[agent]
name="Test Agent"
language="en"
tts="kokoro"
voice="default"
provider="ollama"
baseurl="http://localhost"
model="gpt-4o"
system_prompt="You are a helpful assistant.\nYou assist the user without questions"
memory_enabled="false"
memory_available_predicates="true"
available_tools="tool1,tool2"
ptt="true"
sound_threshold_peak=0.5
end_silence_ms=1000
whisper_model_path="~/.whisper-models/ggml-tiny.bin"
"#;

  let mut file = File::create(&path).expect("Failed to create temp config file");
  file
    .write_all(contents.as_bytes())
    .expect("Failed to write to temp config file");

  // Prepare args with defaults
  let args = Args {
    verbose: false,
    agent: "Test Agent".to_string(),
    llm: None,
    tts: None,
    whisper_model_path: None,
    language: None,
    voice: None,
    sound_threshold_peak: None,
    end_silence_ms: None,
    ollama_url: None,
    model: None,
    llama_server_url: None,
    opentts_base_url: None,
    list_voices: false,
    ptt: false,
  };

  let agents = load_settings(&path, &args).expect("Failed to load settings");
  assert_eq!(agents.len(), 1);
  let agent = &agents[0];
  assert_eq!(agent.name, "Test Agent");
  assert_eq!(agent.language, "en");
  assert_eq!(agent.tts, "kokoro");
  assert_eq!(agent.voice, "default");
  assert_eq!(agent.provider, "ollama");
  assert_eq!(agent.baseurl, "http://localhost");
  assert_eq!(agent.model, "gpt-4o");
  assert_eq!(agent.system_prompt, "You are a helpful assistant.\\nYou assist the user without questions");
  assert_eq!(agent.memory_enabled, "false");
  assert_eq!(agent.memory_available_predicates, "true");
  assert_eq!(agent.available_tools, "tool1,tool2");
  assert_eq!(agent.ptt, "true");
  assert_eq!(agent.whisper_model_path, "~/.whisper-models/ggml-tiny.bin");
}


#[test]
fn test_load_settings() {
  // Create a temporary config file with quoted values
  let mut path = temp_dir();
  path.push("ai_mate_test_config.ini");

  let contents = r#"
[agent]
name=Test Agent
language=en
tts=kokoro
voice=default
provider=ollama
baseurl=http://localhost
model=gpt-4o
system_prompt=You are a helpful assistant.\nYou assist the user without questions
memory_enabled=false
memory_available_predicates=true
available_tools=tool1, tool2
ptt=true
sound_threshold_peak=0.5
end_silence_ms=1000
whisper_model_path=~/.whisper-models/ggml-tiny.bin
"#;

  let mut file = File::create(&path).expect("Failed to create temp config file");
  file
    .write_all(contents.as_bytes())
    .expect("Failed to write to temp config file");

  // Prepare args with defaults
  let args = Args {
    verbose: false,
    agent: "Test Agent".to_string(),
    llm: None,
    tts: None,
    whisper_model_path: None,
    language: None,
    voice: None,
    sound_threshold_peak: None,
    end_silence_ms: None,
    ollama_url: None,
    model: None,
    llama_server_url: None,
    opentts_base_url: None,
    list_voices: false,
    ptt: false,
  };

  let agents = load_settings(&path, &args).expect("Failed to load settings");
  assert_eq!(agents.len(), 1);
  let agent = &agents[0];
  assert_eq!(agent.name, "Test Agent");
  assert_eq!(agent.language, "en");
  assert_eq!(agent.tts, "kokoro");
  assert_eq!(agent.voice, "default");
  assert_eq!(agent.provider, "ollama");
  assert_eq!(agent.baseurl, "http://localhost");
  assert_eq!(agent.model, "gpt-4o");
  assert_eq!(agent.system_prompt, "You are a helpful assistant.\\nYou assist the user without questions");
  assert_eq!(agent.memory_enabled, "false");
  assert_eq!(agent.memory_available_predicates, "true");
  assert_eq!(agent.available_tools, "tool1,tool2");
  assert_eq!(agent.ptt, "true");
  assert_eq!(agent.whisper_model_path, "~/.whisper-models/ggml-tiny.bin");
}
