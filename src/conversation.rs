// ------------------------------------------------------------------
//  Conversation
// ------------------------------------------------------------------

use crate::playback::set_wav_tx;
use crate::state::AppState;
use crate::state::GLOBAL_STATE;
use crate::util::terminate;
use chrono::Local;
use crossbeam_channel::{Receiver, Sender, select};
use hound;

use std::fs;
use std::path::Path;
use std::sync::{
  Arc, Mutex,
  atomic::{AtomicU64, Ordering},
};
use std::thread;
use std::time::Duration;
use tokio::runtime::Builder as TokioBuilder;
use uuid::Uuid;

// API
// ------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct ChatMessage {
  pub role: String,
  pub content: String,
  pub agent_name: Option<String>,
  /// Tool calls requested by an assistant turn, in OpenAI shape
  /// (`{id, type, function: {name, arguments}}`) with `arguments` as a parsed object.
  pub tool_calls: Option<Vec<serde_json::Value>>,
  /// For `role: "tool"` messages: id of the tool call this result answers.
  pub tool_call_id: Option<String>,
  /// For `role: "tool"` messages: name of the tool that produced the result.
  pub tool_name: Option<String>,
}

pub type ConversationHistory = std::sync::Arc<std::sync::Mutex<Vec<ChatMessage>>>;

/// Commands sent from keyboard to conversation thread
pub enum Command {
  Undo,
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
  let whisper = crate::stt::init(&model_path)?;

  // WAV writer thread: activated when -s option is used
  // WAV writer will be started lazily when the first save path is created.
  let mut wav_tx_opt: Option<crossbeam_channel::Sender<crate::audio::AudioChunk>> = None;

  crate::log::log("info", &format!("LLM model: {}", settings.model));

  let settings_clone = settings.clone();

  // Quiet mode: handled within the conversation mode path below.

  // Runtime to use for async debate responses
  let rt = TokioBuilder::new_current_thread()
    .enable_all()
    .build()
    .unwrap();

  // Track interruptions for debate mode
  let mut last_interrupt = interrupt_counter.load(Ordering::SeqCst);
  let mut debate_interrupted = false;
  let mut pending_user_msg: Option<String> = init_prompt;

