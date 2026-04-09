// ------------------------------------------------------------------
//  debate
// ------------------------------------------------------------------

use crate::conversation::ChatMessage;
use crate::state::GLOBAL_STATE;
use crossbeam_channel::unbounded;
use crossbeam_channel::{Receiver, Sender, select};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::runtime::Builder as TokioBuilder;

async fn debate_get_response(
  messages: Vec<ChatMessage>,
  agent: &crate::config::AgentSettings,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
  let (_stop_tx, stop_rx) = unbounded::<()>();
  let interrupt_counter = Arc::new(AtomicU64::new(0));
  let mut result = String::new();
  let mut on_piece = |piece: &str| {
    result.push_str(piece);
  };
  crate::llm::llama_server_stream_response_into(
    &messages,
    &agent.baseurl,
    &agent.model,
    &agent.provider,
    &stop_rx,
    interrupt_counter.clone(),
    0,
    &mut on_piece,
  )
  .await?;
  Ok(result)
}

pub fn run_debate(
  subject: String,
  agents: Vec<crate::config::AgentSettings>,
  tx_tts: crossbeam_channel::Sender<(String, u64, String)>,
  tx_ui: crossbeam_channel::Sender<String>,
  interrupt_counter: std::sync::Arc<std::sync::atomic::AtomicU64>,
  pending_user: std::sync::Arc<std::sync::Mutex<Option<crate::conversation::ChatMessage>>>,
  tts_done_rx: crossbeam_channel::Receiver<()>,
) {
  if agents.len() < 2 {
    eprintln!("Not enough agents to debate. At least two required.");
    return;
  }
  let agent_count = agents.len();
  let rt = TokioBuilder::new_current_thread()
    .enable_all()
    .build()
    .unwrap();
  let mut turn = 0usize;
  let mut previous_reply = String::new();
  let mut history: Vec<crate::conversation::ChatMessage> = Vec::new();
  let mut last_interrupt = interrupt_counter.load(Ordering::SeqCst);
  let mut interrupted = false;
  loop {
    // If an interrupt has been requested, pause debate until user input
    if interrupt_counter.load(Ordering::SeqCst) != last_interrupt {
      interrupted = true;
      // Update the reference for next iteration
      last_interrupt = interrupt_counter.load(Ordering::SeqCst);
      // Skip processing until user provides input
      // We will loop again to check pending input
      continue;
    }
    // Check for pending user input
    let mut pending_msg_opt: Option<crate::conversation::ChatMessage> = None;
    {
      let mut lock = pending_user.lock().unwrap();
      if let Some(msg) = lock.take() {
        pending_msg_opt = Some(msg);
      }
    }
    if interrupted && pending_msg_opt.is_none() {
      // Still waiting for user input, skip AI turn
      continue;
    }
    if interrupted && pending_msg_opt.is_some() {
      // Resuming after user input
      interrupted = false;
    }
    // Duplicate pending_msg_opt block removed to avoid redundancy

    let current_agent = if let Some(_) = pending_msg_opt {
      &agents[0]
    } else {
      &agents[turn % agent_count]
    };
    let system_prompt = current_agent.system_prompt.replace("\\n", "\n");
    let user_msg = if let Some(msg) = pending_msg_opt {
      // Reset turn for new user query
      turn = 0;
      previous_reply.clear();
      msg.content
    } else if turn == 0 {
      format!("{}. Respond as short as possible", subject)
    } else {
      previous_reply.clone()
    };
    let mut messages = history.clone();
    messages.push(ChatMessage {
      role: "system".to_string(),
      content: system_prompt.clone(),
    });
    messages.push(ChatMessage {
      role: "user".to_string(),
      content: user_msg,
    });
    let reply = rt
      .block_on(debate_get_response(messages, current_agent))
      .unwrap_or_else(|e| {
        eprintln!("Error getting response: {}", e);
        std::process::exit(1);
      });
    // Append assistant reply to conversation history for subsequent turns
    history.push(ChatMessage {
      role: "assistant".to_string(),
      content: reply.clone(),
    });
    let _ = tx_ui.send("line| ".to_string());
    let label = format!("\x1b[48;5;22;37m{}:\x1b[0m", current_agent.name);
    let _ = tx_ui.send(format!("line|{}", label));
    let _ = tx_ui.send(format!("line|{}", reply.trim()));
    let current_interrupt = interrupt_counter.load(std::sync::atomic::Ordering::SeqCst);
    {
      let state = GLOBAL_STATE.get().expect("AppState not initialized");
      let original_voice = {
        let v = state.voice.lock().unwrap();
        v.clone()
      };
      let original_tts = {
        let v = state.tts.lock().unwrap();
        v.clone()
      };
      {
        let mut v = state.voice.lock().unwrap();
        *v = current_agent.voice.clone();
      }
      {
        let mut v = state.tts.lock().unwrap();
        *v = current_agent.tts.clone();
      }
      let phrases = split_into_phrases(&reply);
      for phrase in phrases {
        let cleaned = strip_special_chars(&phrase);
        let _ = tx_tts.send((cleaned, current_interrupt, current_agent.voice.clone()));
        let _ = tts_done_rx.recv();
      }
      {
        let mut v = state.voice.lock().unwrap();
        *v = original_voice;
      }
      {
        let mut v = state.tts.lock().unwrap();
        *v = original_tts;
      }
      // Wait for playback to finish before next AI request
      let playback_active = GLOBAL_STATE.get().unwrap().playback.playback_active.clone();
      while playback_active.load(Ordering::SeqCst) {
        std::thread::sleep(std::time::Duration::from_millis(10));
      }
    }
    previous_reply = reply.trim().to_string();
    turn += 1;
  }
}

