// ------------------------------------------------------------------
//  HTML export  (--save-html / -s-html)
//
//  Writes a self-contained folder per conversation:
//
//    2026-09-09_12-00-00_ab12cd34/
//      index.html          the player: every turn, playable in order
//      turn-001-user.wav   one file per turn that produced audio
//      turn-002-nova.wav
//      ...
//
//  Turn audio is captured while it happens: the microphone chunk for a user
//  turn, and every chunk the playback thread pulls for an agent turn. The page
//  is re-rendered after each turn from the conversation history, so the folder
//  is valid (and playable) mid-conversation, not only once vtmate exits.
//
//  This is independent from `-s`, which writes a single .txt plus one .wav for
//  the whole session; both options can be used at the same time.
// ------------------------------------------------------------------

use crate::audio::AudioChunk;
use crate::conversation::{ChatMessage, SaveMetadata};
use crossbeam_channel::Sender;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};

// API
// ------------------------------------------------------------------

/// The turn currently being recorded into its own wav file.
struct OpenTurn {
  /// Index in the conversation history this audio belongs to.
  idx: usize,
  tx: Sender<AudioChunk>,
  /// No chunk ever arrived: `hound` never created the file, so the turn is
  /// rendered as a text-only turn instead of pointing at a missing wav.
  wrote: bool,
}

struct Export {
  dir: PathBuf,
  /// Last metadata rendered, so the page can be rewritten on exit without the
  /// conversation thread having to hand it over again.
  meta: Option<SaveMetadata>,
  /// Wav file name per conversation history index; `None` for a turn that has
  /// no audio (a typed prompt, or a reply that was only code).
  audio: Vec<Option<String>>,
  open: Option<OpenTurn>,
}

static EXPORT: OnceLock<Mutex<Option<Export>>> = OnceLock::new();

fn export() -> MutexGuard<'static, Option<Export>> {
  EXPORT
    .get_or_init(|| Mutex::new(None))
    .lock()
    .unwrap_or_else(|e| e.into_inner())
}

/// True once `init` has created the export folder for this conversation.
pub fn is_active() -> bool {
  export().is_some()
}

/// Create (or re-create) the export folder. Any previous export is closed.
pub fn init(dir: &Path) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
  std::fs::create_dir_all(dir)?;
  let mut slot = export();
  *slot = Some(Export {
    dir: dir.to_path_buf(),
    meta: None,
    audio: Vec::new(),
    open: None,
  });
  Ok(())
}

/// Folder name of the running export, so `-s` and `--save-html` can share it.
pub fn dir_name() -> Option<String> {
  export()
    .as_ref()
    .and_then(|e| e.dir.file_name())
    .map(|n| n.to_string_lossy().to_string())
}

/// Forget the current export: the next turn starts a new folder.
pub fn reset() {
  *export() = None;
}

/// Finish the turn still being recorded and rewrite the page with it. The last
/// turn of a session is only complete once vtmate is on its way out.
pub fn finish() {
  close_turn();
  let meta = export().as_ref().and_then(|e| e.meta.clone());
  let Some(meta) = meta else { return };
  let Some(state) = crate::state::GLOBAL_STATE.get() else {
    return;
  };
  // exiting must not block on a lock another thread is still holding
  let Ok(history) = state.conversation_history.try_lock() else {
    return;
  };
  let snapshot = history.clone();
  drop(history);
  render(&snapshot, &meta);
}

/// Start recording the audio of the message that occupies `idx` in the
/// conversation history. Closes the previous turn, which is what ends an agent
/// turn in the paths that do not wait for playback to drain.
pub fn open_turn(idx: usize, label: &str) {
  let mut slot = export();
  let Some(exp) = slot.as_mut() else { return };
  exp.close_open();
  let file = format!("turn-{:03}-{}.wav", idx + 1, slugify(label));
  let tx = crate::audio::init_wav_writer(&exp.dir.join(&file), 0);
  exp.set_audio(idx, Some(file));
  exp.open = Some(OpenTurn {
    idx,
    tx,
    wrote: false,
  });
}

/// Close the turn being recorded, finalizing its wav file.
pub fn close_turn() {
  let mut slot = export();
  if let Some(exp) = slot.as_mut() {
    exp.close_open();
  }
}

