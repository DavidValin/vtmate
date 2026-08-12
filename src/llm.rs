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

use bytes::Bytes;
use futures_util::StreamExt;
use llm::builder::{LLMBackend, LLMBuilder};
use llm::chat::{ChatMessage as LlmChatMessage, ChatProvider};
use llm::providers::openai_compatible::{OpenAICompatibleProvider, OpenAIProviderConfig};
use reqwest::StatusCode;
use serde_json::json;
use std::sync::{Arc, atomic::AtomicU64};
use std::time::Duration;

/// Providers served by a user supplied host through an OpenAI-compatible
/// endpoint - `openai-compatible-api` is the general case: any server or
/// hosted endpoint that speaks the OpenAI protocol, local (LM Studio,
/// vLLM, ...) or remote (Azure OpenAI, a proxy, LiteLLM, ...); it always
/// needs a baseurl, there being no sensible default for "somewhere else
/// that speaks this protocol". `openai-api` (in `CLOUD_PROVIDERS`) is the
/// unrelated, actual OpenAI hosted api and has no baseurl of its own.
pub const LOCAL_PROVIDERS: &[&str] = &["ollama", "llama-server", "openai-compatible-api"];

/// Each local provider's well-known default baseurl - `None` for
/// `openai-compatible-api`, which has no default (see above). Used by the
/// settings popup to fill in `baseurl` when the provider field changes.
pub fn default_baseurl_for(provider: &str) -> Option<&'static str> {
  match provider {
    "ollama" => Some("http://127.0.0.1:11434"),
    "llama-server" => Some("http://localhost:8080"),
    _ => None,
  }
}

/// Hosted providers handled by the `llm` crate backends (an api key is
/// needed). Named with an `-api` suffix so they read clearly next to their
/// cli-subscription counterpart in the settings picker (`anthropic-api` /
/// `claude-cli`). None of these take a custom baseurl - see
/// `openai-compatible-api` above for that.
pub const CLOUD_PROVIDERS: &[&str] = &[
  "openai-api",
  "anthropic-api",
  "google-api",
  "groq-api",
  "mistral-api",
  "openrouter-api",
  "deepseek-api",
  "xai-api",
];

/// `(canonical, legacy bare form, the `llm` crate's own backend name)` for
/// every hosted provider. `is_cloud_provider`, `api_key_env_var` and
/// `build_provider` all resolve through this, so an agents file written
/// before the `-api` rename (`provider = openai`) keeps working.
const CLOUD_PROVIDER_ALIASES: &[(&str, &str, &str)] = &[
  ("openai-api", "openai", "openai"),
  ("anthropic-api", "anthropic", "anthropic"),
  ("google-api", "google", "google"),
  ("groq-api", "groq", "groq"),
  ("mistral-api", "mistral", "mistral"),
  ("openrouter-api", "openrouter", "openrouter"),
  ("deepseek-api", "deepseek", "deepseek"),
  ("xai-api", "xai", "xai"),
];

/// The `llm` crate's own backend name for `provider` (a canonical `-api`
/// name or the legacy bare form), or `provider` itself if it matches
/// neither table - callers that only reach here for an already-confirmed
/// cloud provider never hit that fallback.
fn cloud_backend_name(provider: &str) -> &str {
  CLOUD_PROVIDER_ALIASES
    .iter()
    .find(|(canonical, legacy, _)| provider == *canonical || provider == *legacy)
    .map(|(_, _, backend)| *backend)
    .unwrap_or(provider)
}

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
    let provider = self.provider.trim().to_lowercase();
    if crate::llm_cli::is_cli_provider(&provider) {
      return crate::llm_cli::cli_hint(&provider);
    }
    match provider.as_str() {
      "ollama" => format!(
        "Make sure ollama (0.13 or newer) is running at {} and model '{}' is pulled",
        self.baseurl, self.model
      ),
      "llama-server" => format!(
        "Make sure llama-server / llamafile is running at {}",
        self.baseurl
      ),
      "openai-compatible-api" => format!("Make sure the server is running at {}", self.baseurl),
      _ => "Check the api_key, the model name and your network connection".to_string(),
    }
  }
}

pub fn is_local_provider(provider: &str) -> bool {
  LOCAL_PROVIDERS.contains(&provider.trim().to_lowercase().as_str())
}

pub fn is_cloud_provider(provider: &str) -> bool {
  let p = provider.trim().to_lowercase();
  CLOUD_PROVIDER_ALIASES
    .iter()
    .any(|(canonical, legacy, _)| p == *canonical || p == *legacy)
}

