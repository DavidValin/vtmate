## The final AI voice conversational system all running in your terminal!

Powerful terminal-based voice ai toolkit with realistic voices and extremely low latency.

Finally the cross platform voice ui you were waiting on now available for MacOS, Windows and Linux, no need for external installations.

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
- 📌 Integrated `whisper`
- 📌 Integrated `kokoro TTS` system
- 📌 Interface with `OpenTTS` system
- 📌 Supports `ollama` or `llama-server`
- 📌 28 languages supported (`vtmate --list-voices`)
- 📌 Use any gguf model from huggingface.com or ollama models (small models reply faster)

### Status

✅ Ready to kick in! [Download](https://github.com/DavidValin/vtmate/releases)

## How it works

`RECORD -> STT -> LLM -> TTS -> PLAYBACK`

```
- You start the program and start talking.
- Once audio is detected (based on sound-threshold-peak option) it will start recording.
- As soon as there is a time of silence (based on end_silence_ms option), it will transcribe the recorded audio using speech to text (stt). In ptt mode, this option is ignored, the program will wait for SPACE key to be released to submit the audio.
- The transcribed text will be sent to the ai model (through ollama)
- The ai model will reply with text.
- The text converted to audio using text to speech (tts) via OpenTTS.
- You can interrupt the ai agent at any moment by start speaking, this will cause the response and audio to stop and you can continue talking.
```

## LLM integration

- ✅ ollama (default)
- ✅ llama-server
- ✅ openclaw / clawbot (voice chat with your agent by connecting vtmate to the endpoint)

You can run the models locally (by default) or remotely by configuring the base urls via cli option.

## TTS engine support

- ✅ Kokoro TTS (default and integrated)
- ✅ OpenTTS (requires external service)

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

* Voice mixing is supported, you can create a voice by mixing 2 voices by percentage. Example mixing 50% of bm_daniel and 50% of am_puck: set voice name to `bm_daniel.5+am_puck.5`

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

####  Separate agents

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

####  Useful to know

vtmate self contains espeak-ng-data, the whisper tiny & small models and kokoro model and voices which will be autoextracted when running vtmate if they are not found in next locations:

- `~/.vtmate/espeak-ng-data.tar.gz`
- `~/.whisper-models/ggml-tiny.bin`
- `~/.whisper-models/ggml-small.bin`
- `~/.cache/k/0.onnx`
- `~/.cache/k/0.bin`

* If you want to avoid sound interruptions you can use `ptt` mode or increase the `sound_threshold_peak` for your microphone levels.
* If you want to use OpenTTS, start the docker service first: `docker run --rm --platform=linux/amd64 -p 5500:5500 synesthesiam/opentts:all` (it will pull the image the first time). Adjust the platform as needed depending on your hardware.
* If you have problems starting vtmate you can remove `~/vtmate/settings` so it recreates the default configuration
* By default whisper tiny is used (from ~/.whisper-models/ggml-small.bin). If you need better speech recognition, download a better whisper model and update the `whisper_model_path` setting.

If you need help:

```
vtmate --help
```

## Acceleration support

Do you have GPU? (nvidia? an apple computer?) Great! then vtmate speed is at lighting speed =)