  // Safety guard: quiet mode without init_prompt should not happen (validated in main.rs),
  // but if it does, exit cleanly instead of hanging forever.
  if quiet && pending_user_msg.is_none() {
    crate::log::log("info", "Quiet mode: no input to process. Exiting.");
    terminate(0);
  }
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
    let my_interrupt = interrupt_counter.load(Ordering::SeqCst);
    // Detect transition to debate mode
    let current_debate_enabled = state.debate_enabled.load(Ordering::SeqCst);
    if current_debate_enabled && !prev_debate_enabled {
      // Reset state for new debate: clear pending message and interrupt flag
      pending_user_msg = None;
      debate_interrupted = false;
      last_interrupt = interrupt_counter.load(Ordering::SeqCst);
    } else if !current_debate_enabled && prev_debate_enabled {
      // Debate just disabled: clear debate-specific state so it doesn't leak
      // into conversation mode processing
      pending_user_msg = None;
      debate_interrupted = false;
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

        // Check for user input or undo command with short timeout
        let mut got_undo = false;
        select! {
          recv(rx_utt) -> utt_result => {
            if let Ok(utt) = utt_result {
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
              if agent_count > 0 {
                let next_agent = &debate_agents[turn % agent_count];
                let _ = apply_agent_settings(state, next_agent);
              }

              let _pcm_f32: Vec<f32> = utt.data.clone();
              let mono_f32 = crate::audio::convert_to_mono(&utt);

              let user_text = whisper.transcribe(
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
          }
          recv(rx_cmd) -> cmd_result => {
            if let Ok(Command::Undo) = cmd_result {
              handle_undo(state, &tx_ui, &conversation_history, &interrupt_counter, &stop_play_tx, &settings);
              got_undo = true;
            }
          }
          default(std::time::Duration::from_millis(100)) => {}
        }

        if got_undo {
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
          let _reply_opt = react_loop(
            state,
            current_agent,
            &conversation_history,
            &tx_ui,
            &tts_tx,
            &tts_done_rx,
            &rt,
            &interrupt_counter,
            user_msg.clone(),
            &[],
          );
          // Wait for the current agent's audio to finish before next agent starts
          wait_for_playback(state, &interrupt_counter, my_interrupt);
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
        let reply = react_loop(
          state,
          &settings,
          &conversation_history,
          &tx_ui,
          &tts_tx,
          &tts_done_rx,
          &rt,
          &interrupt_counter,
          user_msg,
          &settings.tools,
        );
        // react_loop speaks phrases as they stream; wait for the queued audio to finish.
        if reply.as_deref().map_or(false, |t| !t.is_empty()) {
          wait_for_playback(state, &interrupt_counter, my_interrupt);
        }
        if quiet {
          crate::log::log("info", "Quiet mode playback finished. Exiting.");
          terminate(0);
        }
      }
    }
    let state = GLOBAL_STATE.get().expect("AppState not initialized");

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
        let user_text = whisper.transcribe(&mono_f32, utt.sample_rate, &state.language.lock().unwrap())?;
        crate::log::log("info", &format!("Transcribed: '{}'", user_text));
        let user_text = user_text.trim().to_string();

        if user_text.is_empty() {
          crate::log::log("debug", "Transcription returned empty string");
          continue;
        }

        let my_interrupt = interrupt_counter.load(Ordering::SeqCst);
        if interrupt_counter.load(Ordering::SeqCst) != my_interrupt {
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

        let ui_thinking_cloned_for_closure = ui.thinking.clone();

        // Single react_loop call handles both tools and no-tools cases
        let reply = react_loop(
          state,
          &settings,
          &conversation_history,
          &tx_ui,
          &tts_tx,
          &tts_done_rx,
          &rt,
          &interrupt_counter,
          user_text.clone(),
          &settings.tools,
        );
        // react_loop speaks phrases as they stream; wait for the queued audio to finish.
        if reply.as_deref().map_or(false, |t| !t.is_empty()) {
          wait_for_playback(state, &interrupt_counter, my_interrupt);
        }
        ui_thinking_cloned_for_closure.store(false, Ordering::Relaxed);
      }
    }
  }
  Ok(())
}

// PRIVATE
// ------------------------------------------------------------------

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
    let out = self.buf.trim_end().to_string();
    self.buf.clear();
    if out.is_empty() { None } else { Some(out) }
  }
}

fn remove_empty_placeholder(conversation_history: &ConversationHistory) {
  let mut hist = conversation_history.lock().unwrap();
  if let Some(last) = hist.last() {
    if last.role == "assistant" && last.content.is_empty() {
      hist.pop();
    }
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
      let needs_gap = !last.content.is_empty()
        && !last.content.ends_with(char::is_whitespace)
        && !new_piece.starts_with(char::is_whitespace);
      if needs_gap {
        last.content.push(' ');
      }
      last.content.push_str(new_piece);
      return;
    }
  }
  hist.push(ChatMessage {
    role: "assistant".to_string(),
    content: new_piece.to_string(),
    agent_name: Some(agent_name.to_string()),
    ..Default::default()
  });
}