pub fn is_supported_provider(provider: &str) -> bool {
  is_local_provider(provider) || is_cloud_provider(provider) || crate::llm_cli::is_cli_provider(provider)
}

pub fn supported_providers_list() -> String {
  LOCAL_PROVIDERS
    .iter()
    .chain(CLOUD_PROVIDERS.iter())
    .chain(crate::llm_cli::CLI_PROVIDERS.iter())
    .map(|p| format!("'{}'", p))
    .collect::<Vec<_>>()
    .join(", ")
}

/// Environment variable consulted when `api_key` is empty in the settings
pub fn api_key_env_var(provider: &str) -> Option<&'static str> {
  let p = provider.trim().to_lowercase();
  match cloud_backend_name(&p) {
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

  if crate::llm_cli::is_cli_provider(&target.provider) {
    crate::log::log(
      "info",
      &format!(
        "Requesting provider '{}' model '{}' at {}",
        target.provider,
        target.model,
        describe_endpoint(target)
      ),
    );
    let result = crate::llm_cli::stream_cli_response_into(
      messages,
      target,
      interrupt_counter,
      expected_interrupt,
      on_piece,
    )
    .await;
    if let Ok(()) = &result {
      crate::log::log("info", &format!("Streaming response from: {}", describe_endpoint(target)));
    }
    return result;
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
  // Also the ceiling on how stale `interrupted()` can be during a gap
  // between chunks (a barge-in mid-"thinking" pause), so that gap stays
  // imperceptible.
  let poll_interval = Duration::from_millis(80);
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

/// Add a scheme to a bare `host[:port]` and drop a trailing slash, without
/// the `/v1` suffix `local_base_url` adds - used wherever something other
/// than the OpenAI-compatible chat path is being addressed (e.g. ollama's
/// native `/api/tags`).
pub(crate) fn base_url_with_scheme(baseurl: &str) -> String {
  let trimmed = baseurl.trim().trim_end_matches('/');
  if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
    trimmed.to_string()
  } else {
    format!("http://{}", trimmed)
  }
}

/// Turn the configured `host[:port]` into the `/v1` root the local server exposes
fn local_base_url(baseurl: &str) -> String {
  let with_scheme = base_url_with_scheme(baseurl);
  if with_scheme.ends_with("/v1") {
    with_scheme
  } else {
    format!("{}/v1", with_scheme)
  }
}

fn describe_endpoint(target: &LlmTarget) -> String {
  let provider = target.provider.trim().to_lowercase();
  if crate::llm_cli::is_cli_provider(&provider) {
    format!("the '{}' cli (model: {})", target.provider, target.model)
  } else if is_local_provider(&provider) {
    format!("{}/chat/completions", local_base_url(&target.baseurl))
  } else {
    format!("{} (default endpoint)", target.provider)
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

  let backend: LLMBackend = cloud_backend_name(&provider)
    .parse()
    .map_err(|e| format!("Unsupported provider '{}': {}", target.provider, e))?;
  let api_key = resolve_api_key(&provider, &target.api_key).ok_or_else(|| {
    format!(
      "No api_key configured for provider '{}' (set api_key in the agents file or the {} environment variable)",
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
  // no cloud provider takes a custom endpoint - that is what the local
  // openai-compatible-api provider is for; validate_baseurl already rejects
  // setting one here in the first place, baseurl is simply never read
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
    assert!(is_cloud_provider("anthropic-api"));
    // an agents file written before the `-api` rename still works
    assert!(is_cloud_provider("anthropic"));
    assert!(!is_supported_provider("foo"));
    assert_eq!(api_key_env_var("openai-api"), Some("OPENAI_API_KEY"));
    assert_eq!(api_key_env_var("openai"), Some("OPENAI_API_KEY"));
    assert_eq!(api_key_env_var("ollama"), None);
    // openai-api (the actual hosted OpenAI api) and openai-compatible-api
    // (a self-hosted or otherwise custom server speaking that protocol)
    // must never collide, and only the local one takes a baseurl
    assert!(is_cloud_provider("openai-api"));
    assert!(!is_local_provider("openai-api"));
    assert!(is_local_provider("openai-compatible-api"));
    assert!(!is_cloud_provider("openai-compatible-api"));
    assert_eq!(api_key_env_var("openai-compatible-api"), None);
  }

  #[test]
  fn cloud_provider_without_key_is_an_error() {
    for provider in ["anthropic-api", "anthropic"] {
      let target = LlmTarget {
        provider: provider.into(),
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

/// Stream response from Llama/Ollama endpoints, fallback if one fails, and mid-stream cancellation support.
/// When `include_tools` is true, the LLM may return tool_calls which are delivered via `on_tool_call`.
/// Reasoning tokens (from models like Gemma 4) are delivered via `on_reasoning`.
pub async fn llama_server_stream_response_into(
  messages: &Vec<crate::conversation::ChatMessage>,
  llama_host: &str,
  llama_model: &str,
  server_type: &str,
  interrupt_counter: Arc<AtomicU64>,
  expected_interrupt: u64,
  on_piece: &mut dyn FnMut(&str),
  include_tools: bool,
  tools: &[String],
  mut on_tool_call: Option<&mut dyn FnMut(&serde_json::Value)>,
  mut on_reasoning: Option<&mut dyn FnMut(&str)>,
  think: bool,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
  crate::log::log(
    "debug",
    &format!(
      "llama_server_stream_response_into called with include_tools: {} tools: {:?}",
      include_tools, tools
    ),
  );
  #[derive(Clone, Copy, Debug)]
  enum ApiKind {
    OaiChat,
    OllamaChat,
  }

  /// Accumulates one streamed tool call across SSE chunks, keyed by `index`.
  #[derive(Default, Clone)]
  struct ToolCallBuilder {
    id: String,
    name: String,
    arguments: String,
  }

  /// Turn accumulated builders into complete tool_call JSON values, draining the buffer.
  fn finalize_tool_calls(builders: &mut Vec<Option<ToolCallBuilder>>) -> Vec<serde_json::Value> {
    builders
      .drain(..)
      .flatten()
      .filter(|b| !b.name.is_empty())
      .map(|b| {
        json!({
          "id": b.id,
          "type": "function",
          "function": {
            "name": b.name,
            "arguments": b.arguments
          }
        })
      })
      .collect()
  }

  /// Finalize and dispatch any pending tool calls, if there are any and a callback is set.
  fn flush_tool_calls(
    builders: &mut Vec<Option<ToolCallBuilder>>,
    on_tool_call: &mut Option<&mut dyn FnMut(&serde_json::Value)>,
  ) {
    if builders.is_empty() {
      return;
    }
    let calls = finalize_tool_calls(builders);
    if let Some(cb) = on_tool_call.as_mut() {
      for tc in &calls {
        let name = tc
          .get("function")
          .and_then(|f| f.get("name"))
          .and_then(|n| n.as_str())
          .unwrap_or("");
        let args = tc
          .get("function")
          .and_then(|f| f.get("arguments"))
          .and_then(|a| a.as_str())
          .unwrap_or("");
        crate::log::send_line(&format!("\n\x1b[32m {} called with {}", name, args));
        cb(tc);
      }
    }
  }

  fn should_fallback_status(code: StatusCode) -> bool {
    matches!(
      code,
      StatusCode::NOT_FOUND
        | StatusCode::METHOD_NOT_ALLOWED
        | StatusCode::UNPROCESSABLE_ENTITY
        | StatusCode::BAD_REQUEST
        | StatusCode::UNSUPPORTED_MEDIA_TYPE
    )
  }

  fn candidates(host: &str, server_type: &str) -> Vec<(String, ApiKind)> {
    let base = host
      .trim_start_matches("http://")
      .trim_start_matches("https://")
      .trim_end_matches('/');
    let mut out = Vec::new();
    match server_type {
      "llama-server" => {
        out.push((
          format!("http://{}/v1/chat/completions", base),
          ApiKind::OaiChat,
        ));
        out.push((format!("http://{}/api/chat", base), ApiKind::OaiChat));
      }
      "ollama" => {
        out.push((
          format!("http://{}/v1/chat/completions", base),
          ApiKind::OaiChat,
        ));
        out.push((format!("http://{}/api/chat", base), ApiKind::OllamaChat));
      }
      _ => {}
    }
    out
  }

  let client = reqwest::Client::new();
  let tries = candidates(llama_host, server_type);
  let mut last_err: Option<String> = None;

  for (url, kind) in tries {
    if interrupt_counter.load(std::sync::atomic::Ordering::SeqCst) != expected_interrupt {
      return Ok(());
    }

    crate::log::log("info", &format!("Trying endpoint: {}", url));

    let req = match kind {
      ApiKind::OaiChat => {
        let tools_payload = if include_tools {
          Some(crate::tools::tools_schemas(tools).unwrap_or_default())
        } else {
          None
        };
        let payload = json!({
          "model": llama_model,
          "messages": messages.iter().map(|m| json!({ "role": m.role, "content": m.content })).collect::<Vec<_>>(),
          "think": think,
          "stream": true,
          "tools": tools_payload,
          "tool_choice": if include_tools { Some("auto") } else { None::<&str> },
          "parallel_tool_calls": if include_tools { Some(false) } else { None::<bool> },
          "options": {
            "think": think
          },
        });
        crate::log::log(
          "debug",
          &format!(
            "OAI payload tools: {:?}",
            tools_payload.as_ref().map(|v| v
              .iter()
              .filter_map(|t| t.get("name").and_then(|n| n.as_str()))
              .collect::<Vec<_>>())
          ),
        );
        client.post(&url).json(&payload)
      }
      ApiKind::OllamaChat => {
        let tools_payload = if include_tools {
          Some(crate::tools::tools_schemas(tools).unwrap_or_default())
        } else {
          None
        };
        crate::log::log(
          "debug",
          &format!(
            "Ollama payload tools: {:?}",
            tools_payload.as_ref().map(|v| v
              .iter()
              .filter_map(|t| t.get("name").and_then(|n| n.as_str()))
              .collect::<Vec<_>>())
          ),
        );
        let payload = json!({
          "model": llama_model,
          "messages": messages.iter().map(|m| json!({ "role": m.role, "content": m.content })).collect::<Vec<_>>(),
          "think": think,
          "stream": true,
          "tools": tools_payload,
          "tool_choice": if include_tools { Some("auto") } else { None::<&str> },
          "parallel_tool_calls": if include_tools { Some(false) } else { None::<bool> },
          "options": {
            "think": think
          },
        });
        client.post(&url).json(&payload)
      }
    };

    let resp = match tokio::time::timeout(std::time::Duration::from_secs(120), req.send()).await {
      Ok(Ok(r)) => r,
      Ok(Err(e)) => {
        last_err = Some(format!("Request to {} failed: {}", url, e));
        log::warn!("{}", last_err.as_ref().unwrap());
        continue;
      }
      Err(_) => {
        last_err = Some(format!("Request to {} timed out", url));
        log::warn!("{}", last_err.as_ref().unwrap());
        continue;
      }
    };

    if !resp.status().is_success() {
      let status = resp.status();
      last_err = Some(format!("Endpoint {} returned HTTP {}", url, status));
      log::warn!("{}", last_err.as_ref().unwrap());
      if should_fallback_status(status) {
        continue;
      } else {
        return Err(last_err.clone().unwrap().into());
      }
    }

    crate::log::log("info", &format!("Streaming response from: {}", url));
    // inside your endpoint loop
    let mut stream = resp.bytes_stream();
    // Accumulates fragmented tool_calls (see ToolCallBuilder) for this endpoint attempt.
    let mut tool_call_builders: Vec<Option<ToolCallBuilder>> = Vec::new();

    // Bound how long we wait for the next chunk: without this, a server that accepts
    // the connection but stops sending bytes mid-stream (no close, no data) hangs here
    // forever and is deaf to Esc/Undo, since those are only checked between chunks.
    let stall_timeout = std::time::Duration::from_secs(120);
    let poll_interval = std::time::Duration::from_millis(250);
    let mut since_last_chunk = std::time::Duration::ZERO;

    loop {
      // check stop signal, polled regardless of whether data is arriving
      if interrupt_counter.load(std::sync::atomic::Ordering::SeqCst) != expected_interrupt {
        return Ok(());
      }

      let chunk_result = match tokio::time::timeout(poll_interval, stream.next()).await {
        Ok(Some(r)) => {
          since_last_chunk = std::time::Duration::ZERO;
          r
        }
        Ok(None) => break, // stream ended normally
        Err(_) => {
          since_last_chunk += poll_interval;
          if since_last_chunk >= stall_timeout {
            crate::log::log(
              "error",
              &format!(
                "Streaming stalled (no data for {}s) at {}",
                stall_timeout.as_secs(),
                url
              ),
            );
            break; // fallback to next endpoint
          }
          continue;
        }
      };

      let chunk: Bytes = match chunk_result {
        Ok(b) => b,
        Err(e) => {
          crate::log::log("error", &format!("Streaming error at {}: {}", url, e));
          break; // fallback to next endpoint
        }
      };

      if let Ok(text) = std::str::from_utf8(&chunk) {
        // crate::log::log("debug", &format!("chunk: {}", text));
        for line in text.lines() {
          let payload = line.trim().strip_prefix("data:").unwrap_or(line).trim();
          if payload == "[DONE]" {
            flush_tool_calls(&mut tool_call_builders, &mut on_tool_call);
            return Ok(());
          }

          // parse JSON safely
          if let Ok(v) = serde_json::from_str::<serde_json::Value>(payload) {
            // Handle new Llama3.2 style: {"message":{"content":...}}
            if let Some(message) = v.get("message") {
              if let Some(content) = message.get("content").and_then(|c| c.as_str()) {
                if !content.is_empty() {
                  on_piece(content);
                }
              }
              // Check for tool_calls in message (non-streaming response)
              if let Some(tcs) = message.get("tool_calls").and_then(|t| t.as_array()) {
                if !tcs.is_empty() {
                  if let Some(ref mut cb) = on_tool_call {
                    for tc in tcs {
                      // Extract name and arguments from inside "function" wrapper
                      if let Some(func) = tc.get("function") {
                        if let Some(name) = func.get("name").and_then(|v| v.as_str()) {
                          let args = func.get("arguments").and_then(|v| v.as_str()).unwrap_or("");
                          let msg = format!("\n\x1b[32m {} called with {}", name, args);
                          crate::log::send_line(&msg);
                        }
                      }
                      cb(tc);
                    }
                  }
                }
              }
              // End-of-stream signal from Ollama chat API
              if v.get("done").and_then(|x| x.as_bool()) == Some(true) {
                flush_tool_calls(&mut tool_call_builders, &mut on_tool_call);
                return Ok(());
              }
            } else {
              match kind {
                ApiKind::OaiChat | ApiKind::OllamaChat => {
                  if let Some(choices) = v.get("choices").and_then(|c| c.as_array()) {
                    for choice in choices {
                      if let Some(delta) = choice.get("delta") {
                        if let Some(content) = delta.get("content").and_then(|c| c.as_str()) {
                          if !content.is_empty() {
                            on_piece(content);
                          }
                        }
                        // Extract reasoning tokens (Gemma 4, DeepSeek, etc.)
                        if let Some(reasoning) = delta.get("reasoning").and_then(|r| r.as_str()) {
                          if !reasoning.is_empty() {
                            if let Some(ref mut cb) = on_reasoning {
                              cb(reasoning);
                            }
                          }
                        }
                        // Accumulate by index — arguments arrive as partial JSON text.
                        if let Some(tcs) = delta.get("tool_calls").and_then(|t| t.as_array()) {
                          for tc in tcs {
                            let idx = tc.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as usize;
                            if tool_call_builders.len() <= idx {
                              tool_call_builders.resize_with(idx + 1, || None);
                            }
                            let builder =
                              tool_call_builders[idx].get_or_insert_with(ToolCallBuilder::default);
                            if let Some(id) = tc.get("id").and_then(|v| v.as_str()) {
                              if !id.is_empty() {
                                builder.id = id.to_string();
                              }
                            }
                            if let Some(func) = tc.get("function") {
                              if let Some(name) = func.get("name").and_then(|v| v.as_str()) {
                                if !name.is_empty() {
                                  builder.name = name.to_string();
                                }
                              }
                              if let Some(args) = func.get("arguments").and_then(|v| v.as_str()) {
                                builder.arguments.push_str(args);
                              }
                            }
                          }
                        }
                      }
                      let finish_reason = choice.get("finish_reason").and_then(|r| r.as_str());
                      if finish_reason == Some("stop") || finish_reason == Some("tool_calls") {
                        flush_tool_calls(&mut tool_call_builders, &mut on_tool_call);
                        return Ok(());
                      }
                    }
                  }
                  if v.get("done").and_then(|x| x.as_bool()) == Some(true)
                    || v.get("status").and_then(|x| x.as_str()) == Some("completed")
                  {
                    flush_tool_calls(&mut tool_call_builders, &mut on_tool_call);
                    return Ok(());
                  }
                }
              }
            }
          }
        }
      }
    }

    // Stream ended (closed or stalled) without an explicit finish signal —
    // still flush any tool call fragments accumulated so far.
    flush_tool_calls(&mut tool_call_builders, &mut on_tool_call);
    return Ok(());
  }

  // all endpoints failed
  Err(
    last_err
      .unwrap_or_else(|| "No endpoint candidates succeeded".to_string())
      .into(),
  )
}
