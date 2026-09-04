use std::env::temp_dir;
use std::fs::File;
use std::io::Write;
use std::time::{SystemTime, UNIX_EPOCH};

// --- Stubs for binary modules ---------------------------------
mod tts {
  pub fn get_all_available_languages() -> Vec<&'static str> {
    vec!["en"]
  }
  pub fn get_voices_for(_tts: &str, lang: &str) -> Vec<String> {
    // Provide a voice matching the config
    if lang == "en" {
      vec!["bf_alice".to_string()]
    } else {
      vec![format!("voice-{}", lang)]
    }
  }
}

mod util {
  use std::path::PathBuf;
  pub fn get_user_home_path() -> Option<PathBuf> {
    Some(PathBuf::from("/tmp"))
  }
  pub fn terminate(exit_code: i32) -> ! {
    std::process::exit(exit_code)
  }
}

mod log {
  pub fn log(_level: &str, _msg: &str) {}
}

mod tools {
  pub mod http_request {
    pub struct HttpToolDefinition {
      pub name: String,
    }
    pub struct HttpRequestDefinition {
      pub tool_definition: HttpToolDefinition,
    }
    pub fn load_http_request_definitions() -> Vec<HttpRequestDefinition> {
      vec![]
    }
  }
}

#[path = "../src/config.rs"]
mod config;

use config::{Args, load_settings};

#[test]
fn test_load_settings_with_double_quotes() {
  // Create a temporary config file with quoted values
  let mut path = temp_dir();
  path.push(format!(
    "ai_mate_test_config_{}.ini",
    SystemTime::now()
      .duration_since(UNIX_EPOCH)
      .unwrap()
      .as_nanos()
  ));

  let contents = r#"
[agent]
name = "main agent"
language = "en"
tts = "kokoro"
voice = "bf_alice"
provider = "ollama"
baseurl = "http://127.0.0.1:11434"
model = "llama3.2:3b"
system_prompt = "You are a helpful assistant.\nYou assist the user without questions"
sound_threshold_peak = "0.1"
end_silence_ms = "2000"
ptt = "false"
whisper_model_path = "~/.whisper-models/ggml-tiny.bin"
voice_speed = 5.0
tools = web_fetch
"#;

  let mut file = File::create(&path).expect("Failed to create temp config file");
  file
    .write_all(contents.as_bytes())
    .expect("Failed to write to temp config file");

  // Prepare args with defaults
  let args = Args {
    config: None,
    prompt: None,
    prompt_file: None,
    verbose: false,
    agent: Some("main agent".to_string()),
    list_voices: false,
    ptt: Some(true),
    debate: None,
    read_file: None,
    quiet: false,
    save: false,
  };

  let agents = load_settings(&path, &args).expect("Failed to load settings");
  assert_eq!(agents.len(), 1);
  let agent = &agents[0];
  assert_eq!(agent.name, "main agent");
  assert_eq!(agent.language, "en");
  assert_eq!(agent.tts, "kokoro");
  assert_eq!(agent.voice, "bf_alice");
  assert_eq!(agent.provider, "ollama");
  assert_eq!(agent.baseurl, "http://127.0.0.1:11434");
  assert_eq!(agent.model, "llama3.2:3b");
  assert_eq!(
    agent.system_prompt,
    "You are a helpful assistant.\\nYou assist the user without questions"
  );
  assert_eq!(agent.ptt, true);
  assert_eq!(agent.sound_threshold_peak, 0.1);
  assert_eq!(agent.end_silence_ms, 2000);
  assert_eq!(agent.voice_speed, 5.0);
  assert_eq!(agent.whisper_model_path, "~/.whisper-models/ggml-tiny.bin");
  assert_eq!(agent.tools, vec!["web_fetch".to_string()]);
}

#[test]
fn test_load_settings() {
  // Create a temporary config file with quoted values
  let mut path = temp_dir();
  path.push(format!(
    "ai_mate_test_config_{}.ini",
    SystemTime::now()
      .duration_since(UNIX_EPOCH)
      .unwrap()
      .as_nanos()
  ));

  let contents = r#"
[agent]
name = main agent
language = en
tts = kokoro
voice = bf_alice
provider = ollama
baseurl = http://127.0.0.1:11434
model = llama3.2:3b
system_prompt = You are a helpful assistant.\nYou assist the user without questions
sound_threshold_peak = 0.1
end_silence_ms = 2000
ptt = true
whisper_model_path = ~/.whisper-models/ggml-tiny.bin
voice_speed = 5.0
tools = web_fetch
"#;

  let mut file = File::create(&path).expect("Failed to create temp config file");
  file
    .write_all(contents.as_bytes())
    .expect("Failed to write to temp config file");

  // Prepare args with defaults
  let args = Args {
    config: None,
    prompt: None,
    prompt_file: None,
    verbose: false,
    agent: Some("Test Agent".to_string()),
    list_voices: false,
    ptt: None,
    debate: None,
    read_file: None,
    quiet: false,
    save: false,
  };

  let agents = load_settings(&path, &args).expect("Failed to load settings");
  assert_eq!(agents.len(), 1);
  let agent = &agents[0];
  assert_eq!(agent.name, "main agent");
  assert_eq!(agent.language, "en");
  assert_eq!(agent.tts, "kokoro");
  assert_eq!(agent.voice, "bf_alice");
  assert_eq!(agent.provider, "ollama");
  assert_eq!(agent.baseurl, "http://127.0.0.1:11434");
  assert_eq!(agent.model, "llama3.2:3b");
  assert_eq!(
    agent.system_prompt,
    "You are a helpful assistant.\\nYou assist the user without questions"
  );
  assert_eq!(agent.ptt, true);
  assert_eq!(agent.sound_threshold_peak, 0.1);
  assert_eq!(agent.end_silence_ms, 2000);
  assert_eq!(agent.voice_speed, 5.0);
  assert_eq!(agent.whisper_model_path, "~/.whisper-models/ggml-tiny.bin");
  assert_eq!(agent.tools, vec!["web_fetch".to_string()]);
}