fn react_loop(
  state: &AppState,
  settings: &crate::config::AgentSettings,
  conversation_history: &ConversationHistory,
  tx_ui: &Sender<String>,
  tts_tx: &Sender<(String, u64, String)>,
  tts_done_rx: &Receiver<()>,
  rt: &tokio::runtime::Runtime,
  interrupt_counter: &Arc<AtomicU64>,
  user_msg: String,
  available_tools: &[String],
) -> Option<String> {
  let system_prompt = settings.system_prompt.replace("\\n", "\n");
  let system_prompt = augment_system_prompt(system_prompt, available_tools);

  let assistant_name = settings.name.clone();
  let assistant_name_for_closure = assistant_name.clone();
  let my_interrupt = interrupt_counter.load(Ordering::SeqCst);
  let has_tools = !available_tools.is_empty();

  // react_messages carries full reAct context (tool calls + outputs) across iterations.
  // Only the final reply is pushed to conversation_history.
  // Snapshot the history BEFORE adding the assistant placeholder so the request
  // does not carry an empty assistant turn.
  let mut react_messages =
    create_full_context_messages(system_prompt, user_msg, conversation_history);

  // Pre-add assistant placeholder for label
  conversation_history.lock().unwrap().push(ChatMessage {
    role: "assistant".to_string(),
    content: "".to_string(),
    agent_name: Some(assistant_name.clone()),
    ..Default::default()
  });

  // Render assistant label once
  let label = format!("\x1b[48;5;22;37m{}:\x1b[0m", assistant_name);
  let _ = tx_ui.send("line|".to_string());
  let _ = tx_ui.send(format!("line|{}", label));

  let originals = apply_agent_settings(state, settings);

  let max_react_loop_iters = 20;
  let mut react_loop_count = 0;
  // Track the last reply text across iterations
  let mut last_reply = String::new();

  loop {
    react_loop_count += 1;
    if react_loop_count > max_react_loop_iters {
      crate::log::log(
        "warn",
        "react loop exceeded max iterations, using last text as final response",
      );
      // Prefer the last thing actually said; fall back to scanning react_messages.
      let final_reply = if !last_reply.is_empty() {
        last_reply.clone()
      } else {
        react_messages
          .iter()
          .rev()
          .find(|m| m.role == "assistant" && !m.content.is_empty())
          .map(|m| m.content.clone())
          .unwrap_or_else(|| {
            crate::log::log("error", "no text response found in react loop history");
            "Lo siento, no pude completar la solicitud tras varios intentos.".to_string()
          })
      };
      // last_reply was already displayed, spoken and pushed to conversation_history
      // when it was produced. Anything else still has to be shown and spoken.
      if last_reply.is_empty() {
        let _ = tx_ui.send(format!("line|{}", final_reply));
        process_tts_phrases(
          &final_reply,
          tts_tx,
          tts_done_rx,
          settings.voice.clone(),
          interrupt_counter,
          my_interrupt,
        );
        push_or_update_last_assistant(
          &conversation_history,
          &final_reply,
          &assistant_name_for_closure,
        );
      }
      perform_save(&conversation_history, settings);
      restore_agent_settings(state, originals);
      return Some(final_reply);
    }
    if interrupt_counter.load(Ordering::SeqCst) != my_interrupt {
      // Remove empty assistant placeholder if still empty
      remove_empty_placeholder(&conversation_history);
      restore_agent_settings(state, originals);
      perform_save(&conversation_history, settings);
      return Some("User interrupted the request.".to_string());
    }

    let mut tool_calls: Vec<serde_json::Value> = Vec::new();
    crate::log::log(
      "debug",
      &format!("react_loop: starting iteration {}", react_loop_count),
    );
    let speaker_arc = Arc::new(Mutex::new(PhraseSpeaker::new()));
    let reply_accum = Arc::new(Mutex::new(String::new()));
    // Separate accumulator for reasoning tokens (Gemma 4, DeepSeek, etc.)
    // Reasoning is displayed with a visual distinction but NOT spoken via TTS.
    let reasoning_accum = Arc::new(Mutex::new(String::new()));

    // Text content is always prose: tool calls arrive through `on_tool_call`, never as
    // content. So every completed phrase is displayed, spoken and persisted right away,
    // whether or not tools are enabled for this turn.
    let mut on_piece = {
      let speaker_arc = speaker_arc.clone();
      let reply_accum = reply_accum.clone();
      let tx_ui = tx_ui.clone();
      let tts_tx = tts_tx.clone();
      let tts_done_rx = tts_done_rx.clone();
      let voice = settings.voice.clone();
      let conversation_history = conversation_history.clone();
      let assistant_name = assistant_name.clone();
      let my_interrupt = my_interrupt;
      move |piece: &str| {
        if piece.is_empty() {
          return;
        }
        // Always accumulate reply text
        if let Ok(mut acc) = reply_accum.lock() {
          acc.push_str(piece);
        }
        let phrase = {
          let mut speaker = speaker_arc.lock().unwrap();
          speaker.push_text(piece)
        };
        if let Some(ref phrase) = phrase {
          speak_phrase(
            &tx_ui,
            &tts_tx,
            &tts_done_rx,
            &conversation_history,
            phrase,
            &assistant_name,
            my_interrupt,
            &voice,
          );
        }
      }
    };

    let mut on_tool_call = |tc: &serde_json::Value| {
      tool_calls.push(tc.clone());
    };

    // Reasoning callback: display with visual distinction, do NOT speak
    let mut on_reasoning_piece = {
      let reasoning_accum = reasoning_accum.clone();
      let tx_ui = tx_ui.clone();
      move |piece: &str| {
        if piece.is_empty() {
          return;
        }
        if let Ok(mut acc) = reasoning_accum.lock() {
          acc.push_str(piece);
        }
        // Display reasoning with grey styling
        let _ = tx_ui.send(format!("stream|\x1b[90m{}\x1b[0m", piece));
      }
    };

    crate::log::log(
      "debug",
      &format!(
        "react_loop: sending to LLM with tools: {:?}",
        available_tools
      ),
    );
    let stream_result = rt.block_on(crate::llm::llama_server_stream_response_into(
      &react_messages,
      &settings.baseurl,
      &settings.model,
      &settings.provider,
      interrupt_counter.clone(),
      my_interrupt,
      &mut on_piece,
      has_tools,
      available_tools,
      Some(&mut on_tool_call),
      Some(&mut on_reasoning_piece),
      has_tools,
    ));

    if let Err(e) = stream_result {
      crate::log::log("error", &format!("Streaming error: {}", e));
      let _ = tx_ui.send(format!("line|\x1b[31mError getting response: {}\x1b[0m", e));
      // Remove empty assistant placeholder if still empty
      remove_empty_placeholder(&conversation_history);
      restore_agent_settings(state, originals);
      perform_save(&conversation_history, settings);
      return Some(format!("Error getting response: {}", e));
    }

    // Flush remaining phrase (text already in reply_accum from raw pieces)
    if let Some(last_phrase) = speaker_arc.lock().unwrap().flush() {
      speak_phrase(
        tx_ui,
        tts_tx,
        tts_done_rx,
        conversation_history,
        &last_phrase,
        &assistant_name_for_closure,
        my_interrupt,
        &settings.voice,
      );
    }

    // Final reply text
    let reply = {
      let mut acc = reply_accum.lock().unwrap();
      let cloned = acc.clone();
      acc.clear();
      cloned
    };

    // Extract accumulated reasoning text
    let reasoning = {
      let mut acc = reasoning_accum.lock().unwrap();
      let cloned = acc.clone();
      acc.clear();
      cloned
    };

    crate::log::log(
      "debug",
      &format!(
        "react_loop: after stream - reply.len={}, reasoning.len={}, tool_calls.len={}",
        reply.len(),
        reasoning.len(),
        tool_calls.len()
      ),
    );

    // No tool calls: LLM produced reasoning text
    // ----------------------------------------------------------
    if tool_calls.is_empty() {
      if reply.is_empty() && reasoning.is_empty() {
        // LLM produced nothing - force it to give a final text response
        react_messages.push(ChatMessage {
          role: "user".to_string(),
          content: "Please provide your final response.".to_string(),
          agent_name: None,
          ..Default::default()
        });
        continue;
      }

      // Reply text was already displayed, spoken and pushed to history phrase by
      // phrase during streaming. Without tools the stream IS the final answer, and with
      // tools a text-only response ends the loop.
      if !reply.is_empty() || !has_tools {
        perform_save(&conversation_history, settings);
        restore_agent_settings(state, originals);
        return Some(reply);
      }

      // Tools available but the LLM only produced reasoning: keep it in context and
      // let the model continue.
      react_messages.push(ChatMessage {
        role: "assistant".to_string(),
        content: reasoning.clone(),
        agent_name: Some(assistant_name_for_closure.clone()),
        ..Default::default()
      });
      continue;
    }

    // Tool calls present. Any reply text was already displayed, spoken and persisted
    // during streaming; remember it as the last thing said.
    // ----------------------------------------------------------
    if !reasoning.is_empty() {
      let _ = tx_ui.send("line|".to_string());
    }
    if !reply.is_empty() {
      last_reply = reply.clone();
    }

    let calls: Vec<ToolCallSpec> = tool_calls
      .iter()
      .enumerate()
      .map(|(i, tc)| normalize_tool_call(tc, react_loop_count, i))
      .collect();

    // The assistant turn that requested the tools, with its tool_calls, so the provider
    // sees a proper tool exchange. Tool results must directly follow this message, so
    // reasoning is deliberately not inserted as a separate assistant message here.
    react_messages.push(ChatMessage {
      role: "assistant".to_string(),
      content: reply.clone(),
      agent_name: Some(assistant_name_for_closure.clone()),
      tool_calls: Some(calls.iter().map(ToolCallSpec::to_json).collect()),
      ..Default::default()
    });

    crate::log::log(
      "debug",
      &format!("react_loop: executing {} tool calls", calls.len()),
    );

    // Execute tool calls; each result goes back as a `tool` message tied to its call id.
    for call in &calls {
      if interrupt_counter.load(Ordering::SeqCst) != my_interrupt {
        crate::log::log("debug", "Interrupted during tool execution");
        remove_empty_placeholder(&conversation_history);
        restore_agent_settings(state, originals);
        perform_save(&conversation_history, settings);
        return Some("User interrupted the request.".to_string());
      }
      let args_str = serde_json::to_string(&call.arguments).unwrap_or_else(|_| "{}".to_string());
      let payload =
        serde_json::json!({ "name": call.name, "arguments": call.arguments }).to_string();
      // Log tool execution to UI
      let _ = tx_ui.send(format!(
        "line|\n\x1b[42m\x1b[30m {} \x1b[0m {}",
        call.name, args_str
      ));
      let result = crate::tools::handle_tool_call(&payload);
      // handle_tool_call always returns Ok, wrapping errors in a JSON failure payload
      let output =
        result.unwrap_or_else(|e: Box<dyn std::error::Error + Send + Sync>| e.to_string());
      let parsed: Option<serde_json::Value> = serde_json::from_str(&output).ok();
      let is_failure = parsed
        .as_ref()
        .and_then(|v| v.get("status").and_then(|s| s.as_str()))
        .map(|s| s == "failed")
        .unwrap_or(false);
      let content = if is_failure {
        let reasons = parsed
          .as_ref()
          .and_then(|v| v.get("reasons"))
          .and_then(|r| r.as_array())
          .map(|arr| {
            arr
              .iter()
              .filter_map(|r| r.as_str().map(|s| s.to_string()))
              .collect::<Vec<_>>()
              .join(", ")
          })
          .unwrap_or_default();
        // Display the tool failure in the UI
        let _ = tx_ui.send(format!("line|The tool `{}` failed: {}", call.name, reasons));
        let _ = tx_ui.send("line|".to_string());
        format!("Tool error: {}. Try a different approach.", reasons)
      } else {
        // Display the tool result in the UI
        let _ = tx_ui.send(format!("line|{}", output.trim()));
        let _ = tx_ui.send("line|".to_string());
        output
      };
      // Notify UI of tool call
      let _ = tx_ui.send("line|\n\x1b[32m".to_string());

      react_messages.push(ChatMessage {
        role: "tool".to_string(),
        content,
        agent_name: None,
        tool_call_id: Some(call.id.clone()),
        tool_name: Some(call.name.clone()),
        ..Default::default()
      });
    }

    // Loop: send updated messages back to LLM
  }
}