/// Feed audio to the turn being recorded. Called from the playback thread for
/// every chunk that reaches the speakers; a no-op when no turn is open.
pub fn push_audio(chunk: &AudioChunk) {
  let mut slot = export();
  let Some(exp) = slot.as_mut() else { return };
  let Some(open) = exp.open.as_mut() else {
    return;
  };
  if open.tx.send(chunk.clone()).is_ok() {
    open.wrote = true;
  }
}

/// Record a complete turn whose audio is already known (a microphone
/// utterance): open, write and close in one go.
pub fn record_audio_turn(idx: usize, label: &str, chunk: &AudioChunk) {
  open_turn(idx, label);
  push_audio(chunk);
  close_turn();
}

/// Write `index.html` from the current conversation history.
pub fn render(history: &[ChatMessage], meta: &SaveMetadata) {
  let mut slot = export();
  let Some(exp) = slot.as_mut() else { return };
  // History shrinks on undo: drop the audio of turns that no longer exist.
  exp.audio.truncate(history.len());
  exp.meta = Some(meta.clone());
  let page = build_page(history, meta, &exp.audio);
  let dir = exp.dir.clone();
  drop(slot);
  write_atomic(&dir.join("index.html"), &page);
}

// PRIVATE
// ------------------------------------------------------------------

impl Export {
  fn close_open(&mut self) {
    let Some(open) = self.open.take() else { return };
    if !open.wrote {
      // nothing was ever spoken for this turn: no file was created
      self.set_audio(open.idx, None);
    }
    // dropping the sender ends the writer thread, which finalizes the file
    drop(open.tx);
  }

  fn set_audio(&mut self, idx: usize, file: Option<String>) {
    if self.audio.len() <= idx {
      self.audio.resize(idx + 1, None);
    }
    self.audio[idx] = file;
  }
}

/// Replace the file in one step, so a render that happens while the page is
/// open never leaves a half-written document behind.
fn write_atomic(path: &Path, content: &str) {
  let tmp = path.with_extension("html.tmp");
  if std::fs::write(&tmp, content).is_ok() {
    let _ = std::fs::rename(&tmp, path);
  }
}

fn slugify(label: &str) -> String {
  let slug: String = label
    .chars()
    .map(|c| {
      if c.is_ascii_alphanumeric() {
        c.to_ascii_lowercase()
      } else {
        '-'
      }
    })
    .collect();
  let slug = slug.trim_matches('-').to_string();
  if slug.is_empty() {
    "turn".to_string()
  } else {
    slug.chars().take(24).collect()
  }
}

fn esc(s: &str) -> String {
  let mut out = String::with_capacity(s.len());
  for c in s.chars() {
    match c {
      '&' => out.push_str("&amp;"),
      '<' => out.push_str("&lt;"),
      '>' => out.push_str("&gt;"),
      '"' => out.push_str("&quot;"),
      '\'' => out.push_str("&#39;"),
      _ => out.push(c),
    }
  }
  out
}

/// A JavaScript string literal, safe to drop inside a `<script>` block.
fn js_str(s: &str) -> String {
  let mut out = String::with_capacity(s.len() + 2);
  out.push('"');
  for c in s.chars() {
    match c {
      '"' => out.push_str("\\\""),
      '\\' => out.push_str("\\\\"),
      '\n' => out.push_str("\\n"),
      '\r' => out.push_str("\\r"),
      '<' => out.push_str("\\u003c"),
      '\u{2028}' => out.push_str("\\u2028"),
      '\u{2029}' => out.push_str("\\u2029"),
      _ => out.push(c),
    }
  }
  out.push('"');
  out
}

/// Turn a reply into html: ``` fences become code blocks, `code` stays inline,
/// blank lines separate paragraphs. Everything is escaped first.
fn render_text(text: &str) -> String {
  let mut out = String::new();
  for (i, part) in text.split("```").enumerate() {
    if i % 2 == 1 {
      let body = part.strip_prefix('\n').unwrap_or(part);
      // a fenced block often starts with its language on the first line
      let body = match body.split_once('\n') {
        Some((first, rest)) if !first.trim().is_empty() && !first.contains(' ') => rest,
        _ => body,
      };
      out.push_str(&format!("<pre><code>{}</code></pre>", esc(body.trim_end())));
    } else {
      for para in part.split("\n\n") {
        if para.trim().is_empty() {
          continue;
        }
        let lines: Vec<String> = para
          .trim_matches('\n')
          .lines()
          .map(|l| inline_code(l.trim_end()))
          .collect();
        out.push_str(&format!("<p>{}</p>", lines.join("<br>")));
      }
    }
  }
  if out.is_empty() {
    out.push_str("<p class=\"empty\">…</p>");
  }
  out
}

