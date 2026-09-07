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
  pub fn terminate(code: i32) -> ! {
    std::process::exit(code)
  }
}

mod log {
  pub fn log(_level: &str, _msg: &str) {}
}

mod llm {
  pub fn is_local_provider(provider: &str) -> bool {
    matches!(provider, "ollama" | "llama-server" | "openai-compatible")
  }
  pub fn is_cloud_provider(provider: &str) -> bool {
    matches!(provider, "openai" | "anthropic")
  }
  pub fn is_supported_provider(provider: &str) -> bool {
    is_local_provider(provider) || is_cloud_provider(provider)
  }
  pub fn supported_providers_list() -> String {
    "ollama, llama-server, openai-compatible, openai, anthropic".to_string()
  }
  pub fn api_key_env_var(_provider: &str) -> Option<&'static str> {
    Some("API_KEY")
  }
  pub fn resolve_api_key(_provider: &str, configured: &str) -> Option<String> {
    if configured.trim().is_empty() {
      None
    } else {
      Some(configured.to_string())
    }
  }
}

#[path = "../src/config.rs"]
mod config;

use config::{
  Args, DaemonSettings, GeneralSettings, load_daemon_settings,
  load_general_settings, load_settings, persist_selected_agent, select_agent,
  split_leading_sections,
};

fn temp_settings(contents: &str) -> std::path::PathBuf {
  let mut path = temp_dir();
  path.push(format!(
    "vtmate_test_settings_{}_{}.ini",
    std::process::id(),
    SystemTime::now()
      .duration_since(UNIX_EPOCH)
      .unwrap()
      .as_nanos()
  ));
  let mut file = File::create(&path).expect("Failed to create temp config file");
  file
    .write_all(contents.as_bytes())
    .expect("Failed to write to temp config file");
  path
}

fn default_args() -> Args {
  Args {
    config: None,
    prompt: None,
    prompt_file: None,
    verbose: false,
    agent: None,
    list_voices: false,
    ptt: None,
    debate: None,
    read_file: None,
    quiet: false,
    save: false,
    daemon: false,
    daemon_foreground: false,
    daemon_stop: false,
    daemon_status: false,
  }
}

const AGENT_A: &str = r#"[agent]
name = main agent
language = en
tts = kokoro
voice = bf_alice
provider = ollama
baseurl = http://127.0.0.1:11434
model = llama3.2:3b
system_prompt = You are a helpful assistant.
sound_threshold_peak = 0.1
end_silence_ms = 2000
ptt = true
whisper_model_path = ~/.whisper-models/ggml-tiny.bin
voice_speed = 5.0
"#;

const AGENT_B: &str = r#"[agent]
name = explainer
language = en
tts = kokoro
voice = bf_alice
provider = ollama
baseurl = http://127.0.0.1:11434
model = llama3.2:3b
system_prompt = You explain things.
sound_threshold_peak = 0.1
end_silence_ms = 2000
ptt = true
whisper_model_path = ~/.whisper-models/ggml-tiny.bin
voice_speed = 5.0
"#;

#[test]
fn leading_sections_are_split_out_and_agents_still_parse() {
  let contents = format!(
    "[general]\nselected_agent = explainer\n\n[daemon]\nllm_background_ptt_combo = ctrl+alt+q\n\n{}\n{}",
    AGENT_A, AGENT_B
  );
  let sections = split_leading_sections(&contents);
  assert!(sections.general.unwrap().contains("selected_agent = explainer"));
  assert!(sections.daemon.unwrap().contains("ctrl+alt+q"));
  assert!(sections.rest.starts_with("[agent]"));
  assert_eq!(sections.rest.matches("[agent]").count(), 2);

  let path = temp_settings(&contents);
  let agents = load_settings(&path, &default_args()).expect("agents parse with leading sections");
  assert_eq!(agents.len(), 2);
  assert_eq!(agents[0].name, "main agent");
  assert_eq!(agents[1].name, "explainer");

  let general = load_general_settings(&path).unwrap();
  assert_eq!(general.selected_agent, "explainer");
  let daemon = load_daemon_settings(&path).unwrap();
  assert_eq!(daemon.llm_background_ptt_combo, "ctrl+alt+q");
  assert_eq!(daemon.tts_background_combo, "ctrl+alt+r");
  assert_eq!(daemon.stt_and_paste_background_ptt_combo, "ctrl+alt+s");
  assert_eq!(daemon.llm_background_reset, "ctrl+escape");
}

#[test]
fn missing_sections_give_defaults() {
  let path = temp_settings(&format!("{}\n{}", AGENT_A, AGENT_B));
  let general = load_general_settings(&path).unwrap();
  assert_eq!(general.selected_agent, "");
  let daemon = load_daemon_settings(&path).unwrap();
  assert_eq!(daemon.llm_background_ptt_combo, DaemonSettings::default().llm_background_ptt_combo);
  let agents = load_settings(&path, &default_args()).unwrap();
  let picked = select_agent(&agents, None, &general).unwrap();
  assert_eq!(picked.name, "main agent");
}