/// A tool call requested by the model, normalized to a single shape regardless of how
/// the provider reported it.
struct ToolCallSpec {
  id: String,
  name: String,
  /// Parsed argument object (never a JSON string).
  arguments: serde_json::Value,
}

impl ToolCallSpec {
  /// OpenAI-shaped tool call as stored on the assistant turn that requested it.
  fn to_json(&self) -> serde_json::Value {
    serde_json::json!({
      "id": self.id,
      "type": "function",
      "function": {
        "name": self.name,
        "arguments": self.arguments
      }
    })
  }
}

/// Normalize a raw tool call. OpenAI-style servers send `arguments` as a JSON string and
/// always provide an `id`; Ollama's native API sends an object and no id, so a stable id
/// is synthesized to pair the result message with the call.
fn normalize_tool_call(tc: &serde_json::Value, iteration: i32, index: usize) -> ToolCallSpec {
  let func = tc
    .get("function")
    .cloned()
    .unwrap_or(serde_json::Value::Null);
  let name = func
    .get("name")
    .and_then(|n| n.as_str())
    .unwrap_or("unknown")
    .to_string();
  let id = tc
    .get("id")
    .and_then(|v| v.as_str())
    .filter(|s| !s.is_empty())
    .map(str::to_string)
    .unwrap_or_else(|| format!("call_{}_{}", iteration, index));
  let args_value = func
    .get("arguments")
    .or_else(|| func.get("parameters"))
    .cloned()
    .unwrap_or(serde_json::Value::Null);
  let arguments = match args_value {
    serde_json::Value::String(s) => serde_json::from_str(&s).ok(),
    serde_json::Value::Object(o) => Some(serde_json::Value::Object(o)),
    _ => None,
  }
  .filter(|v| v.is_object())
  .unwrap_or_else(|| serde_json::json!({}));
  ToolCallSpec {
    id,
    name,
    arguments,
  }
}