* To be able to use acceleration, pick the built version for your hardware from [Releases list](https://github.com/DavidValin/vtmate/releases)
* For CUDA install CUDA Toolkit. For Vulkan install VULKAN SDK

```
Platform   Arch    CPU    OpenBLAS   CUDA   Metal   Vulkan
--------   ----    ---    --------   ----   -----   ------
macOS      ARM64   ✅    optional     n/a     ✅      ⚠️
Linux      AMD64   ✅       ✅        ✅      n/a     ⚠️
Linux      ARM64   ✅       ✅        ⚠️      n/a     ⚠️
Windows    x86     ✅       ✅        ✅      n/a     ⚠️
Windows    ARM64   ✅       ⚠️        ⚠️      n/a     ⚠️
```

⚠️ Currently working on full static builds for all OS. You can download a release or build it yourself

## Language support

| ID |           Language           |      Support       |        TTS supported           |
|----|------------------------------|--------------------|--------------------------------|
| en |       🇬🇧  English            |  🏆 Best support   |    ✅ Kokoro    ✅ OpenTTS     |
| es |       🇪🇸  Spanish            |  🏆 Best support   |    ✅ Kokoro    ✅ OpenTTS     |
| zh |       🇨🇳  Mandarin Chinese   |  🏆 Best support   |    ✅ Kokoro    ✅ OpenTTS     |
| ja |       🇯🇵  Japanese           |  🏆 Best support   |    ✅ Kokoro    ✅ OpenTTS     |
| pt |       🇵🇹  Portuguese         |  🏆 Best support   |    ✅ Kokoro    ❌ OpenTTS     |
| it |       🇮🇹  Italian            |  🏆 Best support   |    ✅ Kokoro    ✅ OpenTTS     |
| hi |       🇮🇳  Hindi              |  🏆 Best support   |    ✅ Kokoro    ✅ OpenTTS     |
| fr |       🇫🇷  French             |  🏆 Best support   |    ✅ Kokoro    ✅ OpenTTS     |
| ar |       🇸🇦  Arabic             |     Supported      |    ❌ Kokoro    ✅ OpenTTS     |
| bn |       🇧🇩  Bengali            |     Supported      |    ❌ Kokoro    ✅ OpenTTS     |
| ca |       🇪🇸  Catalan            |     Supported      |    ❌ Kokoro    ✅ OpenTTS     |
| cs |       🇨🇿  Czech              |     Supported      |    ❌ Kokoro    ✅ OpenTTS     |
| de |       🇩🇪  German             |     Supported      |    ❌ Kokoro    ✅ OpenTTS     |
| el |       🇬🇷  Greek              |     Supported      |    ❌ Kokoro    ✅ OpenTTS     |
| fi |       🇫🇮  Finnish            |     Supported      |    ❌ Kokoro    ✅ OpenTTS     |
| gu |       🇮🇳  Gujarati           |     Supported      |    ❌ Kokoro    ✅ OpenTTS     |
| hu |       🇭🇺  Hungarian          |     Supported      |    ❌ Kokoro    ✅ OpenTTS     |
| kn |       🇮🇳  Kannada            |     Supported      |    ❌ Kokoro    ✅ OpenTTS     |
| ko |       🇰🇷  Korean             |     Supported      |    ❌ Kokoro    ✅ OpenTTS     |
| mr |       🇮🇳  Marathi            |     Supported      |    ❌ Kokoro    ✅ OpenTTS     |
| nl |       🇳🇱  Dutch              |     Supported      |    ❌ Kokoro    ✅ OpenTTS     |
| pa |       🇮🇳  Punjabi            |     Supported      |    ❌ Kokoro    ✅ OpenTTS     |
| ru |       🇷🇺  Russian            |     Supported      |    ❌ Kokoro    ✅ OpenTTS     |
| sv |       🇸🇪  Swedish            |     Supported      |    ❌ Kokoro    ✅ OpenTTS     |
| sw |       🇰🇪  Swahili            |     Supported      |    ❌ Kokoro    ✅ OpenTTS     |
| ta |       🇮🇳  Tamil              |     Supported      |    ❌ Kokoro    ✅ OpenTTS     |
| te |       🇮🇳  Telugu             |     Supported      |    ❌ Kokoro    ✅ OpenTTS     |
| tr |       🇹🇷  Turkish            |     Supported      |    ❌ Kokoro    ✅ OpenTTS     |

## Build vtmate from source code

Simplest way:
```
cargo install vtmate
```

For custom builds:

- There are 3 script `build_macos.sh`, `build_linux.sh` and `build_windows.bat`
- The scripts accept --arch flag to build for specific architecture
- If you are building for specific acceleration, make sure the SDKs are installed
- During build, TTS and STT models are fetched locally (around 1GB)
- The built binaries will be placed under `./dist` once built

***MacOS***
- (require docker for building the image)
- You can only build the MacOS build from a mac machine
```
./build_macos.sh
```

***Linux***
- (require docker for building the image)
- Openblas is always included in all variants by default

Examples:
```
WITH_CUDA=0 ./build_linux.sh --arch amd64
WITH_CUDA=1 ./build_linux.sh --arch amd64
LINUX_WITH_VULKAN=1 WITH_CUDA=0 ./build_linux.sh --arch amd64

WITH_CUDA=0 ./build_linux.sh --arch arm64
LINUX_WITH_VULKAN=1 WITH_CUDA=0 ./build_linux.sh --arch arm64
```

***Windows***
- You can only build the Windows build from windows
- You need to install [Visual CPP Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools)

```
build_windows.bat cpu
build_windows.bat cuda
build_windows.bat openblas
build_windows.bat vulkan
```

Have fun o:)