fn inline_code(line: &str) -> String {
  line
    .split('`')
    .enumerate()
    .map(|(i, part)| {
      if i % 2 == 1 {
        format!("<code>{}</code>", esc(part))
      } else {
        esc(part)
      }
    })
    .collect()
}

/// How many speaker colours the stylesheet defines (`.turn.a0` … `.turn.a5`).
/// Each one is a css variable, so both themes get their own readable shade.
const ACCENT_COUNT: usize = 6;

fn speaker_label(msg: &ChatMessage) -> String {
  match msg.role.as_str() {
    "user" => "You".to_string(),
    "assistant" => msg
      .agent_name
      .clone()
      .filter(|n| !n.is_empty())
      .unwrap_or_else(|| "Assistant".to_string()),
    other => other.to_string(),
  }
}

fn build_page(history: &[ChatMessage], meta: &SaveMetadata, audio: &[Option<String>]) -> String {
  let mut names: Vec<String> = Vec::new();
  let mut turns_html = String::new();
  let mut turns_js = String::new();

  for (i, msg) in history.iter().enumerate() {
    let name = speaker_label(msg);
    // agents are coloured by order of appearance; the user keeps the neutral
    // accent the `.turn` rule already carries
    let accent = if msg.role == "user" {
      String::new()
    } else {
      let pos = names.iter().position(|n| n == &name).unwrap_or_else(|| {
        names.push(name.clone());
        names.len() - 1
      });
      format!(" a{}", pos % ACCENT_COUNT)
    };
    let file = audio.get(i).and_then(|f| f.clone());
    let has_audio = file.is_some();
    turns_html.push_str(&format!(
      concat!(
        "<article class=\"turn {role}{accent}\" id=\"t{i}\" data-i=\"{i}\">",
        "<div class=\"head\">",
        "<button class=\"play-turn\" data-i=\"{i}\" title=\"play from here\" aria-label=\"play from here\">▶</button>",
        "<span class=\"name\">{name}</span>",
        "{badge}",
        "</div>",
        "<div class=\"body\">{body}</div>",
        "<div class=\"bar\"><i></i></div>",
        "</article>"
      ),
      role = if msg.role == "user" { "user" } else { "agent" },
      i = i,
      accent = accent,
      name = esc(&name),
      badge = if has_audio {
        ""
      } else {
        "<span class=\"badge\">no audio</span>"
      },
      body = render_text(&msg.content),
    ));
    turns_js.push_str(&format!(
      "{{name:{},audio:{},chars:{}}},",
      js_str(&name),
      file.as_deref().map(js_str).unwrap_or("null".to_string()),
      msg.content.chars().count(),
    ));
  }

  let kind = if meta.is_debate {
    "debate"
  } else {
    "conversation"
  };
  let who = if meta.is_debate {
    let list: Vec<String> = meta.agents.iter().map(|a| esc(&a.name)).collect();
    format!("you · {}", list.join(" · "))
  } else {
    format!(
      "you · {}",
      meta
        .agents
        .first()
        .map(|a| esc(&a.name))
        .unwrap_or_default()
    )
  };

  let mut cards = String::new();
  for agent in &meta.agents {
    let voice = if meta.is_debate {
      agent.voice.clone()
    } else {
      meta.voice.clone()
    };
    let prompt = if meta.is_debate {
      agent.system_prompt.clone()
    } else {
      meta.system_prompt.clone()
    };
    cards.push_str(&format!(
      concat!(
        "<div class=\"card\"><h3>{name}</h3><dl>",
        "<dt>model</dt><dd>{model}</dd>",
        "<dt>tts</dt><dd>{tts}</dd>",
        "<dt>voice</dt><dd>{voice}</dd>",
        "<dt>system prompt</dt><dd class=\"prompt\">{prompt}</dd>",
        "</dl></div>"
      ),
      name = esc(&agent.name),
      model = esc(&agent.model),
      tts = esc(&agent.tts),
      voice = esc(&voice),
      prompt = esc(&prompt),
    ));
  }

  PAGE
    .replace(
      "__TITLE__",
      &esc(&format!("vtmate {} · {}", kind, meta.start_date)),
    )
    .replace("__KIND__", kind)
    .replace("__WHO__", &who)
    .replace("__DATE__", &esc(&meta.start_date))
    .replace("__TURNS__", &turns_html)
    .replace("__CARDS__", &cards)
    .replace("__TURNS_JS__", &turns_js)
}