/// Display a completed phrase, hand it to TTS immediately and persist it in history.
fn speak_phrase(
  tx_ui: &Sender<String>,
  tts_tx: &Sender<(String, u64, String)>,
  tts_done_rx: &Receiver<()>,
  conversation_history: &ConversationHistory,
  phrase: &str,
  assistant_name: &str,
  my_interrupt: u64,
  voice: &str,
) {
  let _ = tx_ui.send(format!("stream|{}", phrase));
  let _ = tx_ui.send("line|".to_string());
  let spoken = crate::util::strip_special_chars(phrase);
  if !spoken.trim().is_empty() {
    let _ = tts_tx.send((spoken, my_interrupt, voice.to_string()));
    let _ = tts_done_rx.recv();
  }
  push_or_update_last_assistant(conversation_history, phrase, assistant_name);
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
    ..Default::default()
  });
}

fn wait_for_playback(
  state: &crate::state::AppState,
  interrupt_counter: &Arc<AtomicU64>,
  my_interrupt: u64,
) {
  let playback_active = state.playback.playback_active.clone();
  // Wait until playback starts if it hasn't already. Give up after a grace period so a
  // turn that produced no audio (error, empty reply) cannot block forever.
  let started_waiting = std::time::Instant::now();
  while !playback_active.load(Ordering::SeqCst) {
    if interrupt_counter.load(Ordering::SeqCst) != my_interrupt {
      return;
    }
    if started_waiting.elapsed() > Duration::from_secs(3) {
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
    ..Default::default()
  });
  // history messages
  let hist = conversation_history.lock().unwrap();
  for m in hist.iter() {
    messages.push(m.clone());
  }
  // user message, unless the caller already appended it to the history
  let already_last = hist
    .last()
    .map_or(false, |m| m.role == "user" && m.content == user_msg);
  if !already_last {
    messages.push(ChatMessage {
      role: "user".to_string(),
      content: user_msg,
      agent_name: None,
      ..Default::default()
    });
  }
  messages
}

