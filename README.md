## The final AI voice conversational system all running in your terminal!

Powerful terminal-based voice ai toolkit with many realistic voices, extremely low latency, 28 languages supported. Allows you to voice conversate with local ai models. [Download](https://github.com/DavidValin/vtmate/releases) (⭐ MacOS ⭐ Linux and ⭐ Windows supported)

The program self contains (1.2GB) all tts models and voices and necessary files to recognize speech and speak with voice with no external intallations ensuring maximum portability.

### Video demonstration
<details>
<summary>(🇬🇧 English) Conversation video demo</summary>

https://github.com/user-attachments/assets/d9c27108-41f7-4148-8c32-28c8ca6d8516

</details>
<details>
<summary>(🇪🇸 Spanish) Conversation video demo</summary>

https://github.com/user-attachments/assets/e612feaa-8ab0-4761-9c67-53ec7d40cab7

</details>

#### **Sponsor this project**
[![Sponsor vtmate](https://img.shields.io/static/v1?label=Sponsor&message=%E2%9D%A4&logo=GitHub&color=%23fe8e86)](https://github.com/sponsors/DavidValin)

![ai mate screenshot](https://github.com/DavidValin/vtmate/raw/main/preview.png)

![how it works](https://github.com/DavidValin/vtmate/raw/main/docs/en/diagrams/how-it-works.png)

## Features

- 📌 Continuous Voice chat (live conversation) with voice interruption
- 📌 Realtime agent swap
- 📌 Push to Talk mode (PTT)
- 📌 Recording Pause / Resume via keyboard
- 🚀 AI agents debate (2 agents talking to each other; use can also participate in between)
- 📌 Stop playback for the current response via keyboard
- 📌 Interrupt response altogether via keyboard
- 📌 Live voice speed change via keyboard (applicable to next response)
- 📌 Read a text file with voice, phrase by phrase, with keyboard navigation and pause/resume
- 📌 Read text with voice from stdin, phrase by phrase, with keyboard navigation and pause/resume
- 📌 Save conversation as audio and text
- 📌 Load separate settings file with different agents
- 📌 Integrated `whisper` speech recognition system (no external intallation required)
- 📌 Integrated `kokoro TTS` and `supersonic 2 TTS` systems (no external intallation required)
- 📌 Interface with `OpenTTS` system (requires external docker service)
- 📌 Supports `ollama` or `llama-server`
- 📌 28 languages supported (`vtmate --list-voices`)
- 📌 Use any gguf model from huggingface.com or ollama models (small models reply faster)

## How it works

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

- ✅ ollama (default)
- ✅ llama-server

You can run the models locally (by default) or remotely by configuring the base urls via cli option.

## TTS engine support

- ✅ Kokoro (integrated)
- ✅ Supersonic 2 (integrated)
- ✅ OpenTTS (requires external docker service)


## Installation

### 📌 1. **Download vtmate**
- `https://github.com/DavidValin/vtmate/releases`
- Move the binary to a folder in your $PATH so you can use `vtmate` command anywhere

### 📌 2. **Install llm engine (needed for ai responses)**

Option A- ollama (the default)
- Install `https://ollama.com/download`.
- Pull the model you want to use with vtmate, for instance: `ollama pull llama3.2:3b`.

Option B- llama-server support.
- Install llama.cpp: `https://github.com/ggml-org/llama.cpp`.
- Download a gguf model: `https://huggingface.co/QuantFactory/Meta-Llama-3-8B-Instruct-GGUF/resolve/main/Meta-Llama-3-8B-Instruct.Q8_0.gguf?download=true`.

### 📌 3. **(Windows only) Install supported terminal**

- Install Windows Terminal (which supports emojis): `https://apps.microsoft.com/detail/9n0dx20hk701` (use this terminal to run vtmate)

### 📌 4. **(Optional) OpenTTS support**

- `docker pull synesthesiam/opentts:all`

## Configure agents

The first time you run vtmate it will create a configuration file if it doesn't exist in `~/.vtmate/settings` with 2 agents. You can define as many agents as you want.

Example of agent definition:

```
[agent]
name = main agent
language = en
voice = bf_alice
voice_speed = 1.1
provider = ollama
baseurl = http://localhost:8080
model = gpt-oss-20b
system_prompt = You are a smart ai assistant. You reply to the user with the necessary information following the next rules: Avoid suggestions unless they contribute to the specific user request. If the user hasn't requested anything specific ask the exact questions to find out exactly what he needs assistance with. Replies are no longer than 20 words unless a longer explanation is required.
sound_threshold_peak = 0.1
end_silence_ms = 2000
tts = kokoro
ptt = false
whisper_model_path = ~/.whisper-models/ggml-tiny.bin
```

* Voice mixing is supported for kokoro tts system only, you can create a voice by mixing 2 kokoro voices by percentage. Example mixing 50% of bm_daniel and 50% of am_puck: set voice name to `bm_daniel.5+am_puck.5`

To see explanation of each field:
```
vtmate --help
```

## How to use it

The first agent defined in `~/vtmate/settings` will always be selected agent when running vtmate, unless `--agent <agent_name>` is used.

Before running vtmate make sure ollama is running: `ollama serve`

### Conversation mode

![conversation mode](https://github.com/DavidValin/vtmate/raw/main/docs/en/diagrams/conversation-mode.png)

Start conversation with default agent and save it as audio and text
(waits for user voice input and respond)

```
vtmate -s
```

Start conversation with a specific agent
(waits for user voice input and respond)
```
vtmate --agent "main agent"
```

Start conversation with an initial text prompt
```
vtmate -p "Are we alone in the galaxy?"
```

Start conversation with an initial prompt from file
```
vtmate -i myprompt.txt
```

Start conversation with an initial prompt from stdin
```
echo "How to fly without wings?" | vtmate -i -
```

* You can switch agents in realtime by pressing `ARROW_LEFT` / `ARROW_RIGHT` keyword arrows (you need at least 2 agents defined in `~/vtmate/settings`).
* You can change the voice speed by pressing `ARROW_UP` / `ARROW_DOWN`
* Be able to save the conversation in a wav and text file by adding `-s` option. It will save it in `~/.vtmate/conversations` folder

### Debate mode

![debate mode](https://github.com/DavidValin/vtmate/raw/main/docs/en/diagrams/debate-mode.png)

Initialize a debate between two agents and be able to participate in the debate by speaking at any time. To create a good debate adjust the system prompts of each agent and give a detailed initial input.

Start a debate with an initial subject
```
vtmate --debate "God" "Devil" "How to succeed in life?"
```

Start a debate with an initial prompt from file
```
vtmate --debate "God" "Devil" -i myprompt.txt
```

Start a debate with an initial prompt from stdin
```
echo "Lets discuss the permissions of this files: \n\n $(ls -la)" | vtmate --debate "Unix administrator" "Security Expert" -i -
```

* You can also start/stop a debate from conversation mode by pressing `Control+D` and picking the debate agents.
* Be able to save the conversation in a wav and text file by adding `-s` option. It will save it in `~/.vtmate/conversations` folder

### Single run

Get a single response from prompt
```
vtmate -q -p "Explain me the Zettelkasten Method"
```

Get a single response from prompt from file
```
vtmate -q -i myprompt.txt
```

Get a single response from prompt from stdin
```
echo "Is $(date) a national holiday day in Spain?" | vtmate -q -i -
```

Get a single response and save it as audio file and text file
```
echo "Can you find any suspicious processes in the next list? If so, why?\n\n $(ps aux | head -20)" | vtmate -q -i - -s
```

###  File to speech mode

![read file mode](https://github.com/DavidValin/vtmate/raw/main/docs/en/diagrams/reading-mode.png)

Read a text file or stdin text phrase by phrase using an agent voice. Ensure the agent you choose has correct language and voice for your text.
In this mode, only the next agent settings are used: "tts", "voice" and "language".

from a txt file:
```
vtmate -r myfile.txt --agent "main agent"
```

from stdin text:
```
vtmate -r - --agent "main agent"
```

In this mode you can:

* Move to previous phrase by pressing `ARROW_UP`
* Move to next phrase by pressing `ARROW_DOWN`
* Stop / Resume playback by pressing `SPACE`

###  Separate agents

By default vtmate uses `~/.vtmate/settings` file.
You can create different setting fields for different agent groups, example:

```
philosophers.txt
scientists.txt
employees.txt
```

And then load each as you need:
```
vtmate -c philosophers.txt --debate "Aristoteles" "Ptahhotep" "how to achieve harmony?"
```

###  Model files

vtmate self contains (no need for manual installation) espeak-ng-data, the whisper tiny & small models, kokoro model and voices and supersonic2 model and voices which will be autoextracted from the binary when running vtmate if they are not found in next locations:

whisper models:
```
- `~/.whisper-models/ggml-tiny.bin`
- `~/.whisper-models/ggml-small.bin`
```

kokoro model files:
```
~/.cache/k/0.onnx
~/.cache/k/0.bin
```

espeak phonemes (used by kokoro):
```
- `~/.vtmate/espeak-ng-data.tar.gz`
```

supersonic2 files:
```
~/.vtmate/tts/supersonic2-model/onnx/duration_predictor.onnx
~/.vtmate/tts/supersonic2-model/onnx/text_encoder.onnx
~/.vtmate/tts/supersonic2-model/onnx/tts.json
~/.vtmate/tts/supersonic2-model/onnx/unicode_indexer.json
~/.vtmate/tts/supersonic2-model/onnx/vector_estimator.onnx
~/.vtmate/tts/supersonic2-model/onnx/vocoder.onnx
~/.vtmate/tts/supersonic2-model/voice_styles/M1.json
~/.vtmate/tts/supersonic2-model/voice_styles/M2.json
~/.vtmate/tts/supersonic2-model/voice_styles/M3.json
~/.vtmate/tts/supersonic2-model/voice_styles/M4.json
~/.vtmate/tts/supersonic2-model/voice_styles/M5.json
~/.vtmate/tts/supersonic2-model/voice_styles/F1.json
~/.vtmate/tts/supersonic2-model/voice_styles/F2.json
~/.vtmate/tts/supersonic2-model/voice_styles/F3.json
~/.vtmate/tts/supersonic2-model/voice_styles/F4.json
~/.vtmate/tts/supersonic2-model/voice_styles/F5.json
```

* If you want to avoid sound interruptions you can use `ptt` mode or increase the `sound_threshold_peak` for your microphone levels.
* If you want to use OpenTTS, start the docker service first: `docker run --rm --platform=linux/amd64 -p 5500:5500 synesthesiam/opentts:all` (it will pull the image the first time). Adjust the platform as needed depending on your hardware.
* If you have problems starting vtmate you can remove `~/vtmate/settings` so it recreates the default configuration
* By default whisper tiny is used (from ~/.whisper-models/ggml-small.bin). If you need better speech recognition, download a better whisper model and update the `whisper_model_path` setting.

If you need help:

```
vtmate --help
```

## Language support

| ID |           Language       |      Support       |        TTS supported   |   Number of voices  |
|----|--------------------------|--------------------|---------------------------------------------------|-------------|
| en |   🇬🇧  English            |  🏆 Best support   |    ✅ Supersonic 2    ✅ Kokoro    ✅ OpenTTS     | > 38 voices
| es |   🇪🇸  Spanish            |  🏆 Best support   |    ✅ Supersonic 2    ✅ Kokoro    ✅ OpenTTS     | > 14 voices
| fr |   🇫🇷  French             |  🏆 Best support   |    ✅ Supersonic 2    ✅ Kokoro    ✅ OpenTTS     | > 12 voices
| zh |   🇨🇳  Mandarin Chinese   |  🥈 Good support   |    ❌ Supersonic 2    ✅ Kokoro    ✅ OpenTTS     | > 9 voices
| ja |   🇯🇵  Japanese           |  🥈 Good support   |    ❌ Supersonic 2    ✅ Kokoro    ✅ OpenTTS     | > 6 voices
| pt |   🇵🇹  Portuguese         |  🥈 Good support   |    ✅ Supersonic 2    ✅ Kokoro    ❌ OpenTTS     | > 13 voices
| ko |   🇰🇷  Korean             |  🥈 Good support   |    ✅ Supersonic 2    ❌ Kokoro    ✅ OpenTTS     | 11 voices
| it |   🇮🇹  Italian            |  🥈 Good support   |    ❌ Supersonic 2    ✅ Kokoro    ✅ OpenTTS     | > 3 voices
| hi |   🇮🇳  Hindi              |  🥈 Good support   |    ❌ Supersonic 2    ✅ Kokoro    ✅ OpenTTS     | > 4 voices
| ar |   🇸🇦  Arabic             |     Supported      |    ❌ Supersonic 2    ❌ Kokoro    ✅ OpenTTS     | 1 voice
| bn |   🇧🇩  Bengali            |     Supported      |    ❌ Supersonic 2    ❌ Kokoro    ✅ OpenTTS     | 1 voice
| ca |   🇪🇸  Catalan            |     Supported      |    ❌ Supersonic 2    ❌ Kokoro    ✅ OpenTTS     | 1 voice
| cs |   🇨🇿  Czech              |     Supported      |    ❌ Supersonic 2    ❌ Kokoro    ✅ OpenTTS     | 1 voice
| de |   🇩🇪  German             |     Supported      |    ❌ Supersonic 2    ❌ Kokoro    ✅ OpenTTS     | 1 voice
| el |   🇬🇷  Greek              |     Supported      |    ❌ Supersonic 2    ❌ Kokoro    ✅ OpenTTS     | 1 voice
| fi |   🇫🇮  Finnish            |     Supported      |    ❌ Supersonic 2    ❌ Kokoro    ✅ OpenTTS     | 1 voice
| gu |   🇮🇳  Gujarati           |     Supported      |    ❌ Supersonic 2    ❌ Kokoro    ✅ OpenTTS     | 1 voice
| hu |   🇭🇺  Hungarian          |     Supported      |    ❌ Supersonic 2    ❌ Kokoro    ✅ OpenTTS     | 1 voice
| kn |   🇮🇳  Kannada            |     Supported      |    ❌ Supersonic 2    ❌ Kokoro    ✅ OpenTTS     | 1 voice
| mr |   🇮🇳  Marathi            |     Supported      |    ❌ Supersonic 2    ❌ Kokoro    ✅ OpenTTS     | 1 voice
| nl |   🇳🇱  Dutch              |     Supported      |    ❌ Supersonic 2    ❌ Kokoro    ✅ OpenTTS     | 1 voice
| pa |   🇮🇳  Punjabi            |     Supported      |    ❌ Supersonic 2    ❌ Kokoro    ✅ OpenTTS     | 1 voice
| ru |   🇷🇺  Russian            |     Supported      |    ❌ Supersonic 2    ❌ Kokoro    ✅ OpenTTS     | 1 voice
| sv |   🇸🇪  Swedish            |     Supported      |    ❌ Supersonic 2    ❌ Kokoro    ✅ OpenTTS     | 1 voice
| sw |   🇰🇪  Swahili            |     Supported      |    ❌ Supersonic 2    ❌ Kokoro    ✅ OpenTTS     | 1 voice
| ta |   🇮🇳  Tamil              |     Supported      |    ❌ Supersonic 2    ❌ Kokoro    ✅ OpenTTS     | 1 voice
| te |   🇮🇳  Telugu             |     Supported      |    ❌ Supersonic 2    ❌ Kokoro    ✅ OpenTTS     | 1 voice
| tr |   🇹🇷  Turkish            |     Supported      |    ❌ Supersonic 2    ❌ Kokoro    ✅ OpenTTS     | 1 voice

## Acceleration support

Do you have GPU? (nvidia? an apple computer?) Great! then vtmate speed is at lighting speed =)

* To be able to use acceleration, pick the built version for your hardware from [Releases list](https://github.com/DavidValin/vtmate/releases)
* For CUDA install CUDA Toolkit. For Vulkan install VULKAN SDK

```
macOS:            ✅ CPU    ✅ Metal
Linux (amd64):    ✅ CPU    ⚠️ CUDA     ❌ Vulkan
Linux (arm64):    ✅ CPU    ⚠️ CUDA     ❌ Vulkan
Windows (x86_64)  ✅ CPU    ⚠️ CUDA     ❌ Vulkan
Windows (arm64)   ❌ CPU    ❌ CUDA     ❌ Vulkan
```

⚠️ Currently working on full static builds for all OS with Openblas + CUDA + Vulkan support. In the meantime, pick a release available from [Releases list](https://github.com/DavidValin/vtmate/releases) or build one yourself.

## Build vtmate from source code

**Simplest way:**
```
cargo install vtmate
```

**Full configurable builds (os, arch and gpu acceleration)**

```
git clone https://github.com/DavidValin/vtmate
```

see:
```
build_linux.sh
build_macos.sh
build_windows.sh
```

Have fun o:)
