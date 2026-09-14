## vtmate

![vtmate](banner.png)

The final AI voice conversational system all running in your terminal! vtmate is a Powerful terminal-based voice ai toolkit with many realistic voices, extremely low latency, 41 languages supported. Allows you to voice conversate with local ai models (or cloud based), pipe data and save into files.

Homepage [https://github.com/DavidValin/vtmate](https://github.com/DavidValin/vtmate)

### Quick installation
```
curl -fsSL https://raw.githubusercontent.com/DavidValin/vtmate/main/installer.sh | sh
```

The program self-contains all TTS models and voices and necessary files to recognize speech and speak with voice with no external installations ensuring maximum portability.

* [⬇️ Download](https://github.com/DavidValin/vtmate/releases) (⭐ MacOS ⭐ Linux and ⭐ Windows supported)
* [🤠 Quicksheet (PDF)](https://raw.githubusercontent.com/DavidValin/vtmate/refs/heads/main/docs/en/quicksheet.pdf) (🖨️ print ready for easy access)
* [🎥 Video Overview](https://www.youtube.com/watch?v=TfNcgVsR3oc)

### Video demonstration

<details>
<summary>(🇬🇧 English) Conversation mode demo</summary>

https://github.com/user-attachments/assets/b60b46a4-e8f6-4197-af1d-86cafd34a701

</details>
<details>
<summary>(🇬🇧 English) Debate mode demo</summary>

https://github.com/user-attachments/assets/1b0a8030-96c9-4c14-ad31-ae2bab1f9c73

</details>

<details>
<summary>(🇬🇧 English) Reading mode demo</summary>

https://github.com/user-attachments/assets/925186c2-8424-40aa-8a52-14fa6c7ee536

</details>

<details>
<summary>(🇬🇧 English) Background mode (--daemon)</summary>

https://github.com/user-attachments/assets/e295a496-d7cf-486e-a561-eea42277d8de

</details>

![vtmate screenshot](preview.png)

### Index

- [Features](#features)
- [How it works](#how-it-works)
- [LLM integration](#llm-integration)
- [TTS engine support](#tts-engine-support)
- [Installation](#installation)
- [Configure agents](#configure-agents)
  - [Reusable system prompts](#reusable-system-prompts)
- [How to use it](#how-to-use-it)
  - [Conversation mode](#conversation-mode)
  - [Debate mode](#debate-mode)
  - [Quiet mode](#quiet-mode)
  - [Daemon mode (global shortcuts)](#daemon-mode-global-shortcuts)
  - [Read mode (file to speech)](#read-mode-file-to-speech)
  - [Separate agents](#separate-agents)
  - [Custom voices](#custom-voices)
  - [Voice cloning](#voice-cloning)
  - [Model files](#model-files)
- [Language support](#language-support)
- [Acceleration support](#acceleration-support)
- [Build vtmate from source code](#build-vtmate-from-source-code)

## Features

- 📌 Continuous Voice chat (LIVE conversation) with voice interruption
- 🚀 AI agents debates (2 agents talking to each other; use can also participate in between)
- 📌 Realtime agent swap
- 📌 Mid interrupt response via keyboard
- 📌 Mid interrupt response via voice
- 📌 Reset session (fresh history)
- 📌 "Undo" last response (remove last response from history)
- 📌 Recording Pause / Resume via keyboard in LIVE conversation mode
- 📌 Push to Talk mode (PTT)
- 📌 Save conversation as audio and text
- 📌 Read a text file with voice, phrase by phrase, with keyboard navigation and pause/resume
- 📌 Read text with voice from STDIN, phrase by phrase, with keyboard navigation and pause/resume
- 📌 Save audio speech of a text file or STDIN content
- 📌 Load separate settings file with different agents
- 📌 Integrated `whisper` speech recognition system (no external intallation required)
- 📌 Integrated `kokoro TTS`, `supertonic2 TTS` and `supertonic3 TTS` systems (no external intallation required)
- 📌 Interface with `OpenTTS` system (requires external docker service)
- 📌 Source code in the replies (text inside ``` blocks) is shown on screen but never spoken
- 📌 Use any gguf model from huggingface.com (using llama-server), any ollama model, or a hosted provider (OpenAI, Anthropic, Google, Groq, Mistral, OpenRouter, DeepSeek, xAI)
- 📌 Run in background mode and chat with llm via voice, ask about selection, read selected text or turn your speech into text pasted into screen

* Background mode features can be used to assist your daily routine with ai powered voice responses while you use other apps, voice read your favourite books or articles, write emails via voice and even replace paid tools like Superwhisper

## How it works

![how it works](https://github.com/DavidValin/vtmate/raw/main/docs/en/diagrams/how-it-works.png)

```
- You start the program and start talking
- Once audio is detected (based on sound-threshold-peak option) it will start recording
- As soon as there is a time of silence (based on end_silence_ms option), it will transcribe the recorded audio using speech to text system (whisper). In ptt mode, this option is ignored, the program will wait for SPACE key to be released to submit the audio
- The transcribed text will be sent to the ai model
- The ai model will reply with text
- The text converted to audio using text to speech system
- You can interrupt the ai agent at any moment by start speaking, this will cause the response and audio to stop and you can continue talking.
- In debate mode, the agents reply to each other automatically, playing the audio in each turn
```

## LLM integration

Local servers (no api key needed):

- ✅ ollama (default, version 0.13 or newer)
- ✅ llama-server
- ✅ any OpenAI-compatible server such as LM Studio or vLLM (`provider = openai-compatible`)

Hosted providers (api key needed, set `api_key` in the agent or the provider's environment variable):

- ✅ openai, anthropic, google, groq, mistral, openrouter, deepseek, xai

You can run the models locally (by default) or remotely by configuring the base url of each agent.
Thinking / reasoning is disabled on local servers so replies start speaking right away.

## TTS engine support

- ✅ Kokoro (integrated)
- ✅ Supertonic 2 (integrated)
- ✅ Supertonic 3 (integrated)
- ✅ OpenTTS (requires external docker service)

## Installation

### 📌 1. **Install vtmate**

Single interactive network installer (works for fresh installs or upgrades):
```
curl -fsSL https://raw.githubusercontent.com/DavidValin/vtmate/main/installer.sh | sh
```

Or download a release by hand from `https://github.com/DavidValin/vtmate/releases`.

### 📌 2. **Install llm engine (needed for ai responses)**

Option A- ollama (the default)
- Install `https://ollama.com/download`.
- Pull the model you want to use with vtmate, for instance: `ollama pull llama3.2:3b`.

Option B- llama-server support.
- Install llama.cpp: `https://github.com/ggml-org/llama.cpp`.
- Download a gguf model: `https://huggingface.co/QuantFactory/Meta-Llama-3-8B-Instruct-GGUF/resolve/main/Meta-Llama-3-8B-Instruct.Q8_0.gguf?download=true`.

Option C- hosted provider (no local install).

vtmate works with all major cloud providers, both api and cli options. You can configure your agents by running vtmate and pressing `Control+s` or manually editing ~/.vtmate/agents file.

## Configure agents

vtmate allows you to configure as many agents as you want, each with its personality (model, voice and system prompt). Example:
```
             vmate
                │
       ┌────────┼────────┐
       ↓        ↓        ↓
   Scientist  Lawyer  Programmer
```

It comes with a predefined list of agents.

The quickest way is to press `Control+S` while vtmate is running: a popup opens with the list of your agents, and everything you change there is written to the settings file when you save it.

<img width="1058" height="556" alt="agents" src="https://github.com/user-attachments/assets/04a32f13-0de3-4cb8-afb5-d678dbe833c5" />

Agent settings live in ~/.vtmate/agents file, which you can edit manually too (`see vtmate --help`).

* By default all agents are set in `PTT` mode, you have to keep `SPACE` pressed to talk. If you want to use `LIVE` mode, make sure you adjust your microphone levels correctly and adjust `sound_threshold_peak` and `end_silence_ms` settings to your need
* Source code in an agent's reply is not spoken: anything wrapped in ``` fences is shown but skipped. Reading a file with `-r` does speak it, since the code is part of what you asked to have read.
* Voice mixing is supported for kokoro TTS system only, you can create a voice by mixing 2 kokoro voices by percentage. Example mixing 50% of bm_daniel and 50% of am_puck: set voice name to `bm_daniel.5+am_puck.5`

### Reusable system prompts

A long system prompt is easier to write and to share between agents in its own `[system_prompt]` section, in `~/.vtmate/agents`. The block has a `name` and then the prompt body fenced between two lines of three or more dashes, and agents pull it in with `@<name>`:

```
[system_prompt]
name = planner
---
You assist the user in the creation of a plan based on the user's goal.

When defining the plan follow these format standards:
  1. The plan is composed by tasks and subtasks.
  2. Each task has the format: "[ ] <task name>".
  3. Subtasks are indented with 2 spaces below the parent task.
  4. Before defining a plan, make sure you have the relevant
     information from the user.
---

[agent]
name = planner
...
system_prompt = @planner
```

* The body is taken exactly as written: blank lines, indentation, quotes and lines starting with `[` are all kept. Because it already has real new lines, `\n` inside a block is left alone.
* Close a body that itself contains a `---` line with a longer fence (`----`), the same way as markdown code fences.
* Define as many blocks as you want, in any order, and reference one from as many agents as you want.
* Inline prompts keep working exactly as before: `system_prompt = "You are a nice ai agent\nreply nicely"` turns `\n` into a new line. Start an inline prompt with `@@` if you need it to begin with a literal `@`.
* The `Control+S` popup picks between the two forms for you: a prompt of more than 5 lines is saved as a `[system_prompt]` block, a shorter one inline. A prompt that came from a block keeps that block's name, so agents sharing one go on sharing it.

## How to use it

Start vtmate and press SPACE while you talk and then release (PTT mode):
```
vtmate
```

See [Quicksheet (PDF)](https://raw.githubusercontent.com/DavidValin/vtmate/refs/heads/main/docs/en/quicksheet.pdf) to learn how to use it.

### Conversation mode

![conversation mode](https://github.com/DavidValin/vtmate/raw/main/docs/en/diagrams/conversation-mode.png)

Start conversation with default agent and save it as audio and text
(waits for user voice input and respond)

```
vtmate -s
```

or save it as html with playable turns
```
vtmate -s-html
```

This writes a folder per conversation in `~/.vtmate/conversations`:

```
2026-09-09_18-42-10_ab12cd34/
  index.html            the player: the whole conversation, turn by turn
  turn-001-user.wav     what you said on that turn
  turn-002-nova.wav     what the agent answered on that turn
  ...
```
Here is how it looks exported as html:

<img width="1488" height="849" alt="html-export" src="https://github.com/user-attachments/assets/4495788f-74ac-424c-9f72-f812efea0490" />

Open `index.html` in a browser and press play: it plays every turn in order,
highlights the one being spoken and scrolls to it. Playback can be paused and
resumed, each turn has its own play button to jump to it, and the page has a
light / dark theme switch. It is rewritten after every turn, so the folder can
be opened while the conversation is still going.

`-s` and `-s-html` are independent and can be combined: `-s` writes one `.txt`
plus a single `.wav` for the whole session, `-s-html` writes the folder above.

Start conversation with a specific agent
(waits for user voice input and respond)
```
vtmate -a "main agent"
```

Start conversation with an initial text prompt
```
vtmate -p "Are we alone in the galaxy?"
```

Start conversation with an initial prompt from file
```
vtmate -i myprompt.txt
```

Get a single response from STDIN text and exit
```
echo "How to fly without wings?" | vtmate -i -
```

* When running in LIVE mode just talk. You can also pause/resume recording by pressing `SPACE` once
* When running in PTT mode: keep `SPACE` pushed while talking, and then release
* Press `SCAPE` **once** during a mid response to cancel it
* Press `SCAPE` **twice** for resetting the session
* Press double `u` to undo last response
* You can switch agents in realtime by pressing `ARROW_LEFT` / `ARROW_RIGHT` keyword arrows (you need at least 2 agents defined in `~/vtmate/settings`).
* You can change the voice speed by pressing `ARROW_UP` / `ARROW_DOWN`
* Press `Control+S` to add, edit or remove agents without leaving the conversation (see [Configure agents](#configure-agents))
* Press `Control+E` to start (or stop) saving the running conversation without leaving it or restarting it: pick `.txt` + `.wav`, the html player, or both, from a popup. While a save is running, the same popup shows the folder it is writing to and a "Stop recording" button
* Be able to save the conversation in a wav and text file by adding `-s` option. It will save it in `~/.vtmate/conversations` folder
* Be able to save the debate as an html player with one audio file per turn by adding `-s-html` option. Each agent gets its own colour, and the whole debate can be played back from the browser
* Save the conversation / debate as .html with playable blocks using `--save-html`. It will save it in `~/.vtmate/conversations` folder
* For quick reference get the printable [Quicksheet (PDF)](https://raw.githubusercontent.com/DavidValin/vtmate/refs/heads/main/docs/en/quicksheet.pdf)

### Debate mode

![debate mode](https://github.com/DavidValin/vtmate/raw/main/docs/en/diagrams/debate-mode.png)

Initialize a debate between two agents and be able to participate in the debate by speaking at any time. To create a good debate adjust the system prompts of each agent and give a detailed initial input.

There are two ways to initialize a debate, using a cli command or from vtmate tui itself by pressing Control+D, which open the next popup:
<img width="1060" height="556" alt="new-debate" src="https://github.com/user-attachments/assets/400baa8d-0ae9-4303-b6a1-8476de52df1e" />

In debate mode is good idea to set `--ptt <true/false>` option so that the ptt value is not switched on each agent turn.

The debate's initial subject and `-p`/`-i`'s initial prompt are the same thing: whichever one you give becomes turn 0. Give only one - a trailing `<subject>` together with `-p`/`-i` is rejected, since they would both be trying to set the same message.

Start a debate with an initial subject (with forced ptt mode)
```
vtmate --debate "God" "Devil" "How to succeed in life?" --ptt true
```

Start a debate with an initial prompt from file (with forced live mode)
```
vtmate --debate "God" "Devil" -i myprompt.txt  --ptt false
```

Start a debate with an initial file prompt (with forced ptt mode)
```
cat "Lets discuss the permissions of this files: \n\n $(ls -la)" > prompt.txt
vtmate --debate "Unix administrator" "Security Expert" -i prompt.txt --ptt true
```

Start a debate that ends by itself after 10 turns and export the audio session as a playable html in ~/.vtmate/conversations
```
vtmate --debate "God" "Devil" "How to succeed in life?" --ptt true --max-turns 10 --s-html
```

* When running in LIVE mode just talk. You can also pause/resume recording by pressing `SPACE` once
* When running in PTT mode: keep `SPACE` pushed while talking, and then release
* Press `SCAPE` **once** during a mid response to cancel it and stop the debate
* Press `SCAPE` **twice** for resetting the session
* Press double `u` to undo last response
* You can also start/stop a debate from conversation mode by pressing `Control+D` and picking the debate agents. The popup also has a text field for the initial subject (the same role as the trailing `<subject>` argument above) - leave it blank to provide the topic by voice instead.
* Be able to save the conversation in a wav and text file by adding `-s` option. It will save it in `~/.vtmate/conversations` folder
* Add `--max-turns <N>` to end the program by itself after N turns, where one agent reply is one turn. The debate stops after that reply is spoken and saved, so nothing is cut mid sentence. The `Control+D` popup has its own "Max turns" field too (blank for no limit, or 2-1000000000000, prefilled with whatever limit is already in effect) - but reaching it there switches back to conversation mode instead of exiting, so you can keep going and start a new debate with `Control+D` right away
* [Here is an example](https://gist.github.com/DavidValin/58cf130c4f7b2ea9a6a033bf37bc1cda) on how to create automated audio debates from youtube videos using vtmate in combination with other tools
* For quick reference get the printable [Quicksheet (PDF)](https://raw.githubusercontent.com/DavidValin/vtmate/refs/heads/main/docs/en/quicksheet.pdf)


### Quiet mode

This mode process a text input, responds (text and audio) and exits

Get a single response from prompt
```
vtmate -q -p "Explain me the Zettelkasten Method"
```

Get a single response from prompt from file
```
vtmate -q -i myprompt.txt
```

Get a single response from prompt from STDIN and exit
```
echo "Is $(date) a national holiday day in Spain?" | vtmate -q -i -
```

Get a single response and save it as audio file and text file
```
echo "Can you find any suspicious processes in the next list? If so, why?\n\n $(ps aux | head -20)" | vtmate -q -i - -s
```

### Daemon mode (global shortcuts)

vtmate can run in the background with no terminal, driven by global shortcuts from any application: select some text in your browser or editor, hold a shortcut, talk, release it. Replies are spoken only.

There are 4 features you can use in daemon mode:

* Talk with an agent via voice (and reset the conversation context)
* Ask a question to an agent regarding the selected text and get a voice response
* Read a selected text using voice
* Transform a voice recording into text and paste it as text

Here is how to use it:

```
vtmate --daemon          # start it (models load once, then it waits for shortcuts)
vtmate                   # attach: the normal terminal view of the daemon conversation
vtmate --daemon-status   # is it running? which shortcuts?
vtmate --daemon-stop     # stop it
```

Shortcuts (change them in the `[daemon]` section of `~/.vtmate/settings`):

| setting | default | what it does |
|---|---|---|
| `llm_background_ptt_combo` | `ctrl+alt+a` | hold to talk. On release your speech is transcribed and, if some text is selected anywhere on the desktop, the selection is appended after the speech (speech first, blank line, selection). The whole thing is sent to the agent as one message and the reply is spoken. Pressing it while a reply is playing interrupts the reply. The selection is sent once: what you selected since your previous message. Select the text again to send it a second time, and nothing is appended when nothing is selected. |
| `tts_background_combo` | `ctrl+alt+r` | read the selected text aloud (no LLM), including any code in it. Press again while it is speaking to stop. Reading uses the selection up, so it is not appended to your next message as well. |
| `stt_and_paste_background_ptt_combo` | `ctrl+alt+s` | hold to talk. On release your speech is transcribed and written at the cursor of the application you are in. On Linux it is typed out, so it works in terminals too (where `Ctrl+V` is not the paste shortcut) and your clipboard is left alone; on Windows and macOS it is pasted through the clipboard, whose previous text is put back afterwards. No LLM, nothing spoken. |
| `llm_background_reset` | `ctrl+q` | like `ESCAPE` in the terminal: press once to stop the speech, twice within a second to also reset the conversation (history cleared). A desktop notification "Conversation restarted!" confirms the reset. |

Shortcuts are written as modifiers joined by `+`: `ctrl`, `alt` (or `option`), `shift`, `cmd` (or `super`), `cmdorctrl`, plus a key: letters, digits, `f1`..`f12`, `escape`, `space`, `tab`, arrows... e.g. `ctrl+alt+a`, `shift+f5`, `cmd+alt+r`.

* When starting, the daemon grabs all four shortcuts. If any is already taken by another application (some desktops bind `ctrl+q` or `ctrl+alt` combinations, for example) the daemon does not start and `vtmate --daemon` lists the taken shortcuts so you can change them.
* The agent that replies is the daemon's selected agent: run `vtmate` to attach, press `ARROW_LEFT` / `ARROW_RIGHT` to switch (this is remembered in `selected_agent`), then `Ctrl+C` to detach. Attaching with `vtmate -a <agent_name>` switches straight to that agent instead (same effect as arrowing to it - conversation reset included); `vtmate -c <agents_file>` reloads the daemon's agents from that file live (picking up edits to the running agent without resetting the conversation, unless it switches to a different agent because the previous one no longer exists in the file), and both can be combined. Attached you get the full terminal view: the live transcript, the status bar and the usual keys (`SPACE` push-to-talk, `ESCAPE`, `u`, arrows, `Ctrl+D`, `Ctrl+S`, `Ctrl+E`). `Ctrl+C` only detaches; the daemon keeps running until `vtmate --daemon-stop`.
* Only one daemon runs at a time. Its files live in `~/.vtmate`: `daemon.pid`, `daemon.sock` (Linux/macOS) and `daemon.log` (diagnostics only, never the conversation).
* The daemon always works in push-to-talk mode: the microphone is only open while a shortcut is held.

Platform notes:

* Linux: X11 only (Wayland has no global shortcuts nor a readable selection; under Wayland run vtmate in an X11 session). The selection is the primary selection (whatever is highlighted), no `Ctrl+C` needed.
* Windows / macOS: the selection is read by simulating `Ctrl+C` / `Cmd+C` and the clipboard is restored afterwards (text only). On macOS the vtmate binary needs the Accessibility permission (System Settings → Privacy & Security → Accessibility) to simulate keys. On Windows, `ctrl+alt` is the same as `AltGr` on some keyboard layouts: rebind those if the daemon reports them as taken.
* `vtmate --daemon` detaches from the terminal. To start it at login use your session autostart, a systemd user unit, a launchd agent or the Task Scheduler running `vtmate --daemon`.

###  Read mode (file to speech)

![read file mode](https://github.com/DavidValin/vtmate/raw/main/docs/en/diagrams/reading-mode.png)

Read a text file or STDIN text phrase by phrase using an agent voice. Ensure the agent you choose has correct language and voice for your text.
In this mode, only the next agent settings are used: "tts", "voice" and "language".

read from a txt file (and save it in `~/.vtmate/read-files`)
```
vtmate -r myfile.txt -a reader
```

read from STDIN text, get a response and exit
```
echo "First phrase. Second phrase" | vtmate -r -
```

In this mode you can:

* Move to previous phrase by pressing `ARROW_UP`
* Move to next phrase by pressing `ARROW_DOWN`
* Stop / Resume playback by pressing `SPACE`
* For quick reference get the printable [Quicksheet (PDF)](https://raw.githubusercontent.com/DavidValin/vtmate/refs/heads/main/docs/en/quicksheet.pdf)

`-r-stdout` is the headless, pipeable version: same text splitting and voice, but no on-screen text/navigation and no audio device - the synthesized speech streams to STDOUT as a wav instead of playing out loud, so it can be redirected to a file or piped into another program:
```
vtmate -r-stdout myfile.txt -a reader > myfile.wav
echo "First phrase. Second phrase" | vtmate -r-stdout - | aplay -
```

###  Separate agents

By default vtmate uses the `~/.vtmate/agents` file.
You can create separate agents files for different agent groups, each holding its own `[agent]` (and `[system_prompt]`) sections, example:

```
philosophers.txt
scientists.txt
employees.txt
```

And then load each as you need with `-c`:
```
vtmate -c philosophers.txt --debate "Aristoteles" "Ptahhotep" "how to achieve harmony?"
```

###  Custom voices

`supertonic3` and `supertonic2` read their voices from one JSON file per voice:

```
~/.vtmate/tts/supertonic3-model/voice_styles/M1.json
~/.vtmate/tts/supertonic2-model/voice_styles/F3.json
```

(on Windows `%USERPROFILE%\.vtmate\...`, on macOS `~/.vtmate/...` as well)

Drop a new `<name>.json` in that directory and the voice becomes available under that name: `vtmate --list-voices` shows it, `voice = <name>` in an agent passes validation, and the agent speaks with it. Remove the file and it is gone again. `--list-voices` prints the exact directory for each engine.

The other engines (`kokoro`, `opentts`) have fixed voice lists.

###  Voice cloning

Clone a new `supertonic3` voice from a short recording, or refine an existing clone further with another one - both run fully offline, no model training knowledge needed.

**Clone a new voice** from a WAV reference and its transcript:

```
vtmate --clone-voice <voice_name> <language> <wav_file> <ref_text>
```

* `voice_name`: letters, digits and `_` only, and must not already exist.
* `language`: one of the languages `supertonic3` supports (see `vtmate --list-voices`) - only used to train the clone (matches `ref_text` against it, picks built-in probe sentences for `en`/`es`/`fr`/`de`/`it`/`pt`); it does not lock the resulting voice to that language.
* `wav_file`: the reference recording (mono or stereo WAV, ~2-30s, any common sample rate).
* `ref_text`: the exact words spoken in the recording, quoted - the closer the match, the better the clone. It also sets the tempo: the clone is fitted to say this text in the time the recording takes, so no speed has to be given.

On success it prints `Voice "<voice_name>" ready in supertonic3!` and saves it to `~/.vtmate/tts/supertonic3-model/voice_styles/<voice_name>.json`, immediately usable like any other voice: `voice = <voice_name>` in an agent, or `--voice <voice_name>` elsewhere. Like the built-in `M1`-`F5` voices, a cloned voice is multilingual - one file, usable with any of the 31 supported languages regardless of which language it was cloned with.

**Refine an existing voice** further with another recording, without touching the original:

```
vtmate --refine-voice <voice_name> <language> <wav_file> <ref_text>
```

Same arguments, but `voice_name` must already exist (from a previous `--clone-voice`). It warm-starts from that voice's current style and never overwrites it: the result is saved as a new version, `<voice_name>v<n>` (`v1`, `v2`, `v3`, ...) - the base voice and every earlier version stay untouched and usable. Run it again on the same name to keep improving it: each call picks up from the latest version and produces the next one.

Both commands show a progress popup (current stage, iteration and overall progress) while training, which typically takes a few minutes on CPU.

**What to expect from a clone**

Cloning does not train a model on the speaker: it searches for the `supertonic3` style vector that best matches the recording, guided mainly by a speaker-embedding similarity. That sets what a clone can and cannot pick up:

* **Length**: 10-20 s of one or two natural sentences is the sweet spot. Speaker embeddings saturate at around 5-10 s of clean speech, and the transcript is synthesized as a single utterance, so windows over ~15 s trigger a warning. A longer clip adds little; a cleaner one adds a lot.
* **Quality over quantity**: a quiet room, no music or reverb, a single speaker, and a `ref_text` that matches the audio word for word matter more than extra seconds.
* **Timbre, pitch range and rhythm** transfer well - this is what the search fits.
* **Tempo is detected, not configured**: the speech in the recording is measured and the clone is fitted to speak `ref_text` at that pace (which is why `ref_text` is needed). That pace becomes the voice's `voice_speed = 1.0`; the setting and `ARROW_UP` / `ARROW_DOWN` scale from there.
* **Accent** transfers only partly. `supertonic3` reads raw text with no phoneme layer, so pronunciation (vowel quality, `r`, `th`, ...) comes from the model's own rendering of each language and cannot be changed by the style. The speaker's melody and pacing carry over; their individual sounds do not. Clone from a recording in the language you will mostly synthesize, and the prosody will fit that language best.
* **Expressive range** comes from coverage, not length: one clip pins down one delivery. Use `--refine-voice` with a different sentence (a question, an emphatic line) to widen it.

###  Model files

vtmate self contains (no need for manual installation) the whisper tiny & small models, kokoro model and voices, supertonic2 model and voices and supertonic3 (Supertonic 3) model and voices which will be autoextracted from the binary when running vtmate if they are not found in next locations:

whisper models:
```
- `~/.whisper-models/ggml-tiny.bin`
- `~/.whisper-models/ggml-small-q5_1.bin`
```

kokoro model files:
```
~/.cache/k/0.onnx
~/.cache/k/0.bin
```

supertonic2 files:
```
~/.vtmate/tts/supertonic2-model/onnx/duration_predictor.onnx
~/.vtmate/tts/supertonic2-model/onnx/text_encoder.onnx
~/.vtmate/tts/supertonic2-model/onnx/tts.json
~/.vtmate/tts/supertonic2-model/onnx/unicode_indexer.json
~/.vtmate/tts/supertonic2-model/onnx/vector_estimator.onnx
~/.vtmate/tts/supertonic2-model/onnx/vocoder.onnx
~/.vtmate/tts/supertonic2-model/voice_styles/M1.json
~/.vtmate/tts/supertonic2-model/voice_styles/M2.json
~/.vtmate/tts/supertonic2-model/voice_styles/M3.json
~/.vtmate/tts/supertonic2-model/voice_styles/M4.json
~/.vtmate/tts/supertonic2-model/voice_styles/M5.json
~/.vtmate/tts/supertonic2-model/voice_styles/F1.json
~/.vtmate/tts/supertonic2-model/voice_styles/F2.json
~/.vtmate/tts/supertonic2-model/voice_styles/F3.json
~/.vtmate/tts/supertonic2-model/voice_styles/F4.json
~/.vtmate/tts/supertonic2-model/voice_styles/F5.json
```

supertonic3 files (Supertonic 3, https://huggingface.co/Supertone/supertonic-3):
```
~/.vtmate/tts/supertonic3-model/onnx/duration_predictor.onnx
~/.vtmate/tts/supertonic3-model/onnx/text_encoder.onnx
~/.vtmate/tts/supertonic3-model/onnx/tts.json
~/.vtmate/tts/supertonic3-model/onnx/unicode_indexer.json
~/.vtmate/tts/supertonic3-model/onnx/vector_estimator.onnx
~/.vtmate/tts/supertonic3-model/onnx/vocoder.onnx
~/.vtmate/tts/supertonic3-model/voice_styles/M1.json
~/.vtmate/tts/supertonic3-model/voice_styles/M2.json
~/.vtmate/tts/supertonic3-model/voice_styles/M3.json
~/.vtmate/tts/supertonic3-model/voice_styles/M4.json
~/.vtmate/tts/supertonic3-model/voice_styles/M5.json
~/.vtmate/tts/supertonic3-model/voice_styles/F1.json
~/.vtmate/tts/supertonic3-model/voice_styles/F2.json
~/.vtmate/tts/supertonic3-model/voice_styles/F3.json
~/.vtmate/tts/supertonic3-model/voice_styles/F4.json
~/.vtmate/tts/supertonic3-model/voice_styles/F5.json
```

* If you want to avoid sound interruptions you can use `ptt` mode or increase the `sound_threshold_peak` for your microphone levels.
* If you want to use OpenTTS, start the docker service first: `docker run --rm --platform=linux/amd64 -p 5500:5500 synesthesiam/opentts:all` (it will pull the image the first time). Adjust the platform as needed depending on your hardware.
* If you have problems starting vtmate you can remove `~/vtmate/settings` so it recreates the default configuration
* By default whisper tiny is used (`~/.whisper-models/ggml-tiny.bin`). For better speech recognition point the `whisper_model_path` setting to the bundled 5-bit quantized small model `~/.whisper-models/ggml-small-q5_1.bin`, or download a bigger whisper model and point to it.

If you need help:

```
vtmate --help
```

## Language support

Engines: **ST2** Supertonic 2 (5 languages, 10 voices: M1-M5, F1-F5), **ST3** Supertonic 3 (31 languages, its own 10 voices: M1-M5, F1-F5, usable in every one of its languages), **KK** Kokoro (8 languages), **OpenTTS** (external docker service). Total languages: 41.

| ID |           Language       |      Support       |        TTS supported                          |   Number of voices  |
|----|--------------------------|--------------------|-----------------------------------------------------------|-------------|
| en |   🇬🇧  English            |  🏆 Best support   |    ✅ ST2    ✅ ST3    ✅ KK    ✅ OpenTTS     | > 48 voices
| es |   🇪🇸  Spanish            |  🏆 Best support   |    ✅ ST2    ✅ ST3    ✅ KK    ✅ OpenTTS     | > 24 voices
| fr |   🇫🇷  French             |  🏆 Best support   |    ✅ ST2    ✅ ST3    ✅ KK    ✅ OpenTTS     | > 22 voices
| ja |   🇯🇵  Japanese           |  🏆 Best support   |    ❌ ST2    ✅ ST3    ✅ KK    ✅ OpenTTS     | > 16 voices
| pt |   🇵🇹  Portuguese         |  🏆 Best support   |    ✅ ST2    ✅ ST3    ✅ KK    ❌ OpenTTS     | > 23 voices
| ko |   🇰🇷  Korean             |  🏆 Best support   |    ✅ ST2    ✅ ST3    ❌ KK    ✅ OpenTTS     | 21 voices
| it |   🇮🇹  Italian            |  🏆 Best support   |    ❌ ST2    ✅ ST3    ✅ KK    ✅ OpenTTS     | > 13 voices
| hi |   🇮🇳  Hindi              |  🏆 Best support   |    ❌ ST2    ✅ ST3    ✅ KK    ✅ OpenTTS     | > 14 voices
| zh |   🇨🇳  Mandarin Chinese   |  🥈 Good support   |    ❌ ST2    ❌ ST3    ✅ KK    ✅ OpenTTS     | > 9 voices
| ar |   🇸🇦  Arabic             |  🥈 Good support   |    ❌ ST2    ✅ ST3    ❌ KK    ✅ OpenTTS     | 11 voices
| cs |   🇨🇿  Czech              |  🥈 Good support   |    ❌ ST2    ✅ ST3    ❌ KK    ✅ OpenTTS     | 11 voices
| de |   🇩🇪  German             |  🥈 Good support   |    ❌ ST2    ✅ ST3    ❌ KK    ✅ OpenTTS     | 11 voices
| el |   🇬🇷  Greek              |  🥈 Good support   |    ❌ ST2    ✅ ST3    ❌ KK    ✅ OpenTTS     | 11 voices
| fi |   🇫🇮  Finnish            |  🥈 Good support   |    ❌ ST2    ✅ ST3    ❌ KK    ✅ OpenTTS     | 11 voices
| hu |   🇭🇺  Hungarian          |  🥈 Good support   |    ❌ ST2    ✅ ST3    ❌ KK    ✅ OpenTTS     | 11 voices
| nl |   🇳🇱  Dutch              |  🥈 Good support   |    ❌ ST2    ✅ ST3    ❌ KK    ✅ OpenTTS     | 11 voices
| ru |   🇷🇺  Russian            |  🥈 Good support   |    ❌ ST2    ✅ ST3    ❌ KK    ✅ OpenTTS     | 11 voices
| sv |   🇸🇪  Swedish            |  🥈 Good support   |    ❌ ST2    ✅ ST3    ❌ KK    ✅ OpenTTS     | 11 voices
| tr |   🇹🇷  Turkish            |  🥈 Good support   |    ❌ ST2    ✅ ST3    ❌ KK    ✅ OpenTTS     | 11 voices
| bg |   🇧🇬  Bulgarian          |  Supported         |    ❌ ST2    ✅ ST3    ❌ KK    ❌ OpenTTS     | 10 voices
| hr |   🇭🇷  Croatian           |  Supported         |    ❌ ST2    ✅ ST3    ❌ KK    ❌ OpenTTS     | 10 voices
| da |   🇩🇰  Danish             |  Supported         |    ❌ ST2    ✅ ST3    ❌ KK    ❌ OpenTTS     | 10 voices
| et |   🇪🇪  Estonian           |  Supported         |    ❌ ST2    ✅ ST3    ❌ KK    ❌ OpenTTS     | 10 voices
| id |   🇮🇩  Indonesian         |  Supported         |    ❌ ST2    ✅ ST3    ❌ KK    ❌ OpenTTS     | 10 voices
| lv |   🇱🇻  Latvian            |  Supported         |    ❌ ST2    ✅ ST3    ❌ KK    ❌ OpenTTS     | 10 voices
| lt |   🇱🇹  Lithuanian         |  Supported         |    ❌ ST2    ✅ ST3    ❌ KK    ❌ OpenTTS     | 10 voices
| pl |   🇵🇱  Polish             |  Supported         |    ❌ ST2    ✅ ST3    ❌ KK    ❌ OpenTTS     | 10 voices
| ro |   🇷🇴  Romanian           |  Supported         |    ❌ ST2    ✅ ST3    ❌ KK    ❌ OpenTTS     | 10 voices
| sk |   🇸🇰  Slovak             |  Supported         |    ❌ ST2    ✅ ST3    ❌ KK    ❌ OpenTTS     | 10 voices
| sl |   🇸🇮  Slovenian          |  Supported         |    ❌ ST2    ✅ ST3    ❌ KK    ❌ OpenTTS     | 10 voices
| uk |   🇺🇦  Ukrainian          |  Supported         |    ❌ ST2    ✅ ST3    ❌ KK    ❌ OpenTTS     | 10 voices
| vi |   🇻🇳  Vietnamese         |  Supported         |    ❌ ST2    ✅ ST3    ❌ KK    ❌ OpenTTS     | 10 voices
| bn |   🇧🇩  Bengali            |  Supported         |    ❌ ST2    ❌ ST3    ❌ KK    ✅ OpenTTS     | 1 voice
| ca |   🇪🇸  Catalan            |  Supported         |    ❌ ST2    ❌ ST3    ❌ KK    ✅ OpenTTS     | 1 voice
| gu |   🇮🇳  Gujarati           |  Supported         |    ❌ ST2    ❌ ST3    ❌ KK    ✅ OpenTTS     | 1 voice
| kn |   🇮🇳  Kannada            |  Supported         |    ❌ ST2    ❌ ST3    ❌ KK    ✅ OpenTTS     | 1 voice
| mr |   🇮🇳  Marathi            |  Supported         |    ❌ ST2    ❌ ST3    ❌ KK    ✅ OpenTTS     | 1 voice
| pa |   🇮🇳  Punjabi            |  Supported         |    ❌ ST2    ❌ ST3    ❌ KK    ✅ OpenTTS     | 1 voice
| sw |   🇰🇪  Swahili            |  Supported         |    ❌ ST2    ❌ ST3    ❌ KK    ✅ OpenTTS     | 1 voice
| ta |   🇮🇳  Tamil              |  Supported         |    ❌ ST2    ❌ ST3    ❌ KK    ✅ OpenTTS     | 1 voice
| te |   🇮🇳  Telugu             |  Supported         |    ❌ ST2    ❌ ST3    ❌ KK    ✅ OpenTTS     | 1 voice

Run `vtmate --list-voices` to print every voice for every language and TTS system.

## Acceleration support

Do you have GPU? (nvidia? an apple computer?) Great! then vtmate speed is at lighting speed =)

* To be able to use acceleration, pick the built version for your hardware from [Releases list](https://github.com/DavidValin/vtmate/releases)
* For CUDA install the CUDA Toolkit (12.x or 13.x) and cuDNN 9 (on Windows, put cuDNN's `bin\<cuda major>.x` directory on PATH, or copy its DLLs next to the other vtmate libraries). `installer.sh` checks both are reachable and falls back to the Vulkan/CPU build otherwise. For Vulkan install VULKAN SDK

```
macOS:            ✅ CPU    ✅ Metal
Linux (amd64):    ✅ CPU    ✅ CUDA     ✅ Vulkan
Linux (arm64):    ✅ CPU    ❌ CUDA     ✅ Vulkan
Windows (x86_64)  ✅ CPU    ✅ CUDA     ✅ Vulkan
```

## Build vtmate from source code

**Full configurable builds (OS, arch and GPU acceleration)**

see:
```
build_linux.sh
build_macos.sh
build_windows.ps1
```

Have fun o:)