// Augment system prompt with tool instructions if tools are available
fn augment_system_prompt(mut system_prompt: String, available_tools: &[String]) -> String {
  if !available_tools.is_empty() {
    let current_date = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
    let cwd = std::env::current_dir()
      .unwrap_or_default()
      .to_string_lossy()
      .to_string();
    system_prompt.push_str("\n\nAvailable tools:\n");
    for tool in available_tools {
      match tool.as_str() {
        "read_file" => {
          system_prompt.push_str(
            "        - read_file: Read file contents, including multiple range reads at once\n",
          );
        }
        "apply_patch" => {
          system_prompt
            .push_str("        - apply_patch: applies a patch to a file using diff notation\n");
        }
        "bash_command" => {
          system_prompt
            .push_str("        - bash_command: Execute bash commands (ls, grep, find, etc.)\n");
        }
        "glob" => {
          system_prompt
            .push_str("        - glob: search for files using glob patterns like **/*.js or *\n");
        }
        "grep" => {
          system_prompt.push_str("        - grep: search matching content across files in a directory using full regex syntax\n");
        }
        "search" => {
          system_prompt.push_str("        - search: search online urls for a search term\n");
        }
        "web_fetch" => {
          system_prompt.push_str("        - web_fetch: get page links and content of a url\n");
        }
        _ => {}
      }
    }
    // Add dynamic HTTP request tools
    let http_defs = crate::tools::http_request::load_http_request_definitions();
    for def in http_defs {
      if available_tools.contains(&def.tool_definition.name) {
        system_prompt.push_str(&format!(
          "        - {}: {}\n",
          def.tool_definition.name, def.tool_definition.description
        ));
      }
    }
    system_prompt.push_str("\n");
    system_prompt.push_str("      Guidelines:\n");
    system_prompt.push_str("        - Use read_file to examine files instead of bash_command.\n");
    system_prompt.push_str("        - Use bash_comand for file operations like ls, rg, find... or to build a smart command for a complex task in one go\n");
    system_prompt.push_str(
      "        - Use bash_command for new files, complete rewrites or append to end of file\n",
    );
    system_prompt.push_str("        - Use apply_patch for precise file changes, unified diff patch should match exactly\n");
    system_prompt.push_str("        - When changing multiple separate locations in one file, use one apply_patch call with multiple entries instead of multiple apply_patch calls\n");
    system_prompt.push_str("        - Each apply_patch call uses the current file state, not old file states before applying changes to it. Do not emit overlapping or nested edits. Merge nearby changes into one apply_patch.\n");
    system_prompt.push_str("        - Keep apply_patch as small as possible while still being unique in the file. Do not pad with large unchanged regions.\n");
    system_prompt.push_str("        - Use write only for new files or complete rewrites.\n");
    system_prompt.push_str("        - Be concise in your responses\n");
    system_prompt.push_str("        - Show file paths clearly when working with files\n\n");
    system_prompt.push_str("        - If you need new information, use search to find results and then web_fetch to inspect the page content\n\n");
    system_prompt.push_str(&format!("        Current date: {}\n", current_date));
    system_prompt.push_str(&format!("        Current working directory: {}\n", cwd));
  }

  system_prompt
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
    content.push_str("\n\n##########################################\n");
    content.push_str("\n");
    if meta.is_debate {
      content.push_str(" This was a conversation between:\n");
      if meta.agents.len() >= 2 {
        let a1 = &meta.agents[0];
        let a2 = &meta.agents[1];
        content.push_str(&format!(" user, '{}' and '{}'\n\n", a1.name, a2.name));
        content.push_str("~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~\n");
        content.push_str(&format!("  Agent name:          {}\n", a1.name));
        content.push_str(&format!("  Agent TTS:           {}\n", a1.tts));
        content.push_str(&format!("  Agent model:         {}\n", a1.model));
        content.push_str(&format!("  Agent voice:         {}\n", a1.voice));
        content.push_str(&format!("  Agent system prompt: {}\n", a1.system_prompt));
        content.push_str("~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~\n");
        content.push_str(&format!("  Agent name:          {}\n", a2.name));
        content.push_str(&format!("  Agent TTS:           {}\n", a2.tts));
        content.push_str(&format!("  Agent model:         {}\n", a2.model));
        content.push_str(&format!("  Agent voice:         {}\n", a2.voice));
        content.push_str(&format!("  Agent system prompt: {}\n", a2.system_prompt));
      }
    } else if let Some(agent) = meta.agents.first() {
      content.push_str(" This conversation was a conversation between a user and an ai agent\n\n");
      content.push_str("~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~\n");
      content.push_str(&format!("  Agent name:            {}\n", agent.name));
      content.push_str(&format!("  Agent TTS:             {}\n", agent.tts));
      content.push_str(&format!("  Agent model:           {}\n", agent.model));
      content.push_str(&format!("  Agent voice:           {}\n", meta.voice));
      content.push_str(&format!(
        "  Agent system prompt:   {}\n",
        meta.system_prompt
      ));
    }
    content.push_str("~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~\n\n");
    content.push_str(&format!("  - Date: {}\n", meta.start_date));
    content.push_str("  - Created with vtmate - www.github.com/DavidValin/vtmate\n\n");
    content.push_str("##########################################\n");
  }

  fs::write(filepath, content)?;
  Ok(())
}