const PAGE: &str = r##"<!doctype html>
<html lang="en" data-theme="light">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>__TITLE__</title>
<style>
:root{
  color-scheme:light;
  --bg:#f7f7f5; --panel:#ffffff; --line:#e2e0da; --fg:#1f2328; --dim:#6b7178;
  --code-bg:#f1f1ed; --shadow:rgba(31,35,40,.10);
  --user:#6e7781;
  --a0:#1a7f37; --a1:#0969da; --a2:#bc4c00; --a3:#8250df; --a4:#bf3989; --a5:#137775;
  --radius:10px;
  --mono:ui-monospace,SFMono-Regular,"SF Mono",Menlo,Consolas,"DejaVu Sans Mono","Liberation Mono",monospace;
}
:root[data-theme="dark"]{
  color-scheme:dark;
  --bg:#0d1117; --panel:#131922; --line:#252d38; --fg:#d7e0ea; --dim:#8b949e;
  --code-bg:#0a0e14; --shadow:rgba(0,0,0,.55);
  --user:#8b949e;
  --a0:#7ee787; --a1:#79c0ff; --a2:#ffa657; --a3:#d2a8ff; --a4:#f778ba; --a5:#56d4bc;
}
*{box-sizing:border-box}
html{scroll-behavior:smooth}
body{
  margin:0; background:var(--bg); color:var(--fg);
  font:16px/1.65 var(--mono);
  padding-bottom:40vh;
}
header{
  position:sticky; top:0; z-index:10;
  background:var(--bg); border-bottom:1px solid var(--line);
  display:flex; align-items:center; gap:14px; flex-wrap:wrap;
  padding:12px max(16px,calc(50vw - 420px));
}
.brand{font-weight:700; letter-spacing:.02em; white-space:nowrap}
.brand small{font-weight:400; color:var(--dim)}
.who{color:var(--dim); font-size:13px; flex:1; min-width:120px}
.controls{display:flex; align-items:center; gap:6px}
button,select{
  font:15px/1.4 var(--mono); color:var(--fg); background:var(--panel);
  border:1px solid var(--line); border-radius:var(--radius); padding:7px 10px;
  cursor:pointer; transition:border-color .15s, transform .1s;
}
button:hover,select:hover{border-color:var(--dim)}
button:active{transform:translateY(1px)}
button:disabled{opacity:.4; cursor:default}
#play{min-width:96px; font-weight:700}
#counter{color:var(--dim); font-size:14px; min-width:62px; text-align:center}
#theme{min-width:78px; text-align:left}
main{max-width:840px; margin:0 auto; padding:26px 16px 0}
.turn{
  --accent:var(--user);
  position:relative; margin:0 0 12px; padding:12px 14px 14px;
  background:var(--panel); border:1px solid var(--line);
  border-left:3px solid var(--accent); border-radius:var(--radius);
  transition:box-shadow .2s, border-color .2s;
}
.turn.a0{--accent:var(--a0)}
.turn.a1{--accent:var(--a1)}
.turn.a2{--accent:var(--a2)}
.turn.a3{--accent:var(--a3)}
.turn.a4{--accent:var(--a4)}
.turn.a5{--accent:var(--a5)}
.turn.user{background:transparent}
.turn.active{
  border-color:var(--accent);
  box-shadow:0 0 0 1px var(--accent), 0 10px 26px -18px var(--shadow);
}
.head{display:flex; align-items:center; gap:9px; margin-bottom:6px}
.name{color:var(--accent); font-weight:700; font-size:13px; letter-spacing:.08em; text-transform:uppercase}
.badge{color:var(--dim); font-size:12px; border:1px solid var(--line); border-radius:20px; padding:0 7px}
.play-turn{padding:2px 9px; border-radius:20px; font-size:12px; color:var(--dim)}
.turn.active .play-turn{color:var(--accent); border-color:var(--accent)}
.body p{margin:.35em 0; white-space:pre-wrap; overflow-wrap:anywhere}
.body p.empty{color:var(--dim)}
.body code{background:var(--code-bg); padding:1px 5px; border-radius:4px}
.body pre{
  background:var(--code-bg); border:1px solid var(--line);
  border-radius:var(--radius); padding:11px 13px; overflow-x:auto; margin:.5em 0;
}
.body pre code{background:none; padding:0}
.bar{height:2px; margin-top:11px; background:var(--line); border-radius:2px; overflow:hidden; opacity:0}
.turn.active .bar{opacity:1}
.bar i{display:block; height:100%; width:0; background:var(--accent)}
footer{max-width:840px; margin:34px auto 0; padding:20px 16px 60px; border-top:1px solid var(--line); color:var(--dim); font-size:13px}
.cards{display:flex; flex-wrap:wrap; gap:10px; margin:14px 0}
.card{flex:1 1 250px; background:var(--panel); border:1px solid var(--line); border-radius:var(--radius); padding:11px 13px}
.card h3{margin:0 0 8px; font-size:13px; letter-spacing:.08em; text-transform:uppercase; color:var(--fg)}
dl{display:grid; grid-template-columns:auto 1fr; gap:3px 12px; margin:0; font-size:13px}
dt{color:var(--dim)}
dd{margin:0; overflow-wrap:anywhere}
dd.prompt{white-space:pre-wrap; max-height:7em; overflow:auto}
a{color:inherit}
.hint{margin-top:9px}
kbd{border:1px solid var(--line); border-bottom-width:2px; border-radius:4px; padding:0 5px; font-size:12px}
</style>
</head>
<body>
<header>
  <div class="brand">vtmate <small>__KIND__</small></div>
  <div class="who">__WHO__ · __DATE__</div>
  <div class="controls">
    <button id="prev" title="previous turn" aria-label="previous turn">⏮</button>
    <button id="play">▶ play</button>
    <button id="next" title="next turn" aria-label="next turn">⏭</button>
    <span id="counter">— / —</span>
    <select id="rate" title="playback speed">
      <option value="0.75">0.75x</option>
      <option value="1" selected>1x</option>
      <option value="1.25">1.25x</option>
      <option value="1.5">1.5x</option>
      <option value="2">2x</option>
    </select>
    <button id="theme" title="light / dark theme">☀ light</button>
  </div>
