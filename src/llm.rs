// ------------------------------------------------------------------
//  LLM handling
//
//  All chat traffic goes through the `llm` crate so several backends can be
//  used with the same streaming code path:
//
//    * local servers (ollama, llama-server, any openai-compatible server) are
//      reached through their OpenAI-compatible `/v1/chat/completions` endpoint
//      with `reasoning_effort: "none"`, which is the portable way to disable
//      "thinking" on both ollama (>= 0.13) and llama.cpp.
//    * hosted providers (openai, anthropic, google, ...) use the crate's own
//      backends and need an api key.
// ------------------------------------------------------------------

use futures_util::StreamExt;
use llm::builder::{LLMBackend, LLMBuilder};
use llm::chat::{ChatMessage as LlmChatMessage, ChatProvider};
use llm::providers::openai_compatible::{OpenAICompatibleProvider, OpenAIProviderConfig};
use std::sync::{Arc, atomic::AtomicU64};
use std::time::Duration;

/// Providers served by a user supplied host through an OpenAI-compatible endpoint.
pub const LOCAL_PROVIDERS: &[&str] = &["ollama", "llama-server", "openai-compatible"];

/// Hosted providers handled by the `llm` crate backends (an api key is needed).
pub const CLOUD_PROVIDERS: &[&str] = &[
  "openai",
  "anthropic",
  "google",
  "groq",
  "mistral",
  "openrouter",
  "deepseek",
  "xai",
];

/// Where a request goes: provider name plus the connection details of one agent.
#[derive(Clone, Debug)]
pub struct LlmTarget {
  pub provider: String,
  pub baseurl: String,
  pub model: String,
  pub api_key: String,
}

impl LlmTarget {
  pub fn from_settings(settings: &crate::config::AgentSettings) -> Self {
    Self {
      provider: settings.provider.clone(),
      baseurl: settings.baseurl.clone(),
      model: settings.model.clone(),
      api_key: settings.api_key.clone(),
    }
  }

  pub fn from_state(state: &crate::state::AppState) -> Self {
    Self {
      provider: state.provider.lock().unwrap().clone(),
      baseurl: state.baseurl.lock().unwrap().clone(),
      model: state.model.lock().unwrap().clone(),
      api_key: state.api_key.lock().unwrap().clone(),
    }
  }

  /// Human hint appended to error logs
  pub fn hint(&self) -> String {
    match self.provider.trim().to_lowercase().as_str() {
      "ollama" => format!(
        "Make sure ollama (0.13 or newer) is running at {} and model '{}' is pulled",
        self.baseurl, self.model
      ),
      "llama-server" => format!(
        "Make sure llama-server / llamafile is running at {}",
        self.baseurl
      ),
      "openai-compatible" => format!("Make sure the server is running at {}", self.baseurl),
      _ => "Check the api_key, the model name and your network connection".to_string(),
    }
  }
}

pub fn is_local_provider(provider: &str) -> bool {
  LOCAL_PROVIDERS.contains(&provider.trim().to_lowercase().as_str())
}

pub fn is_cloud_provider(provider: &str) -> bool {
  CLOUD_PROVIDERS.contains(&provider.trim().to_lowercase().as_str())
}

pub fn is_supported_provider(provider: &str) -> bool {
  is_local_provider(provider) || is_cloud_provider(provider)
}

pub fn supported_providers_list() -> String {
  LOCAL_PROVIDERS
    .iter()
    .chain(CLOUD_PROVIDERS.iter())
    .map(|p| format!("'{}'", p))
    .collect::<Vec<_>>()
    .join(", ")
}

/// Environment variable consulted when `api_key` is empty in the settings
pub fn api_key_env_var(provider: &str) -> Option<&'static str> {
  match provider.trim().to_lowercase().as_str() {
    "openai" => Some("OPENAI_API_KEY"),
    "anthropic" => Some("ANTHROPIC_API_KEY"),
    "google" => Some("GOOGLE_API_KEY"),
    "groq" => Some("GROQ_API_KEY"),
    "mistral" => Some("MISTRAL_API_KEY"),
    "openrouter" => Some("OPENROUTER_API_KEY"),
    "deepseek" => Some("DEEPSEEK_API_KEY"),
    "xai" => Some("XAI_API_KEY"),
    _ => None,
  }
}

/// The configured api key, falling back to the provider's environment variable
pub fn resolve_api_key(provider: &str, configured: &str) -> Option<String> {
  let configured = configured.trim();
  if !configured.is_empty() {
    return Some(configured.to_string());
  }
  api_key_env_var(provider)
    .and_then(|var| std::env::var(var).ok())
    .map(|k| k.trim().to_string())
    .filter(|k| !k.is_empty())
}

