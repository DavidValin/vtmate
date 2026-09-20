<p align="center">
  <img src="https://raw.githubusercontent.com/DavidValin/vtmate/main/assets/vtmate-logo-box.png" width="128" height="128" alt="vtmate" />
</p>

<h1 align="center">vtmate</h1>

<p align="center">
  <strong>Local voice AI conversations in your terminal.</strong><br />
  Realistic voices. Extremely low latency. 41 languages.
</p>

<p align="center">
  <a href="https://github.com/DavidValin/vtmate/releases"><img src="https://img.shields.io/github/v/release/DavidValin/vtmate?style=flat&label=release" alt="Latest release" /></a>
  <a href="https://github.com/DavidValin/vtmate"><img src="https://img.shields.io/github/stars/DavidValin/vtmate?style=flat&logo=github&label=Star" alt="GitHub stars" /></a>
</p>

<p align="center">
  <a href="https://vtmate.eu/en"><strong>https://vtmate.eu/en</strong></a>
</p>

The final AI voice conversational system all running in your terminal! vtmate is a Powerful terminal-based voice ai toolkit with many realistic voices, extremely low latency, 41 languages supported. Allows you to voice conversate with local ai models (or cloud based), pipe data and save into files.

### Installation
```
curl -fsSL https://raw.githubusercontent.com/DavidValin/vtmate/main/installer.sh | sh
```

The program self-contains all TTS models and voices and necessary files to recognize speech and speak with voice with no external installations ensuring maximum portability.

* [🎥 Video demos](https://vtmate.eu/en/showcase)
* [⬇️  Download](https://github.com/DavidValin/vtmate/releases) (⭐ MacOS ⭐ Linux and ⭐ Windows supported)
* [📄 Documentation](https://vtmate.eu/en/docs)
* [🤠 Quicksheet (PDF)](https://raw.githubusercontent.com/DavidValin/vtmate/refs/heads/main/docs/en/quicksheet.pdf) (🖨️ print ready for easy access)

![vtmate screenshot](preview.png)

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

* Background mode features can be used to assist your daily routine with ai powered voice responses while you use other apps, voice read your favourite books or articles, write emails via vo>

## Documentation

- [Configure agents](https://vtmate.eu/en/docs/configuration/agents)
- [How to use it](https://vtmate.eu/en/docs)
  - [Conversation mode](https://vtmate.eu/en/docs/how-to-use-it/conversation-mode)
  - [Debate mode](https://vtmate.eu/en/docs/how-to-use-it/debate-mode)
  - [Background mode (global shortcuts)](https://vtmate.eu/en/docs/how-to-use-it/background-mode)
  - [Read mode (file to speech)](https://vtmate.eu/en/docs/how-to-use-it/read-mode)
  - [PTT/LIVE modes](https://vtmate.eu/en/docs/how-to-use-it/ptt-live)
  - [Single run mode](https://vtmate.eu/en/docs/how-to-use-it/single-run-mode)
  - [CLI: STT / TTS](https://vtmate.eu/en/docs/how-to-use-it/cli-stt-tts)
- [Saving sessions](https://vtmate.eu/en/docs/saving-sessions)

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
