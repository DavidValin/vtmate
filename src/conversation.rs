// ------------------------------------------------------------------
//  Conversation
// ------------------------------------------------------------------

use crate::state::GLOBAL_STATE;
use crate::START_INSTANT;
use crossbeam_channel::{select, Receiver, Sender};
use std::cell::Cell;
use std::sync::OnceLock;
use std::sync::{
  atomic::{AtomicU64, Ordering},
  Arc,
};

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
  tts_tx: Sender<(String, u64)>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
  let ctx = init_whisper_context(&model_path);
  crate::log::log("info", &format!("LLM model: {}", settings.model));

  loop {
    select! {
      recv(stop_all_rx) -> _ => break,
      recv(rx_utt) -> msg => {
        let Ok(utt) = msg else { break };
        // Drain any pending stop signals from previous turn
        while stop_all_rx.try_recv().is_ok() {}

        let state = GLOBAL_STATE.get().expect("AppState not initialized");
        state.playback.playback_active.store(true, Ordering::Relaxed);
        state.conversation_paused.store(false, Ordering::Relaxed);
        // start rendering for this turn (agent response to user query)
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
        messages.push(ChatMessage{role:"system".to_string(), content:system_prompt});
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
        let _ = tx_ui.send("line|\n".to_string());
        let _ = tx_ui.send(format!("line|{}", crate::ui::USER_LABEL));
        let _ = tx_ui.send(format!("line|{}", user_text));
        let _ = tx_ui.send("line|\n".to_string());

        conversation_history.lock().unwrap().push(ChatMessage{role:"user".to_string(), content:user_text.clone()});
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
            let _ = tts_tx_cloned_for_closure.send((strip_special_chars(&phrase), my_interrupt));
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
                  crate::log::log("error", &format!("ollama error. {e}. Make sure ollama is running"));
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
          let current_interrupt = interrupt_counter.load(Ordering::SeqCst);
          let _ = tts_tx.send((phrase.clone(), current_interrupt));
        }
      }
    }
  }
  Ok(())
}

// PRIVATE
// ------------------------------------------------------------------

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
    if trigger {
      self.flush()
    } else {
      None
    }
  }
  fn flush(&mut self) -> Option<String> {
    let out = self.buf.trim().to_string();
    self.buf.clear();
    if out.is_empty() {
      None
    } else {
      Some(out)
    }
  }
}

thread_local! {
  static IN_CODE_BLOCK: Cell<bool> = Cell::new(false);
}

fn handle_interruption(interrupt_counter: &Arc<AtomicU64>, current: u64) -> bool {
  if interrupt_counter.load(Ordering::SeqCst) != current {
    true
  } else {
    false
  }
}

fn strip_special_chars(s: &str) -> String {
  let mut result = String::new();
  let parts: Vec<&str> = s.split("```").collect();
  let mut inside = IN_CODE_BLOCK.with(|c| c.get());
  for (i, part) in parts.iter().enumerate() {
    if !inside {
      result.extend(part.chars().filter(|c| {
        ![
          '+', '.', '~', '*', '&', '-', ',', ';', ':', '(', ')', '[', ']', '{', '}', '"', '\'', '#', '`', '|'
        ]
        .contains(c)
      }));
    }
    // toggle after each fence except after last part
    if i < parts.len() - 1 {
      inside = !inside;
    }
  }
  IN_CODE_BLOCK.with(|c| c.set(inside));
  result
}