/// Stream a chat completion for `messages` into `on_piece`, with mid-stream
/// cancellation when `interrupt_counter` moves away from `expected_interrupt`.
pub async fn stream_response_into(
  messages: &[crate::conversation::ChatMessage],
  target: &LlmTarget,
  interrupt_counter: Arc<AtomicU64>,
  expected_interrupt: u64,
  on_piece: &mut dyn FnMut(&str),
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
  let interrupted =
    || interrupt_counter.load(std::sync::atomic::Ordering::SeqCst) != expected_interrupt;

  if interrupted() {
    return Ok(());
  }

  let (system_prompt, chat) = split_messages(messages);
  let provider = build_provider(target, system_prompt)?;

  crate::log::log(
    "info",
    &format!(
      "Requesting provider '{}' model '{}' at {}",
      target.provider,
      target.model,
      describe_endpoint(target)
    ),
  );

  let mut stream =
    match tokio::time::timeout(Duration::from_secs(120), provider.chat_stream(&chat)).await {
      Ok(Ok(s)) => s,
      Ok(Err(e)) => {
        return Err(format!("Request to {} failed: {}", describe_endpoint(target), e).into());
      }
      Err(_) => {
        return Err(format!("Request to {} timed out", describe_endpoint(target)).into());
      }
    };

  crate::log::log(
    "info",
    &format!("Streaming response from: {}", describe_endpoint(target)),
  );

  // Bound how long we wait for the next chunk: without this, a server that accepts
  // the connection but stops sending bytes mid-stream (no close, no data) hangs here
  // forever and is deaf to Esc/Undo, since those are only checked between chunks.
  let stall_timeout = Duration::from_secs(120);
  let poll_interval = Duration::from_millis(250);
  let mut since_last_chunk = Duration::ZERO;

  loop {
    // check stop signal, polled regardless of whether data is arriving.
    // Returning drops the stream, which aborts the underlying request.
    if interrupted() {
      return Ok(());
    }

    match tokio::time::timeout(poll_interval, stream.next()).await {
      Ok(Some(Ok(piece))) => {
        since_last_chunk = Duration::ZERO;
        if !piece.is_empty() {
          on_piece(&piece);
        }
      }
      Ok(Some(Err(e))) => {
        return Err(format!("Streaming error at {}: {}", describe_endpoint(target), e).into());
      }
      Ok(None) => return Ok(()), // stream ended normally
      Err(_) => {
        since_last_chunk += poll_interval;
        if since_last_chunk >= stall_timeout {
          return Err(
            format!(
              "Streaming stalled (no data for {}s) at {}",
              stall_timeout.as_secs(),
              describe_endpoint(target)
            )
            .into(),
          );
        }
      }
    }
  }
}

// PRIVATE
// ------------------------------------------------------------------

/// OpenAI-compatible config used for every local server (ollama, llama-server, ...)
struct LocalOpenAiCompatible;

impl OpenAIProviderConfig for LocalOpenAiCompatible {
  const PROVIDER_NAME: &'static str = "openai-compatible";
  const DEFAULT_BASE_URL: &'static str = "http://127.0.0.1:11434/v1/";
  const DEFAULT_MODEL: &'static str = "llama3.2:3b";
  // needed so `reasoning_effort` is actually sent in the request body
  const SUPPORTS_REASONING_EFFORT: bool = true;
}

/// Turn the configured `host[:port]` into the `/v1` root the local server exposes
fn local_base_url(baseurl: &str) -> String {
  let trimmed = baseurl.trim().trim_end_matches('/');
  let with_scheme = if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
    trimmed.to_string()
  } else {
    format!("http://{}", trimmed)
  };
  if with_scheme.ends_with("/v1") {
    with_scheme
  } else {
    format!("{}/v1", with_scheme)
  }
}

fn describe_endpoint(target: &LlmTarget) -> String {
  if is_local_provider(&target.provider) {
    format!("{}/chat/completions", local_base_url(&target.baseurl))
  } else if target.baseurl.trim().is_empty() {
    format!("{} (default endpoint)", target.provider)
  } else {
    target.baseurl.trim().to_string()
  }
}

