// ------------------------------------------------------------------
//  Conversation
// ------------------------------------------------------------------

use crate::START_INSTANT;
use crate::state::GLOBAL_STATE;
use crossbeam_channel::{Receiver, Sender, select};
use std::sync::OnceLock;
use std::sync::{
  Arc,
  atomic::{AtomicU64, Ordering},
};
use tokio::runtime::Builder as TokioBuilder;

static WHISPER_CTX: OnceLock<whisper_rs::WhisperContext> = OnceLock::new();

// API
// ------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChatMessage {
  pub role: String,
  pub content: String,
}

pub type ConversationHistory = std::sync::Arc<std::sync::Mutex<Vec<ChatMessage>>>;

/// Initialise the Whisper context once, performing a warm‑up.
pub fn init_whisper_context(model_path: &str) -> &'static whisper_rs::WhisperContext {
  WHISPER_CTX.get_or_init(|| {
    let ctx = whisper_rs::WhisperContext::new_with_params(model_path, Default::default())
      .expect("Failed to create WhisperContext");
    // Perform warm‑up to load the model into memory
    crate::stt::whisper_warmup(model_path).expect("Whisper warm‑up failed");
    ctx
  })
}

pub fn conversation_thread(
  rx_utt: Receiver<crate::audio::AudioChunk>,
  stop_all_rx: Receiver<()>,
  stop_all_tx: Sender<()>,
  interrupt_counter: Arc<AtomicU64>,
  model_path: String,
  settings: crate::config::AgentSettings,
  ui: crate::state::UiState,
  conversation_history: ConversationHistory,
  tx_ui: Sender<String>,
  tts_tx: Sender<(String, u64, String)>,
  tts_done_rx: Receiver<()>,
  initial_prompt: Option<String>,
  quiet: bool,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
  let ctx = init_whisper_context(&model_path);
  crate::log::log("info", &format!("LLM model: {}", settings.model));
  
  //  –––––––––––––––––––––––––––––––––––––
  //   single run mode
  //  –––––––––––––––––––––––––––––––––––––

  if quiet {
    crate::log::log("info", "Running in quiet mode");
    
    let rt = TokioBuilder::new_current_thread()
      .enable_all()
      .build()
      .unwrap();
      
    if let Some(prompt) = initial_prompt {
      // Show user message in UI
      send_user_message_ui(&tx_ui, &prompt, false);
      push_user_message(&conversation_history, &prompt);
      
      let system_prompt = settings.system_prompt.replace("\\n", "\n");
      let messages = create_basic_messages(system_prompt, prompt);
      
      let my_interrupt = interrupt_counter.load(Ordering::SeqCst);
      let reply = rt
        .block_on(get_response(messages, &settings))
        .unwrap_or_else(|e| {
          crate::log::log("error", &format!("Error getting response in quiet mode: {}", e));
          String::new()
        });
        
      if !reply.is_empty() {
        conversation_history.lock().unwrap().push(ChatMessage {
          role: "assistant".to_string(),
          content: reply.clone(),
        });
        
        // Display in UI
        let _ = tx_ui.send("line| ".to_string());
        let label = format!("\x1b[48;5;22;37m{}:\x1b[0m", settings.name);
        let _ = tx_ui.send(format!("line|{}", label));
        let _ = tx_ui.send(format!("stream|{}", reply.trim()));
        let _ = tx_ui.send("line|".to_string());
        
        process_tts_phrases(
          &reply,
          &tts_tx,
          &tts_done_rx,
          settings.voice.clone(),
          &interrupt_counter,
          my_interrupt,
        );
        
        let state = GLOBAL_STATE.get().expect("AppState not initialized");
        wait_for_playback(state, &interrupt_counter, my_interrupt);
      }
    }
    
    crate::log::log("info", "Quiet mode playback finished. Exiting.");
    std::process::exit(0);
  }
  
  // Runtime to use for async debate responses
  let rt = TokioBuilder::new_current_thread()
    .enable_all()
    .build()
    .unwrap();

  // Track interruptions for debate mode
  let mut last_interrupt = interrupt_counter.load(Ordering::SeqCst);
  let mut debate_interrupted = false;
  let mut pending_user_msg: Option<String> = initial_prompt;

  //  –––––––––––––––––––––––––––––––––––––
  //   loop
  //  –––––––––––––––––––––––––––––––––––––
  loop {
    // Check if we should run debate turn
    let state = GLOBAL_STATE.get().expect("AppState not initialized");
    // Show initial prompt only if not in debate mode
    if !state.debate_enabled.load(Ordering::SeqCst) {
      if let Some(ref prompt) = pending_user_msg {
        send_user_message_ui(&tx_ui, prompt, false);
        push_user_message(&conversation_history, prompt);
        pending_user_msg = Some(prompt.clone());
      }
    }
    //  –––––––––––––––––––––––––––––––––––––
    //   debate mode
    //  –––––––––––––––––––––––––––––––––––––
    if state.debate_enabled.load(Ordering::SeqCst) {
      let debate_agents = state.debate_agents.lock().unwrap().clone();
      if debate_agents.len() >= 2 {
        // Check for stop signal (Ctrl+C)
        if stop_all_rx.try_recv().is_ok() {
          break;
        }

        // Check for interruption
        let current_interrupt = interrupt_counter.load(Ordering::SeqCst);
        if current_interrupt != last_interrupt {
          debate_interrupted = true;
          last_interrupt = current_interrupt;
          // Stop any ongoing playback
          state
            .playback
            .playback_active
            .store(false, Ordering::Relaxed);
          // Skip to waiting for user input
          crate::log::log("debug", "Debate interrupted, waiting for user input");
        }

        // Check for user input with short timeout
        let user_input_result = rx_utt.recv_timeout(std::time::Duration::from_millis(100));

        if let Ok(utt) = user_input_result {
          // User provided input - process it
          let state = GLOBAL_STATE.get().expect("AppState not initialized");
          state.conversation_paused.store(false, Ordering::Relaxed);
          state.processing_response.store(true, Ordering::Relaxed);

          let _pcm_f32: Vec<f32> = utt.data.clone();
          let mono_f32 = crate::audio::convert_to_mono(&utt);

          let user_text = crate::stt::whisper_transcribe_with_ctx(
            &ctx,
            &mono_f32,
            utt.sample_rate,
            &state.language.lock().unwrap(),
          )?;
          let user_text = user_text.trim().to_string();

          if !user_text.is_empty() {
            // Clear STOP_STREAM flag to ensure user text displays fully
            crate::ui::STOP_STREAM.store(false, Ordering::Relaxed);
            send_user_message_ui(&tx_ui, &user_text, true);
            push_user_message(&conversation_history, &user_text);

            // Store user message for next agent to respond to
            pending_user_msg = Some(user_text.clone());
            debate_interrupted = false;
            state
              .playback
              .playback_active
              .store(false, Ordering::Relaxed);
          }
          continue;
        }

        // If interrupted but no user input yet, skip AI turn
        if debate_interrupted && pending_user_msg.is_none() {
          std::thread::sleep(std::time::Duration::from_millis(50));
          continue;
        }

        // No user input - run debate turn
        let turn = state.debate_turn.load(Ordering::SeqCst) as usize;
        let agent_count = debate_agents.len();
        // If first debate turn, display subject as user input before agent reply
        if turn == 0 {
          let subject = state.debate_subject.lock().unwrap().clone();
          if !subject.is_empty() {
            send_user_message_ui(&tx_ui, &subject, false);
          }
        }

        // Determine current agent and message
        let (current_agent, user_msg) = if let Some(msg) = pending_user_msg.take() {
          // User interrupted - current agent responds to user
          (&debate_agents[turn % agent_count], msg)
        } else {
          let current_agent = &debate_agents[turn % agent_count];
          let subject = state.debate_subject.lock().unwrap().clone();
          let user_msg = if turn == 0 && !subject.is_empty() {
            format!("{}. Respond as short as possible", subject)
          } else {
            // Get last assistant message as the prompt for next agent
            let hist = conversation_history.lock().unwrap();
            hist
              .iter()
              .rev()
              .find(|m| m.role == "assistant")
              .map(|m| m.content.clone())
              .unwrap_or_else(|| subject.clone())
          };
          (current_agent, user_msg)
        };

        if !user_msg.is_empty() {
          let system_prompt = current_agent.system_prompt.replace("\\n", "\n");
          let messages = create_basic_messages(system_prompt, user_msg.clone());

          let my_interrupt = interrupt_counter.load(Ordering::SeqCst);
          let reply = rt
            .block_on(get_response(messages, current_agent))
            .unwrap_or_else(|e| {
              crate::log::log("error", &format!("Error getting debate response: {}", e));
              String::new()
            });

          // Check if we were interrupted during LLM response
          if interrupt_counter.load(Ordering::SeqCst) != my_interrupt {
            crate::log::log("debug", "LLM response interrupted, discarding");
            continue;
          }

          if !reply.is_empty() {
            // Add to conversation history
            conversation_history.lock().unwrap().push(ChatMessage {
              role: "assistant".to_string(),
              content: reply.clone(),
            });

            // Display in UI
            let _ = tx_ui.send("line| ".to_string());
            let label = format!("\x1b[48;5;22;37m{}:\x1b[0m", current_agent.name);
            let _ = tx_ui.send(format!("line|{}", label));
            let _ = tx_ui.send(format!("stream|{}", reply.trim()));
            let _ = tx_ui.send("line|".to_string());

            // Temporarily switch to current agent's voice/tts/language/baseurl settings
            let originals = apply_agent_settings(state, current_agent);

            // Send to TTS with current agent's voice and wait for each phrase
            process_tts_phrases(
              &reply,
              &tts_tx,
              &tts_done_rx,
              current_agent.voice.clone(),
              &interrupt_counter,
              my_interrupt,
            );

            restore_agent_settings(state, originals);

            // Check again for interruption before waiting for playback
            if interrupt_counter.load(Ordering::SeqCst) != my_interrupt {
              crate::log::log("debug", "Playback interrupted");
              continue;
            }

            wait_for_playback(state, &interrupt_counter, my_interrupt);
          }

          // Increment turn only if not interrupted
          if interrupt_counter.load(Ordering::SeqCst) == my_interrupt {
            state.debate_turn.fetch_add(1, Ordering::SeqCst);
          }
        }

        continue;
      }
    }

    //  –––––––––––––––––––––––––––––––––––––
    //   conversation mode
    //  –––––––––––––––––––––––––––––––––––––
    if !state.debate_enabled.load(Ordering::SeqCst) {
      if let Some(user_msg) = pending_user_msg.take() {
        // Build messages for LLM
        let system_prompt = settings.system_prompt.replace("\\n", "\n");
        let messages = create_basic_messages(system_prompt, user_msg.clone());

        let my_interrupt = interrupt_counter.load(Ordering::SeqCst);
        let reply = rt
          .block_on(get_response(messages, &settings))
          .unwrap_or_else(|e| {
            crate::log::log("error", &format!("Error getting response: {}", e));
            String::new()
          });

        if !reply.is_empty() {
          conversation_history.lock().unwrap().push(ChatMessage {
            role: "assistant".to_string(),
            content: reply.clone(),
          });

          // Display in UI
          let _ = tx_ui.send("line| ".to_string());
          let label = format!("\x1b[48;5;22;37m{}:\x1b[0m", settings.name);
          let _ = tx_ui.send(format!("line|{}", label));
          let _ = tx_ui.send(format!("stream|{}", reply.trim()));
          let _ = tx_ui.send("line|".to_string());

          let originals = apply_agent_settings(state, &settings);

          process_tts_phrases(
            &reply,
            &tts_tx,
            &tts_done_rx,
            settings.voice.clone(),
            &interrupt_counter,
            my_interrupt,
          );

          restore_agent_settings(state, originals);

          wait_for_playback(state, &interrupt_counter, my_interrupt);
        }
      }
    }

    select! {
      recv(stop_all_rx) -> _ => break,
      recv(rx_utt) -> msg => {
        //  –––––––––––––––––––––––––––––––––––––
        //   user audio input handler
        //  –––––––––––––––––––––––––––––––––––––
        let Ok(utt) = msg else { break };
        // Drain any pending stop signals from previous turn
        while stop_all_rx.try_recv().is_ok() {}

        let state = GLOBAL_STATE.get().expect("AppState not initialized");
        state.conversation_paused.store(false, Ordering::Relaxed);
        // start rendering for this turn (agent response to user query)
        state.processing_response.store(true, Ordering::Relaxed);
        let pcm_f32: Vec<f32> = utt.data.clone();
         let mono_f32 = crate::audio::convert_to_mono(&utt);

        crate::log::log("debug", &format!("Received audio chunk of len {}", utt.data.len()));
        crate::log::log("debug", &format!("Received mono f32 pcm len {}", pcm_f32.len()));
        crate::log::log("debug", "Transcribing utterance...");
        let state = GLOBAL_STATE.get().expect("AppState not initialized");
        let user_text = crate::stt::whisper_transcribe_with_ctx(&ctx, &mono_f32, utt.sample_rate, &state.language.lock().unwrap())?;
        crate::log::log("info", &format!("Transcribed: '{}'", user_text));
        let system_prompt = {
          let state = GLOBAL_STATE.get().expect("AppState not initialized");
          state.system_prompt.lock().unwrap().clone()
        };
        let hist = conversation_history.lock().unwrap();
        let mut messages = Vec::new();
        messages.push(ChatMessage{role:"system".to_string(), content:system_prompt.replace("\\n", "\n")});
        for m in hist.iter() {
          messages.push(m.clone());
        }
        // Release the conversation history lock before re-acquiring it to push the user message
        std::mem::drop(hist);
        messages.push(ChatMessage{role:"user".to_string(), content:user_text.clone()});
        let user_text = user_text.trim().to_string();
        let speech_end_ms = crate::util::SPEECH_END_AT.load(std::sync::atomic::Ordering::SeqCst);
        let mut first_phrase_logged = false;
        if user_text.is_empty() {
          crate::log::log("debug", "Transcription returned empty string");
          continue;
        }

        // Print user line (keep spinner/emojis only on the latest bottom line).
        let my_interrupt = interrupt_counter.load(Ordering::SeqCst);
        if handle_interruption(&interrupt_counter, my_interrupt) {
          interrupt_counter.store(my_interrupt, Ordering::SeqCst);
          continue;
        }
        // Clear STOP_STREAM flag to ensure user text displays fully
        crate::ui::STOP_STREAM.store(false, Ordering::Relaxed);
         send_user_message_ui(&tx_ui, &user_text, false);
         push_user_message(&conversation_history, &user_text);


        // Check if debate mode is enabled
        let state = GLOBAL_STATE.get().expect("AppState not initialized");
        if state.debate_enabled.load(Ordering::SeqCst) {
          // User has interrupted the debate with new input
          // Update debate subject and continue debate
          {
            let mut subject = state.debate_subject.lock().unwrap();
            *subject = user_text.clone();
          }
          // Reset turn counter so debate continues
          state.debate_turn.store(0, Ordering::SeqCst);
          // Signal playback is done for user input
          state.playback.playback_active.store(false, Ordering::Relaxed);
          continue;
        }

        ui.thinking.store(true, Ordering::Relaxed);

        // Snapshot interruption counter for this assistant turn.
        let speaker_arc = std::sync::Arc::new(std::sync::Mutex::new(PhraseSpeaker::new()));
        let mut got_any_token = false;

        let _ = tx_ui.send("line| ".to_string());
        let _ = tx_ui.send(format!("line|{}", crate::ui::ASSIST_LABEL));

        let mut interrupted = false;

        // clones for the on_piece closure
        let stop_all_rx_cloned_for_closure = stop_all_rx.clone();
        let stop_all_tx_cloned_for_closure = stop_all_tx.clone();
        let speaker_arc_cloned_for_closure = speaker_arc.clone();
        let tx_ui_cloned_for_closure = tx_ui.clone();
        let tts_tx_cloned_for_closure = tts_tx.clone();
        let ui_thinking_cloned_for_closure = ui.thinking.clone();
        let conversation_history_cloned_for_closure = conversation_history.clone();
        // clones for closure
        let ui_thinking_for_closure = ui_thinking_cloned_for_closure.clone();
        let conversation_history_for_closure_cloned = conversation_history_cloned_for_closure.clone();

        // called on every chunk received from llm
        let voice_for_tts = settings.voice.clone();
        let on_piece = move |piece: &str| {
          let hist = conversation_history_for_closure_cloned.clone();
          if interrupted {
            let _ = stop_all_tx_cloned_for_closure.try_send(());
            return;
          }
          if piece.is_empty() {
            return;
          }
          if stop_all_rx_cloned_for_closure.try_recv().is_ok() {
            interrupted = true;
            speaker_arc_cloned_for_closure.lock().unwrap().buf.clear();
            return;
          }
          if !got_any_token && !piece.is_empty() {
            got_any_token = true;
            ui_thinking_for_closure.store(false, Ordering::Relaxed);
          }
          if let Some(phrase) = speaker_arc_cloned_for_closure.lock().unwrap().push_text(piece) {
            if !first_phrase_logged {
              let elapsed_ms = crate::util::now_ms(&START_INSTANT) - speech_end_ms;
              crate::log::log("info", &format!("Time from speech end to first phrase playback: {:.2?}", elapsed_ms));
              first_phrase_logged = true;
            }
            hist.lock().unwrap().push(ChatMessage{role:"assistant".to_string(), content:phrase.clone()});
            // send the complete phrase to tts
            let cleaned = crate::util::strip_special_chars(&phrase);
            crate::log::log("info", &format!("Sending phrase to TTS: '{}' (original: '{}'), interrupt={}", cleaned, phrase, my_interrupt));
            let _ = tts_tx_cloned_for_closure.send((cleaned, my_interrupt, voice_for_tts.clone()));
          }
          // send raw piece immediately
          let _ = tx_ui_cloned_for_closure.send(format!("stream|{}", piece));
        };

        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let stop_all_rx_cloned = stop_all_rx.clone();
        let ollama_url = state.baseurl.lock().unwrap().clone();
        let interrupt_counter_cloned = interrupt_counter.clone();
        let llama_url = state.baseurl.lock().unwrap().clone();
        let model = state.model.lock().unwrap().clone();
        let engine_type = state.provider.lock().unwrap().clone();

        if *state.provider.lock().unwrap() == "llama-server" {
          let on_piece_cloned = std::sync::Arc::new(std::sync::Mutex::new(on_piece));
          let handle = std::thread::spawn(move || {
            rt.block_on(async {
              crate::log::log("info", "eoo");
              match crate::llm::llama_server_stream_response_into (
                &messages,
                llama_url.as_str(),
                model.as_str(),
                engine_type.as_str(),
                &stop_all_rx_cloned,
                interrupt_counter_cloned.clone(),
                my_interrupt,
                &mut *on_piece_cloned.lock().unwrap()
              ).await {
                Ok(_) => Ok(()),
                Err(e) => {
                  crate::log::log("error", &format!("llama server error: {e}. Make sure llama-server / llamafile is running"));
                  Err(e)
                }
              }
            })
          });
          // ignore join result to prevent panic on llama server error
          let _join_result = handle.join();
        } else {
          let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
          let on_piece_cloned = std::sync::Arc::new(std::sync::Mutex::new(on_piece));
          let handle = std::thread::spawn(move || {
            rt.block_on(async {
              match crate::llm::llama_server_stream_response_into (
                &messages,
                ollama_url.as_str(),
                model.as_str(),
                engine_type.as_str(),
                &stop_all_rx_cloned,
                interrupt_counter_cloned.clone(),
                my_interrupt,
                &mut *on_piece_cloned.lock().unwrap()
              ).await {
                Ok(_) => Ok(()),
                Err(e) => {
                  crate::log::log("error", &format!("ollama error. {}. Make sure ollama is running and model '{}' is available", e, model.as_str()));
                  Err(e)
                }
              }
            })
          });
          // ignore join result to prevent panic on llama server error
          let _join_result = handle.join();
        }
        ui_thinking_cloned_for_closure.store(false, Ordering::Relaxed);
        if let Some(phrase) = speaker_arc.lock().unwrap().flush() {
          let phrase_clone = phrase.clone();
          let _ = tx_ui.send(phrase_clone);
          conversation_history.lock().unwrap().push(ChatMessage{role:"assistant".to_string(), content:phrase.clone()});
          let cleaned = crate::util::strip_special_chars(&phrase);
          crate::log::log("info", &format!("Sending final phrase to TTS: '{}' (original: '{}'), interrupt={}", cleaned, phrase, my_interrupt));
          let _ = tts_tx.send((cleaned, my_interrupt, settings.voice.clone()));
        }
      }
    }
  }
  Ok(())
}