#[test]
fn select_agent_precedence() {
  let path = temp_settings(&format!("{}\n{}", AGENT_A, AGENT_B));
  let agents = load_settings(&path, &default_args()).unwrap();
  let general = GeneralSettings { selected_agent: "explainer".to_string() };
  assert_eq!(select_agent(&agents, None, &general).unwrap().name, "explainer");
  // -a wins over the persisted selection
  assert_eq!(
    select_agent(&agents, Some("main agent"), &general).unwrap().name,
    "main agent"
  );
  // unknown -a is an error
  assert!(select_agent(&agents, Some("nobody"), &general).is_err());
  // unknown persisted selection falls back to the first agent
  let general = GeneralSettings { selected_agent: "nobody".to_string() };
  assert_eq!(select_agent(&agents, None, &general).unwrap().name, "main agent");
}

#[test]
fn persist_selected_agent_inserts_section_on_top_and_keeps_agents() {
  let original = format!("\n{}\n{}", AGENT_A, AGENT_B);
  let path = temp_settings(&original);
  persist_selected_agent(&path, "explainer").unwrap();
  let after = std::fs::read_to_string(&path).unwrap();
  assert!(after.starts_with("[general]\nselected_agent = explainer\n\n"));
  assert!(after.ends_with(original.trim_start_matches('\n')));
  assert_eq!(load_general_settings(&path).unwrap().selected_agent, "explainer");
  assert_eq!(load_settings(&path, &default_args()).unwrap().len(), 2);
}

#[test]
fn persist_selected_agent_replaces_only_that_line() {
  let original = format!(
    "[general]\nselected_agent = \"main agent\"\nother = keep me\n\n[daemon]\ntts_background_combo = ctrl+alt+t\n\n{}",
    AGENT_A
  );
  let path = temp_settings(&original);
  persist_selected_agent(&path, "explainer").unwrap();
  let after = std::fs::read_to_string(&path).unwrap();
  let expected = original.replace("selected_agent = \"main agent\"", "selected_agent = explainer");
  assert_eq!(after, expected);
  // a section without the key gets it appended right after the header block
  let path = temp_settings(&format!("[general]\n\n{}", AGENT_A));
  persist_selected_agent(&path, "main agent").unwrap();
  let after = std::fs::read_to_string(&path).unwrap();
  assert!(after.starts_with("[general]\n\nselected_agent = main agent\n[agent]"));
  assert_eq!(load_general_settings(&path).unwrap().selected_agent, "main agent");
}

#[test]
fn daemon_section_validation() {
  let path = temp_settings(&format!("[daemon]\nllm_background_ptt_combo = ctrl+alt+\n\n{}", AGENT_A));
  let err = load_daemon_settings(&path).unwrap_err().to_string();
  assert!(err.contains("llm_background_ptt_combo"), "{}", err);

  let path = temp_settings(&format!(
    "[daemon]\nllm_background_ptt_combo = ctrl+alt+r\n\n{}",
    AGENT_A
  ));
  let err = load_daemon_settings(&path).unwrap_err().to_string();
  assert!(err.contains("same shortcut"), "{}", err);

  let path = temp_settings(&format!(
    "[daemon]\nllm_background_ptt_combo = \"Ctrl+Alt+Q\"\nllm_background_reset = shift+f5\n\n{}",
    AGENT_A
  ));
  let d = load_daemon_settings(&path).unwrap();
  assert_eq!(d.llm_background_ptt_combo, "ctrl+alt+q");
  assert_eq!(d.llm_background_reset, "shift+f5");
}

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
    daemon: false,
    daemon_foreground: false,
    daemon_stop: false,
    daemon_status: false,
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
    daemon: false,
    daemon_foreground: false,
    daemon_stop: false,
    daemon_status: false,
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
}

#[test]
fn unknown_section_is_reported_and_headers_are_case_insensitive() {
  // a typo'd header used to be swallowed into the first agent block and
  // surface as `missing field name`
  let path = temp_settings(
    "[Deamon]\nllm_background_reset = ctrl+alt+x\n\n[agent]\nname = a\nlanguage = en\ntts = supertonic\nvoice = M1\nprovider = ollama\nmodel = m\nbaseurl = http://127.0.0.1:11434\nsystem_prompt = x\n",
  );
  let err = load_settings(&path, &default_args()).unwrap_err().to_string();
  assert!(err.contains("unknown section [Deamon]"), "{}", err);

  let sections = split_leading_sections("[General]\nselected_agent = a\n[DAEMON]\n[Agent]\nname = a\n");
  assert!(sections.unknown.is_empty());
  assert!(sections.general.is_some() && sections.daemon.is_some());
  assert!(sections.rest.starts_with("[agent]\n"), "{:?}", sections.rest);
}