/// vtmate keeps the system prompt as the first message; the `llm` crate takes it
/// as a provider option instead, so split it out and map the rest.
fn split_messages(
  messages: &[crate::conversation::ChatMessage],
) -> (Option<String>, Vec<LlmChatMessage>) {
  let mut system_parts: Vec<&str> = Vec::new();
  let mut chat = Vec::new();
  for m in messages {
    match m.role.as_str() {
      "system" => system_parts.push(m.content.as_str()),
      "assistant" => {
        // some backends reject empty assistant messages (e.g. an interrupted turn)
        if !m.content.trim().is_empty() {
          chat.push(
            LlmChatMessage::assistant()
              .content(m.content.clone())
              .build(),
          );
        }
      }
      _ => chat.push(LlmChatMessage::user().content(m.content.clone()).build()),
    }
  }
  let system_prompt = if system_parts.is_empty() {
    None
  } else {
    Some(system_parts.join("\n"))
  };
  (system_prompt, chat)
}

fn build_provider(
  target: &LlmTarget,
  system_prompt: Option<String>,
) -> Result<Box<dyn ChatProvider>, String> {
  let provider = target.provider.trim().to_lowercase();

  if is_local_provider(&provider) {
    // No overall request timeout: replies can legitimately take minutes while
    // streaming; stalls are detected chunk by chunk in `stream_response_into`.
    let client = reqwest::Client::builder()
      .connect_timeout(Duration::from_secs(15))
      .build()
      .map_err(|e| format!("Failed to build HTTP client: {}", e))?;
    let api_key =
      resolve_api_key(&provider, &target.api_key).unwrap_or_else(|| "no-key".to_string());
    let p = OpenAICompatibleProvider::<LocalOpenAiCompatible>::with_client(
      client,
      api_key,
      Some(local_base_url(&target.baseurl)),
      Some(target.model.clone()),
      None, // max_tokens
      None, // temperature
      None, // timeout_seconds
      system_prompt,
      None, // top_p
      None, // top_k
      None, // tools
      None, // tool_choice
      // `reasoning_effort: "none"` disables thinking on ollama (>= 0.13) and
      // llama.cpp; the old `think: false` field is ignored by their /v1 endpoints.
      Some("none".to_string()),
      None, // json_schema
      None, // voice
      None, // extra_body
      None, // parallel_tool_calls
      None, // normalize_response
      None, // embedding_encoding_format
      None, // embedding_dimensions
    );
    return Ok(Box::new(p));
  }

  if !is_cloud_provider(&provider) {
    return Err(format!(
      "Unsupported provider '{}'. Supported providers: {}",
      target.provider,
      supported_providers_list()
    ));
  }

  let backend: LLMBackend = provider
    .parse()
    .map_err(|e| format!("Unsupported provider '{}': {}", target.provider, e))?;
  let api_key = resolve_api_key(&provider, &target.api_key).ok_or_else(|| {
    format!(
      "No api_key configured for provider '{}' (set api_key in the settings file or the {} environment variable)",
      provider,
      api_key_env_var(&provider).unwrap_or("provider")
    )
  })?;

  let mut builder = LLMBuilder::new()
    .backend(backend)
    .api_key(api_key)
    .model(target.model.clone())
    .timeout_seconds(600);
  if let Some(system) = system_prompt {
    builder = builder.system(system);
  }
  if !target.baseurl.trim().is_empty() {
    builder = builder.base_url(target.baseurl.trim().to_string());
  }
  let p = builder
    .build()
    .map_err(|e| format!("Failed to initialise provider '{}': {}", provider, e))?;
  Ok(p)
}

// TESTS
// ------------------------------------------------------------------

#[cfg(test)]
mod tests {
  use super::*;
  use crate::conversation::ChatMessage;
  use std::sync::atomic::Ordering;

  fn msg(role: &str, content: &str) -> ChatMessage {
    ChatMessage {
      role: role.to_string(),
      content: content.to_string(),
      agent_name: None,
    }
  }

  #[test]
  fn local_base_url_normalises_host_forms() {
    assert_eq!(
      local_base_url("http://127.0.0.1:11434"),
      "http://127.0.0.1:11434/v1"
    );
    assert_eq!(
      local_base_url("http://127.0.0.1:11434/"),
      "http://127.0.0.1:11434/v1"
    );
    assert_eq!(local_base_url("127.0.0.1:8080"), "http://127.0.0.1:8080/v1");
    assert_eq!(
      local_base_url("https://box:8080/v1/"),
      "https://box:8080/v1"
    );
  }

  #[test]
  fn split_messages_moves_system_prompt_out() {
    let (system, chat) = split_messages(&[
      msg("system", "be terse"),
      msg("user", "hi"),
      msg("assistant", ""),
      msg("assistant", "hello"),
    ]);
    assert_eq!(system.as_deref(), Some("be terse"));
    // empty assistant placeholder is dropped
    assert_eq!(chat.len(), 2);
  }