// PRIVATE
// ------------------------------------------------------------------

/// Get response from LLM for debate mode (synchronous, non-streaming)
async fn get_response(
  messages: Vec<ChatMessage>,
  agent: &crate::config::AgentSettings,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
  let (_stop_tx, stop_rx) = crossbeam_channel::unbounded::<()>();
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

/// Emits phrases when punctuation/newline/length threshold happens.
struct PhraseSpeaker {
  buf: String,
}
impl PhraseSpeaker {
  fn new() -> Self {
    Self { buf: String::new() }
  }
  fn push_text(&mut self, s: &str) -> Option<String> {
    self.buf.push_str(s);
    // cap phrases by new lines or dots
    let trigger = self.buf.contains('\n') || self.buf.ends_with('.');
    if trigger { self.flush() } else { None }
  }
  fn flush(&mut self) -> Option<String> {
    let out = self.buf.trim().to_string();
    self.buf.clear();
    if out.is_empty() { None } else { Some(out) }
  }
}

fn handle_interruption(interrupt_counter: &Arc<AtomicU64>, current: u64) -> bool {
  if interrupt_counter.load(Ordering::SeqCst) != current {
    true
  } else {
    false
  }
}

/// Split text into phrases for TTS (used in debate mode)
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

fn send_user_message_ui(tx_ui: &Sender<String>, text: &str, use_stream: bool) {
  let _ = tx_ui.send("line|\n".to_string());
  let _ = tx_ui.send(format!("line|{}", crate::ui::USER_LABEL));
  let msg = if use_stream {
    format!("stream|{}", text)
  } else {
    format!("line|{}", text)
  };
  let _ = tx_ui.send(msg);
  let _ = tx_ui.send("line|".to_string());
}

fn push_user_message(history: &ConversationHistory, text: &str) {
  history.lock().unwrap().push(ChatMessage {
    role: "user".to_string(),
    content: text.to_string(),
  });
}

fn wait_for_playback(
  state: &crate::state::AppState,
  interrupt_counter: &Arc<AtomicU64>,
  my_interrupt: u64,
) {
  let playback_active = state.playback.playback_active.clone();
  while playback_active.load(Ordering::SeqCst) {
    if interrupt_counter.load(Ordering::SeqCst) != my_interrupt {
      break;
    }
    std::thread::sleep(std::time::Duration::from_millis(10));
  }
}

fn process_tts_phrases(
  reply: &str,
  tts_tx: &Sender<(String, u64, String)>,
  tts_done_rx: &Receiver<()>,
  voice: String,
  interrupt_counter: &Arc<AtomicU64>,
  my_interrupt: u64,
) {
  let phrases = split_into_phrases(reply);
  for phrase in phrases {
    if interrupt_counter.load(Ordering::SeqCst) != my_interrupt {
      break;
    }
    let cleaned = crate::util::strip_special_chars(&phrase);
    let _ = tts_tx.send((cleaned, my_interrupt, voice.clone()));
    let _ = tts_done_rx.recv();
  }
}

fn create_basic_messages(system_prompt: String, user_msg: String) -> Vec<ChatMessage> {
  vec![
    ChatMessage {
      role: "system".to_string(),
      content: system_prompt,
    },
    ChatMessage {
      role: "user".to_string(),
      content: user_msg,
    },
  ]
}

fn apply_agent_settings(
  state: &crate::state::AppState,
  agent: &crate::config::AgentSettings,
) -> (String, String, String, String) {
  let original_voice = state.voice.lock().unwrap().clone();
  let original_tts = state.tts.lock().unwrap().clone();
  let original_language = state.language.lock().unwrap().clone();
  let original_baseurl = state.baseurl.lock().unwrap().clone();

  *state.voice.lock().unwrap() = agent.voice.clone();
  *state.tts.lock().unwrap() = agent.tts.clone();
  *state.language.lock().unwrap() = agent.language.clone();
  *state.baseurl.lock().unwrap() = agent.baseurl.clone();

  (
    original_voice,
    original_tts,
    original_language,
    original_baseurl,
  )
}

fn restore_agent_settings(
  state: &crate::state::AppState,
  originals: (String, String, String, String),
) {
  let (voice, tts, language, baseurl) = originals;
  *state.voice.lock().unwrap() = voice;
  *state.tts.lock().unwrap() = tts;
  *state.language.lock().unwrap() = language;
  *state.baseurl.lock().unwrap() = baseurl;
}