/// Lightweight conversation thread for debate mode.
/// It transcribes STT input and forwards it to the debate thread
/// while rendering USER messages in the UI.
pub fn debate_conversation_thread(
  rx_utt: Receiver<crate::audio::AudioChunk>,
  stop_all_rx: Receiver<()>,
  stop_all_tx: Sender<()>,
  interrupt_counter: Arc<AtomicU64>,
  model_path: String,
  settings: crate::config::AgentSettings,
  ui: crate::state::UiState,
  _conversation_history: crate::conversation::ConversationHistory,
  tx_ui: Sender<String>,
  _tts_tx: Sender<(String, u64, String)>,
  tx_debate: Sender<ChatMessage>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
  // Initialise whisper context once
  let ctx = crate::conversation::init_whisper_context(&model_path);
  crate::log::log("info", &format!("LLM model: {}", settings.model));

  loop {
    select! {
      recv(stop_all_rx) -> _ => break,
      recv(rx_utt) -> msg => {
        let Ok(utt) = msg else { break };
        // Drain any pending stop signals
        while stop_all_rx.try_recv().is_ok() {}

        let state = GLOBAL_STATE.get().expect("AppState not initialized");
        state.playback.playback_active.store(true, Ordering::Relaxed);
        state.conversation_paused.store(false, Ordering::Relaxed);
        state.processing_response.store(true, Ordering::Relaxed);

        let pcm_f32: Vec<f32> = utt.data.clone();
        let mono_f32 = if utt.channels == 1 {
          pcm_f32.clone()
        } else {
          let ch = utt.channels as usize;
          let frames = pcm_f32.len() / ch;
          let mut mono = Vec::with_capacity(frames);
          for f in 0..frames {
            let start = f * ch;
            let sum: f32 = pcm_f32[start..start + ch].iter().sum();
            mono.push(sum / ch as f32);
          }
          mono
        };

        let state = GLOBAL_STATE.get().expect("AppState not initialized");
        let user_text = crate::stt::whisper_transcribe_with_ctx(
          &ctx,
          &mono_f32,
          utt.sample_rate,
          &state.language.lock().unwrap(),
        )?;
        if user_text.trim().is_empty() {
          continue;
        }

        // Render USER label and text
        let _ = tx_ui.send("line|\n".to_string());
        let _ = tx_ui.send(format!("line|{}", crate::ui::USER_LABEL));
        let _ = tx_ui.send(format!("line|{}", user_text.trim()));
        let _ = tx_ui.send("line|\n".to_string());

        // Push to debate thread
        let _ = tx_debate.send(ChatMessage{role:"user".to_string(), content:user_text.trim().to_string()});

        // In debate mode, we skip assistant response logic
        if crate::state::DEBATE_MODE.load(Ordering::SeqCst) {
          continue;
        }
      }
    }
  }
  Ok(())
}

// Utility to split a large reply into phrase chunks similar to conversation.rs
fn split_into_phrases(text: &str) -> Vec<String> {
  let mut phrases = Vec::new();
  let mut buf = String::new();
  for c in text.chars() {
    buf.push(c);
    if c == '\n' || c == '.' {
      let trimmed = buf.trim();
      if !trimmed.is_empty() {
        phrases.push(trimmed.to_string());
      }
      buf.clear();
    }
  }
  if !buf.trim().is_empty() {
    phrases.push(buf.trim().to_string());
  }
  phrases
}

// Utility to strip special chars as conversation.rs does before TTS
fn strip_special_chars(s: &str) -> String {
  let mut result = String::new();
  let parts: Vec<&str> = s.split("```").collect();
  let mut inside = false;
  for (i, part) in parts.iter().enumerate() {
    if !inside {
      result.extend(part.chars().filter(|c| {
        ![
          '+', '.', '~', '*', '&', '-', ',', ';', ':', '(', ')', '[', ']', '{', '}', '"', '\'',
          '#', '`', '|',
        ]
        .contains(c)
      }));
    }
    if i < parts.len() - 1 {
      inside = !inside;
    }
  }
  result
}