</header>

<main id="turns">
__TURNS__
</main>

<footer>
  <div class="cards">__CARDS__</div>
  <div>recorded __DATE__ · exported by <a href="https://github.com/DavidValin/vtmate">vtmate</a></div>
  <div class="hint"><kbd>space</kbd> play / pause · <kbd>←</kbd> <kbd>→</kbd> previous / next turn</div>
</footer>

<script>
const TURNS = [__TURNS_JS__];
const els = Array.from(document.querySelectorAll('.turn'));
const playBtn = document.getElementById('play');
const counter = document.getElementById('counter');
const rateSel = document.getElementById('rate');
const themeBtn = document.getElementById('theme');
const root = document.documentElement;
const audio = new Audio();

let cur = -1;        // turn being played (or paused on)
let playing = false;
let timer = null;    // silent turns are held for a readable moment
let timerEnds = 0, timerLeft = 0;
let ticker = null;

// light unless this browser remembers otherwise; storage is unavailable on
// file:// in some browsers, so every access is guarded
function setTheme(theme){
  root.dataset.theme = theme;
  themeBtn.textContent = theme === 'dark' ? '☾ dark' : '☀ light';
  try { localStorage.setItem('vtmate-theme', theme); } catch (e) {}
}
let stored = null;
try { stored = localStorage.getItem('vtmate-theme'); } catch (e) {}
setTheme(stored === 'dark' ? 'dark' : 'light');
themeBtn.addEventListener('click', function(){
  setTheme(root.dataset.theme === 'dark' ? 'light' : 'dark');
});

