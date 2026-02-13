// ------------------------------------------------------------------
//  Conversation
// ------------------------------------------------------------------

use crate::state::GLOBAL_STATE;
use std::sync::OnceLock;

static WHISPER_CTX: OnceLock<whisper_rs::WhisperContext> = OnceLock::new();

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

use crate::START_INSTANT;

fn print_conversation_line(print_lock: &Arc<Mutex<()>>, status_line: &Arc<Mutex<String>>, s: &str) {
  let state = GLOBAL_STATE.get().expect("AppState not initialized");
  if !state.conversation_paused.load(Ordering::Relaxed) {
    crate::ui::ui_println(print_lock, status_line, s);
  }
}

use crossbeam_channel::{Receiver, Sender, select};
use std::cell::Cell;
use std::sync::{
  Arc, Mutex,
  atomic::{AtomicU64, Ordering},
};

// API
// ------------------------------------------------------------------

pub fn conversation_thread(
  voice_state: Arc<Mutex<String>>,
  rx_utt: Receiver<crate::audio::AudioChunk>,
  tx_play: Sender<crate::audio::AudioChunk>,
  stop_all_rx: Receiver<()>,
  stop_all_tx: Sender<()>,
  out_sample_rate: u32, // MUST match playback SR
  interrupt_counter: Arc<AtomicU64>,
  model_path: String,
  args: crate::config::Args,
  ui: crate::state::UiState,
  status_line: Arc<Mutex<String>>,
  print_lock: Arc<Mutex<()>>,
  conversation_history: std::sync::Arc<std::sync::Mutex<String>>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
  let ctx = init_whisper_context(&model_path);
  crate::log::log("info", &format!("Ollama model: {}", args.ollama_model));

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
        let user_text = crate::stt::whisper_transcribe_with_ctx(&ctx, &mono_f32, utt.sample_rate, &args.language)?;
        crate::log::log("info", &format!("Transcribed: '{}'", user_text));
        let prompt = format!("{}\n{}: {}", conversation_history.lock().unwrap(), crate::ui::USER_LABEL, user_text);
        let cleaned_prompt = crate::util::strip_ansi(&prompt);
        let user_text = user_text.trim().to_string();
        let speech_end_ms = crate::util::SPEECH_END_AT.load(std::sync::atomic::Ordering::SeqCst);
        let mut first_phrase_logged = false;
        if user_text.is_empty() {
          crate::log::log("debug", "Transcription returned empty string");
          continue;
        }

        // Print user line (keep spinner/emojis only on the latest bottom line).
        let my_interrupt = interrupt_counter.load(Ordering::SeqCst);
        if interrupt_counter.load(Ordering::SeqCst) != my_interrupt {
          // signal playback to stop queued audio
          let _ = stop_all_tx.try_send(());
          conversation_history.lock().unwrap().clear();
          continue;
        }
        print_conversation_line(&print_lock, &status_line, "");
        print_conversation_line(&print_lock, &status_line, &format!("{} {user_text}", crate::ui::USER_LABEL));
        conversation_history.lock().unwrap().push_str(&format!("{}: {}\n", crate::ui::USER_LABEL, user_text));
        ui.thinking.store(true, Ordering::Relaxed);

        // Snapshot interruption counter for this assistant turn.

        let mut speaker = PhraseSpeaker::new();
        let mut got_any_token = false;

        print_conversation_line(&print_lock, &status_line, "");
        print_conversation_line(&print_lock, &status_line, crate::ui::ASSIST_LABEL);

        let mut interrupted = false;
        let mut interrupted_printed = false;

        // Print interruption banner exactly once per assistant turn.
        let mut print_user_interrupted = || {
          if interrupted_printed {
            return;
          }
          interrupted_printed = true;
          print_conversation_line(&print_lock, &status_line, "");
          print_conversation_line(&print_lock, &status_line, "🛑 USER interrupted");
          print_conversation_line(&print_lock, &status_line, "");
        };

        let stop_all_tx_clone = stop_all_tx.clone();
        let mut on_piece = |piece: &str| {
          if interrupted {
            let _ = stop_all_tx_clone.try_send(());
            return;
          }

          if stop_all_rx.try_recv().is_ok() {
            interrupted = true;
            speaker.buf.clear();
            return;
          }

          // Abort if user interrupted before this token
          if interrupt_counter.load(Ordering::SeqCst) != my_interrupt {
            interrupted = true;
            speaker.buf.clear();
            return;
          }

          if !got_any_token && !piece.is_empty() {
            got_any_token = true;
            ui.thinking.store(false, Ordering::Relaxed);
          }

          if let Some(phrase) = speaker.push_text(piece) {
            // Log time from utterance start to first phrase playback
            if !first_phrase_logged {
              let elapsed_ms = crate::util::now_ms(&START_INSTANT) - speech_end_ms;
              crate::log::log("info", &format!("Time from speech end to first phrase playback: {:.2?}", elapsed_ms));
              first_phrase_logged = true;
            }
            print_conversation_line(&print_lock, &status_line, &phrase);
            conversation_history.lock().unwrap().push_str(&format!("{}: {}\n", crate::ui::ASSIST_LABEL, phrase));

            let outcome = match crate::tts::speak(
              &strip_special_chars(&phrase),
              args.tts.as_str(),
              args.opentts_base_url.as_str(),
              args.language.as_str(),
              &voice_state.lock().unwrap().as_str(),
              out_sample_rate,
              tx_play.clone(),
              stop_all_rx.clone(),
              interrupt_counter.clone(),
              my_interrupt,
            ) {
              Ok(o) => o,
              Err(_e) => {
                crate::log::log("error", &format!("TTS error. Can't play audio speech. Make sure OpenTTS is running: docker run --rm -p 5500:5500 synesthesiam/opentts:all"));
                interrupted = true;
                return;
              }
            };

            if outcome == crate::tts::SpeakOutcome::Interrupted
              || (interrupt_counter.load(Ordering::SeqCst) != my_interrupt && ui.playing.load(Ordering::Relaxed))
            {
              interrupted = true;
              print_user_interrupted();
              // crate::ui::ui_clear_last_line(&print_lock);
              std::thread::sleep(std::time::Duration::from_millis(500));
              // *status_line.lock().unwrap() = "".to_string();
              return;
            }
          }
        };

        if interrupt_counter.load(Ordering::SeqCst) != my_interrupt {
          // interruption detected, skip remaining speech
          continue;
        }

        if args.llm == "llama-server" {
          match crate::llm::llama_server_stream_response_into(
            &cleaned_prompt,
            args.llama_server_url.as_str(),
            stop_all_rx.clone(),
            interrupt_counter.clone(),
            my_interrupt,
            &mut on_piece,
          ) {
            Ok(o) => o,
            Err(e) => {
              crate::log::log("error", &format!("llama server error: {e}. Make sure llama-server / llamafile is running"));
              // skip this turn and continue
              continue;
            }
          }
        } else {
          match crate::llm::ollama_stream_response_into(
            &cleaned_prompt,
            args.ollama_url.as_str(),
            args.ollama_model.as_str(),
            stop_all_rx.clone(),
            interrupt_counter.clone(),
            my_interrupt,
            &mut on_piece
          ) {
            Ok(o) => o,
            Err(e) => {
              crate::log::log("error", &format!("ollama error. {e}. Make sure ollama is running"));
              // skip this turn and continue
              continue;
            }
          }
        }

        if interrupt_counter.load(Ordering::SeqCst) != my_interrupt {
          // interruption detected, skip remaining speech
          continue;
        }

        ui.thinking.store(false, Ordering::Relaxed);

        // If the user spoke over playback, cancel the rest of the assistant turn.
        if interrupt_counter.load(Ordering::SeqCst) != my_interrupt {
          print_user_interrupted();
          continue;
        }

        if let Some(phrase) = speaker.flush() {
          print_conversation_line(&print_lock, &status_line, &phrase);
          conversation_history.lock().unwrap().push_str(&format!("{}: {}\n", crate::ui::ASSIST_LABEL, phrase));
          let outcome = match crate::tts::speak(
            &strip_special_chars(&phrase),
            args.tts.as_str(),
            args.opentts_base_url.as_str(),
            args.language.as_str(),
            &voice_state.lock().unwrap().as_str(),
            out_sample_rate,
            tx_play.clone(),
            stop_all_rx.clone(),
            interrupt_counter.clone(),
            my_interrupt,
          ) {
            Ok(o) => o,
            Err(_e) => {
              crate::log::log("error", &format!("TTS error. Can't play audio speech. Make sure OpenTTS is running: docker run --rm -p 5500:5500 synesthesiam/opentts:all"));
              // skip this turn and continue
              continue;
            }
          };

          if outcome == crate::tts::SpeakOutcome::Interrupted
            || interrupt_counter.load(Ordering::SeqCst) != my_interrupt
          {
            print_user_interrupted();
            continue;
          }
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
    if trigger { self.flush() } else { None }
  }
  fn flush(&mut self) -> Option<String> {
    let out = self.buf.trim().to_string();
    self.buf.clear();
    if out.is_empty() { None } else { Some(out) }
  }
}

thread_local! {
  static IN_CODE_BLOCK: Cell<bool> = Cell::new(false);
}

fn strip_special_chars(s: &str) -> String {
  let mut result = String::new();
  let parts: Vec<&str> = s.split("```").collect();
  let mut inside = IN_CODE_BLOCK.with(|c| c.get());
  for (i, part) in parts.iter().enumerate() {
    if !inside {
      result.extend(part.chars().filter(|c| {
        ![
          '.', '~', '*', '&', '-', ',', ';', ':', '(', ')', '[', ']', '{', '}', '"', '\'',
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
