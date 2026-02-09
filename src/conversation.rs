// ------------------------------------------------------------------
//  Conversation
// ------------------------------------------------------------------

use crossbeam_channel::{select, Receiver, Sender};
use std::sync::{
  atomic::{AtomicU64, Ordering},
  Arc, Mutex,
};

// API
// ------------------------------------------------------------------

pub fn conversation_thread(
  voice: &str,
  rx_utt: Receiver<crate::audio::AudioChunk>,
  tx_audio_into_router: Sender<crate::audio::AudioChunk>,
  stop_all_rx: Receiver<()>,
  out_sample_rate: u32, // MUST match playback SR
  interrupt_counter: Arc<AtomicU64>,
  args: crate::config::Args,
  ui: crate::ui::UiState,
  status_line: Arc<Mutex<String>>,
  print_lock: Arc<Mutex<()>>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
  crate::log::log("info", &format!("Ollama model: {}", args.ollama_model));
  let mut conversation_history = String::new();

  loop {
    select! {
      recv(stop_all_rx) -> _ => break,
      recv(rx_utt) -> msg => {
        let Ok(utt) = msg else { break };
        let pcm: Vec<i16> = utt.data.iter().map(|s| ((*s).clamp(-1.0, 1.0) * (i16::MAX as f32)) as i16).collect();
        let user_text = crate::stt::whisper_transcribe(&pcm, utt.sample_rate, &args.resolved_whisper_model_path())?;
        let prompt = format!("{}\n{}: {}", conversation_history, crate::ui::USER_LABEL, user_text);
        let user_text = user_text.trim().to_string();
        if user_text.is_empty() {
          continue;
        }

        // Print user line (keep spinner/emojis only on the latest bottom line).
        crate::ui::ui_println(&print_lock, &status_line, "");
        crate::ui::ui_println(&print_lock, &status_line, &format!("{} {user_text}", crate::ui::USER_LABEL));
        conversation_history.push_str(&format!("{}: {}\n", crate::ui::USER_LABEL, user_text));
        ui.thinking.store(true, Ordering::Relaxed);

        // Snapshot interruption counter for this assistant turn.
        let my_interrupt = interrupt_counter.load(Ordering::SeqCst);
        let mut speaker = PhraseSpeaker::new();
        let mut got_any_token = false;

        crate::ui::ui_println(&print_lock, &status_line, "");
        crate::ui::ui_println(&print_lock, &status_line, crate::ui::ASSIST_LABEL);

        let mut interrupted = false;
        let mut interrupted_printed = false;

        // Print interruption banner exactly once per assistant turn.
        let mut print_user_interrupted = || {
          if interrupted_printed {
            return;
          }
          interrupted_printed = true;
          crate::ui::ui_println(&print_lock, &status_line, "");
          crate::ui::ui_println(&print_lock, &status_line, "🛑 USER interrupted");
          crate::ui::ui_println(&print_lock, &status_line, "");
        };

        let mut on_piece = |piece: &str| {
          if interrupted {
            return;
          }

          if stop_all_rx.try_recv().is_ok() {
            interrupted = true;
            return;
          }

          if interrupt_counter.load(Ordering::SeqCst) != my_interrupt {
            interrupted = true;
            return;
          }

          if !got_any_token && !piece.is_empty() {
            got_any_token = true;
            ui.thinking.store(false, Ordering::Relaxed);
          }

          if let Some(phrase) = speaker.push_text(piece) {
            crate::ui::ui_println(&print_lock, &status_line, &phrase);
            conversation_history.push_str(&format!("{}: {}\n", crate::ui::ASSIST_LABEL, phrase));

            let outcome = match crate::tts::speak(
              &strip_special_chars(&phrase),
              args.tts.as_str(),
              args.opentts_base_url.as_str(),
              args.language.as_str(),
              voice,
              out_sample_rate,
              tx_audio_into_router.clone(),
              stop_all_rx.clone(),
              interrupt_counter.clone(),
              my_interrupt,
            ) {
              Ok(o) => o,
              Err(e) => {
                crate::log::log("error", &format!("TTS error: {}", e));
                interrupted = true;
                return;
              }
            };

            if outcome == crate::tts::SpeakOutcome::Interrupted
              || (interrupt_counter.load(Ordering::SeqCst) != my_interrupt && ui.playing.load(Ordering::Relaxed))
            {
              interrupted = true;
              print_user_interrupted();
              std::thread::sleep(std::time::Duration::from_millis(500));
              *status_line.lock().unwrap() = "".to_string();
              return;
            }
          }
        };

        match crate::llm::ollama_stream_response_into(
          &prompt,
          args.ollama_url.as_str(),
          args.ollama_model.as_str(),
          stop_all_rx.clone(),
          interrupt_counter.clone(),
          my_interrupt,
          &mut on_piece,
        ) {
          Ok(()) => {},
          Err(e) => {
            crate::log::log("error", &format!("Ollama error: {}", e));
            // skip this turn and continue
            continue;
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
          crate::ui::ui_println(&print_lock, &status_line, &phrase);
          conversation_history.push_str(&format!("{}: {}\n", crate::ui::ASSIST_LABEL, phrase));
          let outcome = match crate::tts::speak(
            &strip_special_chars(&phrase),
            args.tts.as_str(),
            args.opentts_base_url.as_str(),
            args.language.as_str(),
            voice,
            out_sample_rate,
            tx_audio_into_router.clone(),
            stop_all_rx.clone(),
            interrupt_counter.clone(),
            my_interrupt,
          ) {
            Ok(o) => o,
            Err(e) => {
              crate::log::log("error", &format!("TTS error: {}", e));
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
    let trigger = self.buf.contains('\n')
      || self.buf.ends_with('.')
      || self.buf.ends_with('!')
      || self.buf.ends_with('?')
      || self.buf.len() >= 140;
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

fn strip_special_chars(s: &str) -> String {
  s.chars()
    .filter(|c| !['.', '\n', '~', '\r', '\t', '*', '&'].contains(c))
    .collect()
}