// A turn with no audio still gets its moment on screen, roughly the time it
// takes to read it, so playing back a conversation never jumps over what was
// typed rather than spoken.
function holdMs(chars){ return Math.min(6000, Math.max(900, chars * 32)); }

function setCounter(){
  counter.textContent = (cur < 0 ? '—' : cur + 1) + ' / ' + TURNS.length;
}

function highlight(i){
  els.forEach(function(el){ el.classList.remove('active'); progress(el, 0); });
  const el = els[i];
  if (el){ el.classList.add('active'); el.scrollIntoView({behavior:'smooth', block:'center'}); }
  setCounter();
}

function progress(el, pct){
  const bar = el && el.querySelector('.bar i');
  if (bar) bar.style.width = (pct * 100) + '%';
}

function stopTicker(){ if (ticker){ clearInterval(ticker); ticker = null; } }
function clearTimer(){ if (timer){ clearTimeout(timer); timer = null; } }

function startTicker(total){
  stopTicker();
  ticker = setInterval(function(){
    const el = els[cur];
    if (!el) return;
    if (TURNS[cur] && TURNS[cur].audio){
      progress(el, audio.duration ? audio.currentTime / audio.duration : 0);
    } else {
      const left = Math.max(0, timerEnds - Date.now());
      progress(el, total ? 1 - left / total : 0);
    }
  }, 80);
}

function play(i){
  if (i < 0 || i >= TURNS.length){ stop(); return; }
  clearTimer();
  cur = i;
  playing = true;
  highlight(i);
  render();
  const turn = TURNS[i];
  if (turn.audio){
    audio.src = turn.audio;
    audio.playbackRate = parseFloat(rateSel.value);
    audio.currentTime = 0;
    audio.play().catch(function(){ next(); });
    startTicker(0);
  } else {
    const total = holdMs(turn.chars) / parseFloat(rateSel.value);
    timerLeft = total;
    timerEnds = Date.now() + total;
    timer = setTimeout(next, total);
    startTicker(total);
  }
}

function next(){ if (cur + 1 < TURNS.length) play(cur + 1); else stop(true); }

function stop(finished){
  playing = false;
  audio.pause();
  clearTimer();
  stopTicker();
  if (finished){ cur = -1; highlight(-1); }
  render();
}

function toggle(){
  if (playing){
    playing = false;
    if (TURNS[cur] && TURNS[cur].audio){
      audio.pause();
    } else {
      timerLeft = Math.max(0, timerEnds - Date.now());
      clearTimer();
    }
    stopTicker();
    render();
    return;
  }
  if (cur < 0 || cur >= TURNS.length){ play(0); return; }
  playing = true;
  if (TURNS[cur] && TURNS[cur].audio){
    audio.play().catch(function(){ next(); });
    startTicker(0);
  } else {
    timerEnds = Date.now() + timerLeft;
    timer = setTimeout(next, timerLeft);
    startTicker(holdMs(TURNS[cur].chars) / parseFloat(rateSel.value));
  }
  render();
}

function render(){
  playBtn.textContent = playing ? '⏸ pause' : (cur >= 0 ? '▶ resume' : '▶ play');
  setCounter();
}

audio.addEventListener('ended', next);
playBtn.addEventListener('click', toggle);
document.getElementById('next').addEventListener('click', function(){
  play(Math.min(TURNS.length - 1, cur + 1));
});
document.getElementById('prev').addEventListener('click', function(){
  play(Math.max(0, cur - 1));
});
rateSel.addEventListener('change', function(){
  audio.playbackRate = parseFloat(rateSel.value);
});
els.forEach(function(el){
  el.querySelector('.play-turn').addEventListener('click', function(){
    play(parseInt(el.dataset.i, 10));
  });
});
document.addEventListener('keydown', function(e){
  if (e.target.tagName === 'SELECT') return;
  if (e.code === 'Space'){ e.preventDefault(); toggle(); }
  else if (e.key === 'ArrowRight'){ e.preventDefault(); play(Math.min(TURNS.length - 1, cur + 1)); }
  else if (e.key === 'ArrowLeft'){ e.preventDefault(); play(Math.max(0, cur - 1)); }
});

if (!TURNS.length){ playBtn.disabled = true; }
setCounter();
</script>
</body>
</html>
"##;
