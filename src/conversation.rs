// ------------------------------------------------------------------
//  Conversation
// ------------------------------------------------------------------

use crate::START_INSTANT;
use crate::playback::set_wav_tx;
use crate::state::AppState;
use crate::state::GLOBAL_STATE;
use crate::util::terminate;
use chrono::Local;
use crossbeam_channel::{Receiver, Sender, select};
use hound;
use std::fs;
use std::path::Path;
use std::sync::OnceLock;
use std::sync::{
  Arc, Mutex,
  atomic::{AtomicU64, Ordering},
};
use std::thread;
use std::time::Duration;
use tokio::runtime::Builder as TokioBuilder;
use uuid::Uuid;

static WHISPER_CTX: OnceLock<whisper_rs::WhisperContext> = OnceLock::new();

// API
// ------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChatMessage {
  pub role: String,
  pub content: String,
  pub agent_name: Option<String>,
}

pub type ConversationHistory = std::sync::Arc<std::sync::Mutex<Vec<ChatMessage>>>;

/// Commands sent from keyboard to conversation thread
pub enum Command {
  Undo,
}

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
  interrupt_counter: Arc<AtomicU64>,
  model_path: String,
  settings: crate::config::AgentSettings,
  ui: crate::state::UiState,
  conversation_history: ConversationHistory,
  tx_ui: Sender<String>,
  tts_tx: Sender<(String, u64, String)>,
  tts_done_rx: Receiver<()>,
  stop_play_tx: Sender<()>,
  rx_cmd: Receiver<Command>,
  init_prompt: Option<String>,
  quiet: bool,
  save: bool,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
  let ctx = init_whisper_context(&model_path);

  // WAV writer thread: activated when -s option is used
  // WAV writer will be started lazily when the first save path is created.
  let mut wav_tx_opt: Option<crossbeam_channel::Sender<crate::audio::AudioChunk>> = None;

  crate::log::log("info", &format!("LLM model: {}", settings.model));

  let settings_clone = settings.clone();

  //  –––––––––––––––––––––––––––––––––––––
  //   quiet mode
  //  –––––––––––––––––––––––––––––––––––––
  if quiet {
    crate::log::log("info", "Running in quiet mode");

    // Setup save path and WAV writer if saving is requested
    if save {
      maybe_setup_and_save(
        &mut wav_tx_opt,
        &conversation_history,
        &settings_clone,
        save,
      )?;
    }

    let rt = TokioBuilder::new_current_thread()
      .enable_all()
      .build()
      .unwrap();

    if let Some(prompt) = &init_prompt {
      // Show user message in UI
      send_user_message_ui(&tx_ui, &prompt, false);
      push_user_message(&conversation_history, &prompt);
      perform_save(&conversation_history, &settings_clone);
      let system_prompt = settings.system_prompt.replace("\\n", "\n");
      let messages = create_basic_messages(system_prompt, prompt.clone());

      let my_interrupt = interrupt_counter.load(Ordering::SeqCst);
      let messages_clone = messages.clone();
      let reply = rt
        .block_on(get_response(messages_clone, &settings))
        .unwrap_or_else(|e| {
          crate::log::log(
            "error",
            &format!("Error getting response in quiet mode: {}", e),
          );
          String::new()
        });
      if !reply.is_empty() {
        conversation_history.lock().unwrap().push(ChatMessage {
          role: "assistant".to_string(),
          content: reply.clone(),
          agent_name: Some(settings.name.clone()),
        });
        perform_save(&conversation_history, &settings_clone);
        // Display in UI
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
    terminate(0);
  }

  // Runtime to use for async debate responses
  let rt = TokioBuilder::new_current_thread()
    .enable_all()
    .build()
    .unwrap();

  // Track interruptions for debate mode
  let mut last_interrupt = interrupt_counter.load(Ordering::SeqCst);
  let mut debate_interrupted = false;
  let mut pending_user_msg: Option<String> = init_prompt;
  let mut prev_debate_enabled = false;

  let state = GLOBAL_STATE.get().expect("AppState not initialized");
  if state.debate_enabled.load(Ordering::SeqCst) {
    // render the initial user message for the debate
    if let Some(msg) = &pending_user_msg {
      if !msg.is_empty() {
        send_user_message_ui(&tx_ui, msg, false);
        push_user_message(&conversation_history, msg);
        perform_save(&conversation_history, &settings_clone);
      }
    } else {
      // If no initial prompt, use debate subject as first user message
      let subject = state.debate_subject.lock().unwrap();
      if !subject.is_empty() {
        let msg = subject.clone();
        send_user_message_ui(&tx_ui, &msg, false);
        push_user_message(&conversation_history, &msg);
        perform_save(&conversation_history, &settings_clone);
      }
    }
  }

  //  –––––––––––––––––––––––––––––––––––––
  //   loop
  //  –––––––––––––––––––––––––––––––––––––
  loop {
    // Detect transition to debate mode
    let current_debate_enabled = state.debate_enabled.load(Ordering::SeqCst);
    if current_debate_enabled && !prev_debate_enabled {
      // Reset state for new debate: clear pending message and interrupt flag
      pending_user_msg = None;
      debate_interrupted = false;
      // Also reset last_interrupt to avoid false interruption detection
      last_interrupt = interrupt_counter.load(Ordering::SeqCst);
    }
    prev_debate_enabled = current_debate_enabled;

    if save && state.save_path.lock().unwrap().is_none() {
      maybe_setup_and_save(
        &mut wav_tx_opt,
        &conversation_history,
        &settings_clone,
        save,
      )?;
    }

    if !state.debate_enabled.load(Ordering::SeqCst) {
      if let Some(ref prompt) = pending_user_msg {
        send_user_message_ui(&tx_ui, prompt, false);
        push_user_message(&conversation_history, prompt);
        perform_save(&conversation_history, &settings_clone);
        pending_user_msg = Some(prompt.clone());
      }
    }

    //  –––––––––––––––––––––––––––––––––––––
    //   debate mode
    //  –––––––––––––––––––––––––––––––––––––
    if state.debate_enabled.load(Ordering::SeqCst) {
      let debate_agents = state.debate_agents.lock().unwrap().clone();
      if debate_agents.len() >= 2 {
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
          let _ = stop_play_tx.try_send(());
          // Skip to waiting for user input
          crate::log::log("debug", "Debate interrupted, waiting for user input");
        }

        // Check for user input with short timeout
        let user_input_result = rx_utt.recv_timeout(std::time::Duration::from_millis(100));

        if let Ok(utt) = user_input_result {
          // User provided input - process it
          let state = GLOBAL_STATE.get().expect("AppState not initialized");
          state.conversation_paused.store(false, Ordering::Relaxed);
          // Resume debate if it was paused
          state.debate_paused.store(false, Ordering::SeqCst);
          state.processing_response.store(true, Ordering::Relaxed);

          // Apply settings of the agent that will respond next
          let debate_agents = state.debate_agents.lock().unwrap().clone();
          let turn = state.debate_turn.load(Ordering::SeqCst) as usize;
          let agent_count = debate_agents.len();
          let next_agent = &debate_agents[turn % agent_count];
          let _ = apply_agent_settings(state, next_agent);

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
            perform_save(&conversation_history, &settings_clone);

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

        if state.debate_paused.load(Ordering::SeqCst) {
          thread::sleep(Duration::from_millis(100));
          continue;
        }
        if !user_msg.is_empty() {
          let my_interrupt = interrupt_counter.load(Ordering::SeqCst);
          // If interrupted before starting LLM request, skip
          if interrupt_counter.load(Ordering::SeqCst) != my_interrupt {
            continue;
          }
          // Set recording pause based on current agent's ptt
          state
            .recording_paused
            .store(current_agent.ptt, Ordering::Relaxed);
          // Stop any ongoing playback
          state
            .playback
            .playback_active
            .store(false, Ordering::Relaxed);
          let _ = stop_play_tx.try_send(());
          let _reply_opt = handle_reply(
            state,
            current_agent,
            &conversation_history,
            &tx_ui,
            &tts_tx,
            &tts_done_rx,
            &rt,
            &interrupt_counter,
            user_msg.clone(),
          );
          state.processing_response.store(false, Ordering::Relaxed);
          // important: next agent will reply to this response using history

          // Increment turn only if not interrupted
          if interrupt_counter.load(Ordering::SeqCst) == my_interrupt {
            if !state.debate_paused.load(Ordering::SeqCst) {
              state.debate_turn.fetch_add(1, Ordering::SeqCst);
            }
          }

          // Reset debate_interrupted flag
          debate_interrupted = false;
          // (turn already advanced)
        }

        continue;
      }
    }

    //  –––––––––––––––––––––––––––––––––––––
    //   conversation mode
    //  –––––––––––––––––––––––––––––––––––––
    if !state.debate_enabled.load(Ordering::SeqCst) {
      if let Some(user_msg) = pending_user_msg.take() {
        handle_reply(
          state,
          &settings,
          &conversation_history,
          &tx_ui,
          &tts_tx,
          &tts_done_rx,
          &rt,
          &interrupt_counter,
          user_msg,
        );
      }
    }

    select! {
       recv(rx_cmd) -> cmd => {
         if let Ok(command) = cmd {
           match command {
             Command::Undo => {
                handle_undo(state, &tx_ui, &conversation_history, &interrupt_counter, &stop_play_tx, &settings);
              }
           }
         }
       }
      recv(rx_utt) -> msg => {
        //  –––––––––––––––––––––––––––––––––––––
        //   user audio input handler
        //  –––––––––––––––––––––––––––––––––––––
        let Ok(utt) = msg else { break };
        if let Some(ref wav_tx) = wav_tx_opt {
          wav_tx.send(utt.clone()).unwrap_or(());
        }

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
        messages.push(ChatMessage{role:"system".to_string(), content:system_prompt.replace("\\n", "\n"), agent_name:None});

        for m in hist.iter() {
          messages.push(m.clone());
        }
        // Release the conversation history lock before re-acquiring it to push the user message
        std::mem::drop(hist);
        messages.push(ChatMessage{role:"user".to_string(), content:user_text.clone(), agent_name:None});

        let user_text = user_text.trim().to_string();
        let speech_end_ms = crate::util::SPEECH_END_AT.load(std::sync::atomic::Ordering::SeqCst);
        let mut first_phrase_logged = false;
        if user_text.is_empty() {
          crate::log::log("debug", "Transcription returned empty string");
          continue;
        }

        let my_interrupt = interrupt_counter.load(Ordering::SeqCst);
        if handle_interruption(&interrupt_counter, my_interrupt) {
          interrupt_counter.store(my_interrupt, Ordering::SeqCst);
          continue;
        }

        // Clear STOP_STREAM flag to ensure user text displays fully
        crate::ui::STOP_STREAM.store(false, Ordering::Relaxed);
        send_user_message_ui(&tx_ui, &user_text, false);
        push_user_message(&conversation_history, &user_text);
        perform_save(&conversation_history, &settings_clone);

        // Check if debate mode is enabled
        let state = GLOBAL_STATE.get().expect("AppState not initialized");
        if state.debate_enabled.load(Ordering::SeqCst) {
        debate_interrupted = false;
          // User has interrupted the debate with new input
          // Update debate subject and continue debate
          {
            let mut subject = state.debate_subject.lock().unwrap();
            *subject = user_text.clone();
          }
          // Stop playback immediately
          let _ = stop_play_tx.try_send(());
          // Signal playback is done for user input
          state.playback.playback_active.store(false, Ordering::Relaxed);
          continue;
        }

        ui.thinking.store(true, Ordering::Relaxed);

        // Snapshot interruption counter for this assistant turn.
        let speaker_arc = std::sync::Arc::new(std::sync::Mutex::new(PhraseSpeaker::new()));
        let mut got_any_token = false;

        let _ = tx_ui.send("line|".to_string());
        let _ = tx_ui.send(format!("line|{}", crate::ui::ASSIST_LABEL));

        // clones for the on_piece closure
        let speaker_arc_cloned_for_closure = speaker_arc.clone();
        let tx_ui_cloned_for_closure = tx_ui.clone();
        let tts_tx_cloned_for_closure = tts_tx.clone();
        let ui_thinking_cloned_for_closure = ui.thinking.clone();
        // clones for closure
        let ui_thinking_for_closure = ui_thinking_cloned_for_closure.clone();

        // called on every chunk received from llm
        let voice_for_tts = state.voice.lock().unwrap().clone();
        let voice_for_tts_inner = voice_for_tts.clone();
        // Clone for use inside closure

        // reply accumulator for single ChatMessage
        let reply_accum = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
        let reply_accum_cloned = reply_accum.clone();
        let on_piece = move |piece: &str| {
          if piece.is_empty() {
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
              // accumulate reply for single ChatMessage
            if let Ok(mut acc) = reply_accum_cloned.lock() {
              acc.push_str(&phrase);
              acc.push(' ');
            }
            // send the complete phrase to tts
            let mut cleaned = crate::util::strip_special_chars(&phrase);
            cleaned.push(' ');
            crate::log::log("info", &format!("Sending phrase to TTS: '{}' (original: '{}'), interrupt={}", cleaned, phrase, my_interrupt));
            let _ = tts_tx_cloned_for_closure.send((cleaned, my_interrupt, voice_for_tts_inner.clone()));
          }

          // send raw piece immediately
          let mut ui_piece = piece.to_string();
          if ui_piece.ends_with('.') || ui_piece.ends_with('!') || ui_piece.ends_with('?') {
            ui_piece.push(' ');
          }
          let _ = tx_ui_cloned_for_closure.send(format!("stream|{}", ui_piece));
        };

        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let ollama_url = state.baseurl.lock().unwrap().clone();
        let interrupt_counter_cloned = interrupt_counter.clone();
        let llama_url = state.baseurl.lock().unwrap().clone();
        let model = state.model.lock().unwrap().clone();
        let engine_type = state.provider.lock().unwrap().clone();

        if *state.provider.lock().unwrap() == "llama-server" {
          let on_piece_cloned = std::sync::Arc::new(std::sync::Mutex::new(on_piece));
          let handle = std::thread::spawn(move || {
            rt.block_on(async {
              match crate::llm::llama_server_stream_response_into (
                &messages,
                llama_url.as_str(),
                model.as_str(),
                engine_type.as_str(),
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
        // Prepare clones for post-closure use
        let speaker_arc_for_after = speaker_arc.clone();
        let reply_accum_for_after = reply_accum.clone();
        let tts_tx_for_after = tts_tx.clone();
        let voice_for_tts_for_after = voice_for_tts.clone();

        // Flush any remaining phrase from the speaker when stream ends
        if let Some(last_phrase) = speaker_arc_for_after.lock().unwrap().flush() {
          // accumulate reply
          if let Ok(mut acc) = reply_accum_for_after.lock() {
            acc.push_str(&last_phrase);
            acc.push(' ');
          }
          // send to TTS
          let mut cleaned = crate::util::strip_special_chars(&last_phrase);
          cleaned.push(' ');
          let _ = tts_tx_for_after.send((cleaned, my_interrupt, voice_for_tts_for_after.clone()));
        }
        {
          // Retrieve and clear the accumulated reply
          let acc_clone = {
            let mut acc = reply_accum.lock().unwrap();
            let cloned = acc.clone();
            acc.clear();
            cloned
          };
          if !acc_clone.is_empty() {
            conversation_history.lock().unwrap().push(ChatMessage{role:"assistant".to_string(), content:acc_clone, agent_name:Some(settings.name.clone())});
            perform_save(&conversation_history, &settings_clone);
          }
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
    interrupt_counter.clone(),
    0,
    &mut on_piece,
  )
  .await?;
  Ok(result)
}

/// Persist conversation history if needed
fn perform_save(
  conversation_history: &ConversationHistory,
  settings: &crate::config::AgentSettings,
) {
  let state = GLOBAL_STATE.get().expect("AppState not initialized");
  let save_path = state.save_path.lock().unwrap().clone();
  if let Some(path) = save_path {
    let is_debate = state.debate_enabled.load(Ordering::SeqCst);
    let agents = if is_debate {
      state.debate_agents.lock().unwrap().clone()
    } else {
      vec![settings.clone()]
    };
    let metadata = SaveMetadata {
      start_date: state.start_date.lock().unwrap().clone(),
      agents,
      is_debate,
      system_prompt: settings.system_prompt.clone(),
      voice: settings.voice.clone(),
    };
    let _ = save_conversation(conversation_history, Some(&path), Some(&metadata));
  }
}

fn maybe_setup_and_save(
  wav_tx_opt: &mut Option<crossbeam_channel::Sender<crate::audio::AudioChunk>>,
  conversation_history: &ConversationHistory,
  settings_clone: &crate::config::AgentSettings,
  save: bool,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
  if !save {
    return Ok(());
  }
  let state = GLOBAL_STATE.get().expect("AppState not initialized");
  if state.save_path.lock().unwrap().is_none() {
    let now = Local::now();
    let date_str = now.format("%Y-%m-%d_%H-%M-%S").to_string();
    let uuid_str = &Uuid::new_v4().to_string()[..8];
    let home = crate::util::get_user_home_path().ok_or("Unable to determine home directory")?;
    let path = home
      .join(".vtmate")
      .join("conversations")
      .join(format!("{}_{}.txt", date_str, uuid_str));

    *state.save_path.lock().unwrap() = Some(path.clone());
    *state.start_date.lock().unwrap() = date_str;

    if let Some(txt_path) = state.save_path.lock().unwrap().clone() {
      let wav_path = txt_path.with_extension("wav");
      let (wav_tx, wav_rx) = crossbeam_channel::unbounded::<crate::audio::AudioChunk>();
      set_wav_tx(wav_tx.clone());
      std::thread::spawn(move || {
        let mut writer: Option<hound::WavWriter<std::io::BufWriter<std::fs::File>>> = None;
        while let Ok(chunk) = wav_rx.recv() {
          if writer.is_none() {
            let spec = hound::WavSpec {
              channels: chunk.channels,
              sample_rate: chunk.sample_rate,
              bits_per_sample: 16,
              sample_format: hound::SampleFormat::Int,
            };
            writer = Some(hound::WavWriter::create(&wav_path, spec).unwrap());
          }
          let samples = crate::audio::f32_to_i16(&chunk.data);
          for s in samples {
            writer.as_mut().unwrap().write_sample(s).unwrap();
          }
          let silence_samples = (chunk.sample_rate * 500 / 1000) as usize * chunk.channels as usize;
          for _ in 0..silence_samples {
            writer.as_mut().unwrap().write_sample(0_i16).unwrap();
          }
          writer.as_mut().unwrap().flush().unwrap();
        }
      });
      *wav_tx_opt = Some(wav_tx);
    }
  }

  // perform save
  let state = GLOBAL_STATE.get().expect("AppState not initialized");
  let save_path = state.save_path.lock().unwrap().clone();
  if let Some(path) = save_path {
    let is_debate = state.debate_enabled.load(Ordering::SeqCst);
    let agents = if is_debate {
      state.debate_agents.lock().unwrap().clone()
    } else {
      vec![settings_clone.clone()]
    };
    let metadata = SaveMetadata {
      start_date: state.start_date.lock().unwrap().clone(),
      agents,
      is_debate,
      system_prompt: settings_clone.system_prompt.clone(),
      voice: settings_clone.voice.clone(),
    };
    let _ = save_conversation(conversation_history, Some(&path), Some(&metadata));
  }
  Ok(())
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

fn handle_undo(
  state: &AppState,
  tx_ui: &Sender<String>,
  conversation_history: &ConversationHistory,
  interrupt_counter: &Arc<AtomicU64>,
  stop_play_tx: &Sender<()>,
  settings: &crate::config::AgentSettings,
) {
  // Check if this undo was triggered during an ongoing response
  // (keyboard thread sets this flag and increments the interrupt counter)
  let was_interrupted = state.undo_pending.swap(false, Ordering::SeqCst);

  // If a response was in progress, interrupt it (same as Esc)
  if was_interrupted {
    // Remove partial assistant message if present
    let mut h = conversation_history.lock().unwrap();
    if let Some(last) = h.last() {
      if last.role == "assistant" {
        h.pop();
      }
    }
    drop(h);
    // Reset processing flag after interrupt
    state.processing_response.store(false, Ordering::Relaxed);
    interrupt_counter.fetch_add(1, Ordering::SeqCst);
    let _ = stop_play_tx.try_send(());
    let _ = tx_ui.send("user_interrupt_show|".to_string());
    // The interrupted response was NOT saved to history (interrupt check in streaming code),
    // so we do NOT pop — the user message that triggered it stays.
  } else {
    // No ongoing response: remove the last message from history
    let mut h = conversation_history.lock().unwrap();
    h.pop();
    drop(h);
  }

  // Clear and re-render history
  let _ = tx_ui.send("redraw_full_history|".to_string());
  let _ = tx_ui.send("line|\n\x1b[32m✨ Last message reverted \x1b[0m\n".to_string());

  // Persist conversation after undo
  perform_save(&conversation_history, settings);
}

/// Handle a single conversation reply when debate mode is disabled
// Helper to push or update last assistant message
fn push_or_update_last_assistant(
  conversation_history: &ConversationHistory,
  new_piece: &str,
  agent_name: &str,
) {
  let mut hist = conversation_history.lock().unwrap();
  if let Some(last) = hist.last_mut() {
    if last.role == "assistant" {
      last.content.push_str(new_piece);
      return;
    }
  }
  hist.push(ChatMessage {
    role: "assistant".to_string(),
    content: new_piece.to_string(),
    agent_name: Some(agent_name.to_string()),
  });
}

fn handle_reply(
  state: &AppState,
  settings: &crate::config::AgentSettings,
  conversation_history: &ConversationHistory,
  tx_ui: &Sender<String>,
  tts_tx: &Sender<(String, u64, String)>,
  tts_done_rx: &Receiver<()>,
  rt: &tokio::runtime::Runtime,
  interrupt_counter: &Arc<AtomicU64>,
  user_msg: String,
) -> Option<String> {
  // Build messages for LLM
  let system_prompt = settings.system_prompt.replace("\\n", "\n");
  let messages =
    create_full_context_messages(system_prompt, user_msg.clone(), conversation_history);

  let my_interrupt = interrupt_counter.load(Ordering::SeqCst);
  // Speaker for incremental buffering
  let speaker_arc = Arc::new(Mutex::new(PhraseSpeaker::new()));
  let reply_accum = Arc::new(Mutex::new(String::new()));
  let originals = apply_agent_settings(state, settings);
  let mut first_phrase_sent = false;
  let assistant_name = settings.name.clone();
  let assistant_name_for_closure = assistant_name.clone();
  let interrupt_counter_clone = interrupt_counter.clone();
  let my_interrupt_clone = my_interrupt;
  let mut on_piece = {
    let speaker_arc = speaker_arc.clone();
    let reply_accum = reply_accum.clone();
    let tts_tx = tts_tx.clone();
    let tx_ui = tx_ui.clone();
    let voice = settings.voice.clone();
    let conversation_history = conversation_history.clone();
    move |piece: &str| {
      if piece.is_empty() {
        return;
      }
      // Accumulate reply
      if let Ok(mut acc) = reply_accum.lock() {
        acc.push_str(piece);
      }
      // Buffer via speaker and get phrase (if delimiter reached)
      let phrase = {
        let mut speaker = speaker_arc.lock().unwrap();
        speaker.push_text(piece)
      };
      if let Some(ref phrase) = phrase {
        // First phrase: push new assistant message to history
        push_or_update_last_assistant(&conversation_history, piece, &assistant_name);
        // UI
        if !first_phrase_sent {
          let label = format!("\x1b[48;5;22;37m{}:\x1b[0m", assistant_name);
          let _ = tx_ui.send("line|".to_string());
          let _ = tx_ui.send(format!("line|{}", label));
          first_phrase_sent = true;
        }
        let _ = tx_ui.send(format!("stream|{}", phrase));
        let _ = tx_ui.send("line|".to_string());
        // TTS
        let _ = tts_tx.send((phrase.clone(), my_interrupt, voice.clone()));
        let _ = tts_done_rx.recv();
      }
      // If interrupted, flush any remaining buffered text into history
      if interrupt_counter_clone.load(Ordering::SeqCst) != my_interrupt_clone {
        if let Some(rem) = speaker_arc.lock().unwrap().flush() {
          push_or_update_last_assistant(&conversation_history, &rem, &assistant_name);
        }
      }
    }
  };

  let stream_result = rt.block_on(crate::llm::llama_server_stream_response_into(
    &messages,
    &settings.baseurl,
    &settings.model,
    &settings.provider,
    interrupt_counter.clone(),
    my_interrupt,
    &mut on_piece,
  ));
  if let Err(e) = stream_result {
    crate::log::log("error", &format!("Streaming error: {}", e));
    restore_agent_settings(state, originals);
    return None;
  }

  // Flush remaining phrase
  if let Some(last_phrase) = speaker_arc.lock().unwrap().flush() {
    let _ = tts_tx.send((last_phrase.clone(), my_interrupt, settings.voice.clone()));
    let _ = tx_ui.send(format!("stream|{}", last_phrase));
    let _ = tx_ui.send("line|".to_string());
    // Append any remaining buffered text to history
    push_or_update_last_assistant(
      &conversation_history,
      &last_phrase,
      &assistant_name_for_closure,
    );
  }

  // Final reply string
  let reply = {
    let mut acc = reply_accum.lock().unwrap();
    let cloned = acc.clone();
    acc.clear();
    cloned
  };
  // If interrupted, flush any remaining buffered text to history
  if interrupt_counter.load(Ordering::SeqCst) != my_interrupt {
    if let Some(rem) = speaker_arc.lock().unwrap().flush() {
      push_or_update_last_assistant(&conversation_history, &rem, &assistant_name_for_closure);
    }
  }

  // Persist conversation after streaming
  perform_save(&conversation_history, settings);

  // Restore settings and wait playback
  restore_agent_settings(state, originals);
  wait_for_playback(state, &interrupt_counter, my_interrupt);
  Some(reply)
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
    agent_name: None,
  });
}

fn wait_for_playback(
  state: &crate::state::AppState,
  interrupt_counter: &Arc<AtomicU64>,
  my_interrupt: u64,
) {
  let playback_active = state.playback.playback_active.clone();
  // Wait until playback starts if it hasn't already
  while !playback_active.load(Ordering::SeqCst) {
    if interrupt_counter.load(Ordering::SeqCst) != my_interrupt {
      return;
    }
    thread::sleep(Duration::from_millis(10));
  }
  // Playback is active, wait until it stops
  while playback_active.load(Ordering::SeqCst) {
    if interrupt_counter.load(Ordering::SeqCst) != my_interrupt {
      return;
    }
    thread::sleep(Duration::from_millis(10));
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
      agent_name: None,
    },
    ChatMessage {
      role: "user".to_string(),
      content: user_msg,
      agent_name: None,
    },
  ]
}

/// Build messages including full conversation history.
fn create_full_context_messages(
  system_prompt: String,
  user_msg: String,
  conversation_history: &ConversationHistory,
) -> Vec<ChatMessage> {
  let mut messages = Vec::new();
  // system message
  messages.push(ChatMessage {
    role: "system".to_string(),
    content: system_prompt,
    agent_name: None,
  });
  // history messages
  let hist = conversation_history.lock().unwrap();
  for m in hist.iter() {
    messages.push(m.clone());
  }
  // user message
  messages.push(ChatMessage {
    role: "user".to_string(),
    content: user_msg,
    agent_name: None,
  });
  messages
}

fn apply_agent_settings(
  state: &crate::state::AppState,
  agent: &crate::config::AgentSettings,
) -> (
  String,
  String,
  String,
  String,
  String,
  String,
  String,
  bool,
  u32,
) {
  // Store original settings
  let original_voice = state.voice.lock().unwrap().clone();
  let original_tts = state.tts.lock().unwrap().clone();
  let original_language = state.language.lock().unwrap().clone();
  let original_baseurl = state.baseurl.lock().unwrap().clone();
  let original_provider = state.provider.lock().unwrap().clone();
  let original_model = state.model.lock().unwrap().clone();
  let original_system_prompt = state.system_prompt.lock().unwrap().clone();
  let original_ptt = state.ptt.load(std::sync::atomic::Ordering::Relaxed);
  let original_speed = state.speed.load(std::sync::atomic::Ordering::Relaxed);

  // Apply new agent settings
  *state.voice.lock().unwrap() = agent.voice.clone();
  *state.tts.lock().unwrap() = agent.tts.clone();
  *state.language.lock().unwrap() = agent.language.clone();
  *state.baseurl.lock().unwrap() = agent.baseurl.clone();
  *state.provider.lock().unwrap() = agent.provider.clone();
  *state.model.lock().unwrap() = agent.model.clone();
  *state.system_prompt.lock().unwrap() = agent.system_prompt.clone();
  state
    .ptt
    .store(agent.ptt, std::sync::atomic::Ordering::Relaxed);
  state.speed.store(
    (agent.voice_speed * 10.0) as u32,
    std::sync::atomic::Ordering::Relaxed,
  );

  (
    original_voice,
    original_tts,
    original_language,
    original_baseurl,
    original_provider,
    original_model,
    original_system_prompt,
    original_ptt,
    original_speed,
  )
}

fn restore_agent_settings(
  state: &crate::state::AppState,
  originals: (
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    bool,
    u32,
  ),
) {
  let (voice, tts, language, baseurl, provider, model, system_prompt, ptt, speed) = originals;
  *state.voice.lock().unwrap() = voice;
  *state.tts.lock().unwrap() = tts;
  *state.language.lock().unwrap() = language;
  *state.baseurl.lock().unwrap() = baseurl;
  *state.provider.lock().unwrap() = provider;
  *state.model.lock().unwrap() = model;
  *state.system_prompt.lock().unwrap() = system_prompt;
  state.ptt.store(ptt, std::sync::atomic::Ordering::Relaxed);
  state
    .speed
    .store(speed, std::sync::atomic::Ordering::Relaxed);
}

pub struct SaveMetadata {
  pub start_date: String,
  pub agents: Vec<crate::config::AgentSettings>,
  pub is_debate: bool,
  pub system_prompt: String,
  pub voice: String,
}

pub fn save_conversation(
  history: &ConversationHistory,
  path: Option<&Path>,
  metadata: Option<&SaveMetadata>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
  let home = crate::util::get_user_home_path().ok_or("Unable to determine home directory")?;
  let conv_dir = home.join(".vtmate").join("conversations");

  if !conv_dir.exists() {
    fs::create_dir_all(&conv_dir)?;
  }

  let filepath = if let Some(p) = path {
    p.to_path_buf()
  } else {
    let now = Local::now();
    let date_str = now.format("%Y-%m-%d_%H-%M-%S").to_string();
    let uuid_str = &Uuid::new_v4().to_string()[..8];
    conv_dir.join(format!("{}_{}.txt", date_str, uuid_str))
  };

  let hist = history.lock().unwrap();
  let mut content = String::new();

  content.push_str(crate::ui::get_banner());
  content.push_str("\n\n");

  for msg in hist.iter() {
    let label = if msg.role == "user" {
      "USER"
    } else if msg.role == "assistant" {
      if metadata.map_or(false, |m| m.is_debate) {
        msg.agent_name.as_deref().unwrap_or("ASSISTANT")
      } else {
        "ASSISTANT"
      }
    } else {
      &msg.role
    };
    content.push_str(&format!("{}:\n{}\n\n", label, msg.content));
  }

  if let Some(meta) = metadata {
    content.push_str("\n\n___________________________________________________________________\n");
    content.push_str("\n");
    if meta.is_debate {
      content.push_str(" This was a conversation between ai agents\n\n");
      if meta.agents.len() >= 2 {
        let a1 = &meta.agents[0];
        let a2 = &meta.agents[1];
        content.push_str(&format!(
          " Participants:\t\t'{}' and '{}'\n\n",
          a1.name, a2.name
        ));
        content.push_str(" ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~\n\n");
        content.push_str(&format!("  Agent name:\t\t{}\n", a1.name));
        content.push_str(&format!("  Agent TTS:\t\t{}\n\n", a1.tts));
        content.push_str(&format!("  Agent model:\t\t{}\n", a1.model));
        content.push_str(&format!("  Agent voice:\t\t{}\n", a1.voice));
        content.push_str(&format!("  Agent system prompt:\t{}\n", a1.system_prompt));
        content.push_str(" ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~\n\n");
        content.push_str(&format!("  Agent name:\t\t{}\n", a2.name));
        content.push_str(&format!("  Agent TTS:\t\t{}\n", a2.tts));
        content.push_str(&format!("  Agent model:\t\t{}\n", a2.model));
        content.push_str(&format!("  Agent voice:\t\t{}\n", a2.voice));
        content.push_str(&format!("  Agent system prompt:\t{}\n", a2.system_prompt));
      }
    } else if let Some(agent) = meta.agents.first() {
      content.push_str(" This conversation was a conversation between a user and an ai agent\n\n");
      content.push_str(" ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~\n\n");
      content.push_str(&format!("  Agent name:\t\t{}\n", agent.name));
      content.push_str(&format!("  Agent TTS:\t\t{}\n", agent.tts));
      content.push_str(&format!("  Agent model:\t\t{}\n", agent.model));
      content.push_str(&format!("  Agent voice:\t\t{}\n", meta.voice));
      content.push_str(&format!("  Agent system prompt:\t{}\n", meta.system_prompt));
    }
    content.push_str("\n ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~\n\n");
    content.push_str(&format!("  * Date: {}\n", meta.start_date));
    content.push_str("  * Created with vtmate - www.github.com/DavidValin/vtmate\n");
  }

  fs::write(filepath, content)?;
  Ok(())
}