  #[test]
  fn provider_classification() {
    assert!(is_local_provider("ollama"));
    assert!(is_local_provider("Llama-Server"));
    assert!(is_cloud_provider("anthropic"));
    assert!(!is_supported_provider("foo"));
    assert_eq!(api_key_env_var("openai"), Some("OPENAI_API_KEY"));
    assert_eq!(api_key_env_var("ollama"), None);
  }

  #[test]
  fn cloud_provider_without_key_is_an_error() {
    let target = LlmTarget {
      provider: "anthropic".into(),
      baseurl: String::new(),
      model: "claude-sonnet-5".into(),
      api_key: String::new(),
    };
    // make sure the env fallback does not kick in
    unsafe { std::env::remove_var("ANTHROPIC_API_KEY") };
    let err = build_provider(&target, None)
      .err()
      .expect("must fail without key");
    assert!(err.contains("ANTHROPIC_API_KEY"), "{err}");
  }

  fn test_target() -> LlmTarget {
    LlmTarget {
      provider: std::env::var("VTMATE_TEST_LLM_PROVIDER").unwrap_or_else(|_| "ollama".into()),
      baseurl: std::env::var("VTMATE_TEST_LLM_HOST")
        .unwrap_or_else(|_| "http://127.0.0.1:11434".into()),
      model: std::env::var("VTMATE_TEST_LLM_MODEL").unwrap_or_else(|_| "llama3.2:3b".into()),
      api_key: String::new(),
    }
  }

  fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
      .enable_all()
      .build()
      .unwrap()
  }

  /// Needs a running server: VTMATE_TEST_LLM_HOST / VTMATE_TEST_LLM_MODEL
  #[test]
  #[ignore = "needs a running llm server"]
  fn streams_a_reply_from_local_server() {
    let target = test_target();
    let messages = vec![
      msg(
        "system",
        "You are a terse assistant. Answer with a single word.",
      ),
      msg("user", "What is the capital of France?"),
    ];
    let started = std::time::Instant::now();
    let mut first_piece_at = None;
    let mut out = String::new();
    let mut on_piece = |p: &str| {
      if first_piece_at.is_none() {
        first_piece_at = Some(started.elapsed());
      }
      out.push_str(p);
    };
    rt()
      .block_on(stream_response_into(
        &messages,
        &target,
        Arc::new(AtomicU64::new(0)),
        0,
        &mut on_piece,
      ))
      .expect("stream must succeed");
    crate::log::log(
      "debug",
      &format!(
        "[{}] first piece after {:?}, total {:?}, reply={:?}",
        target.model,
        first_piece_at,
        started.elapsed(),
        out
      ),
    );
    assert!(
      out.to_lowercase().contains("paris"),
      "unexpected reply: {out}"
    );
    // thinking must be disabled: with reasoning on, a thinking model spends
    // seconds before the first visible token and may leak <think> tags
    assert!(
      !out.contains("<think>"),
      "thinking leaked into content: {out}"
    );
    assert!(
      first_piece_at.unwrap() < Duration::from_secs(8),
      "first piece took {:?}, thinking probably not disabled",
      first_piece_at.unwrap()
    );
  }

  #[test]
  #[ignore = "needs a running llm server"]
  fn interrupt_stops_the_stream_early() {
    let target = test_target();
    let messages = vec![
      msg("system", "You are a storyteller."),
      msg("user", "Tell a 300 word story about a lighthouse."),
    ];
    let counter = Arc::new(AtomicU64::new(0));
    let counter_for_closure = counter.clone();
    let mut pieces = 0usize;
    let mut on_piece = |_: &str| {
      pieces += 1;
      if pieces == 5 {
        // simulate the user interrupting (Esc / speaking)
        counter_for_closure.fetch_add(1, Ordering::SeqCst);
      }
    };
    let started = std::time::Instant::now();
    rt()
      .block_on(stream_response_into(
        &messages,
        &target,
        counter.clone(),
        0,
        &mut on_piece,
      ))
      .expect("interrupted stream returns Ok");
    crate::log::log(
      "debug",
      &format!(
        "[{}] interrupted after {} pieces in {:?}",
        target.model,
        pieces,
        started.elapsed()
      ),
    );
    assert!(
      pieces >= 5 && pieces < 20,
      "stream did not stop promptly: {pieces} pieces"
    );
  }
}
