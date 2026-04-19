## The final AI voice conversational system all running in your terminal!

Powerful ai toolkit to interact with ai models via voice from your terminal at extremely low latency with realistic voices. Allows live voice conversations and run debates between ai agents with user intervention, stdin and text file inputs and more

Finally the cross platform voice ui you were waiting on now available for MacOS, Windows and Linux, no need for external installations.

🚀 Now it also supports automatic infinite debates between agents. Listen and learn, and you can participate in the debate too via voice

#### **Sponsor this project**
[![Sponsor ai-mate](https://img.shields.io/static/v1?label=Sponsor&message=%E2%9D%A4&logo=GitHub&color=%23fe8e86)](https://github.com/sponsors/DavidValin)

![ai mate screenshot](https://github.com/DavidValin/ai-mate/raw/main/preview.png)

### English demo
https://github.com/user-attachments/assets/d9c27108-41f7-4148-8c32-28c8ca6d8516

### Spanish demo
https://github.com/user-attachments/assets/e612feaa-8ab0-4761-9c67-53ec7d40cab7

### Status

✅ Ready to kick in! [Download](https://github.com/DavidValin/ai-mate/releases)

## How it works

`RECORD -> STT -> LLM -> TTS -> PLAYBACK`

```
- You start the program and start talking.
- Once audio is detected (based on sound-threshold-peak option) it will start recording.
- As soon as there is a time of silence (based on end_silence_ms option), it will transcribe the recorded audio using speech to text (stt).
- The transcribed text will be sent to the ai model (through ollama)
- The ai model will reply with text.
- The text converted to audio using text to speech (tts) via OpenTTS.
- You can interrupt the ai agent at any moment by start speaking, this will cause the response and audio to stop and you can continue talking.
```

## Features

- 📌 Continuous Voice chat (live conversation): `records user continuously and stops on silence, submitting the request to the agent`
- 🚀 AI agents debate (2 agents talking to each other): `give an initial input and let the agents talk to each other. You can interrupt in the middle of the debate changing the subject`
- 📌 Realtime agent swap: `change the agent by pressing <ARROW_LEFT> / <ARROW_RIGHT> (applicable to next response)`
- 📌 Voice interrupt: `the agent stops talking if you interrupt via voice`
- 📌 Recording Pause / Resume: `toggle "<SPACE>" key to pause / resume voice recording only`
- 📌 Stop PlayBack: `press "<ESCAPE>" ONCE to stop the playback for the current response`
- 📌 Interrupt: `press "<ESCAPE>" TWICE to interrupt the current response alltogether`
- 📌 Push to Talk mode (PTT): `keep <SPACE> pressed while talking and release to stop recording`
- 📌 Voice speed change: `change the agent voice speed by pressing <ARROW_UP> / <ARROW_DOWN> (applicable to next response)`
- 📌 Voice read a txt file: `ai-mate -r myfile.txt`
- 📌 Voice read text from stdin phrase by phrase: `echo "Hello. How are you?" | ai-mate -r -`
- 📌 Save conversation as audio and text: `ai-mate -s`
- 📌 Load separate settings file with different agents: `ai-mate -c philosophers-settings.txt`
- 📌 Integrated `whisper`
- 📌 Integrated `kokoro TTS` system
- 📌 Interface with `OpenTTS` system
- 📌 Supports `ollama` or `llama-server`
- 📌 28 languages supported (`ai-mate --list-voices`)
- 📌 Use any gguf model from huggingface.com or ollama models (small models reply faster)

## LLM integration

- ✅ ollama (default)
- ✅ llama-server
- ✅ openclaw / clawbot (voice chat with your agent by connecting ai-mate to the endpoint)

You can run the models locally (by default) or remotely by configuring the base urls via cli option.

## TTS engine support

- ✅ Kokoro TTS (default and integrated)
- ✅ OpenTTS (requires external service)

## Installation

### 📌 1. **Download ai-mate**
- `https://github.com/DavidValin/ai-mate/releases`
- Move the binary to a folder in your $PATH so you can use `ai-mate` command anywhere

### 📌 2. **Install llm engine (needed for ai responses)**

Option A- ollama (the default)
- Install `https://ollama.com/download`.
- Pull the model you want to use with ai-mate, for instance: `ollama pull llama3.2:3b`.

Option B- llama-server support.
- Install llama.cpp: `https://github.com/ggml-org/llama.cpp`.
- Download a gguf model: `https://huggingface.co/QuantFactory/Meta-Llama-3-8B-Instruct-GGUF/resolve/main/Meta-Llama-3-8B-Instruct.Q8_0.gguf?download=true`.

### 📌 3. **(Windows only) Install supported terminal**

- Install Windows Terminal (which supports emojis): `https://apps.microsoft.com/detail/9n0dx20hk701` (use this terminal to run ai-mate)

### 📌 4. **(Optional) OpenTTS support**

- `docker pull synesthesiam/opentts:all`

## Configure agents

The first time you run ai-mate it will create a configuration file if it doesn't exist in `~/.ai-mate/settings` with 2 agents. You can define as many agents as you want.

Example of agent definition:

```
[agent]
name = main agent
language = en
voice = bf_alice
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
ai-mate --help
```

## How to use it

The first agent defined in `~/ai-mate/settings` will always be selected agent when running ai-mate, unless `--agent <agent_name>` is used.

Before running ai-mate make sure ollama is running: `ollama serve`

#### Conversation mode

Start conversation with default agent (waits for user voice input and respond)

```
ai-mate
```

Start conversation with a specific agent (waits for user voice input and respond)
```
ai-mate --agent "main agent"
```

Start conversation with an initial text prompt
```
ai-mate -p "Are we alone in the galaxy?"
```

Start conversation with an initial prompt from file
```
ai-mate -i myprompt.txt
```

Start conversation with an initial prompt from stdin
```
echo "How to fly without wings?" | ai-mate -i -
```

* You can switch agents in realtime by pressing `ARROW_LEFT` / `ARROW_RIGHT` keyword arrows (you need at least 2 agents defined in `~/ai-mate/settings`).
* You can change the voice speed by pressing `ARROW_UP` / `ARROW_DOWN`
* Be able to save the conversation in a wav and text file by adding `-s` option. It will save it in `~/.ai-mate/conversations` folder

#### Debate mode

Initialize a debate between two agents and be able to participate in the debate by speaking at any time. To create a good debate adjust the system prompts of each agent and give a detailed initial input.

Start a debate with an initial subject
```
ai-mate --debate "God" "Devil" "How to succeed in life?"
```

Start a debate with an initial prompt from file
```
ai-mate --debate "God" "Devil" -i myprompt.txt
```

Start a debate with an initial prompt from stdin
```
echo "Lets discuss the permissions of this files: \n\n $(ls -la)" | ai-mate --debate "Unix administrator" "Security Expert" -i -
```

* You can also start/stop a debate from conversation mode by pressing `Control+D` and picking the debate agents.
* Be able to save the conversation in a wav and text file by adding `-s` option. It will save it in `~/.ai-mate/conversations` folder

#### Single run

Get a single response from prompt
```
ai-mate -q -p "Explain me the Zettelkasten Method"
```

Get a single response from prompt from file
```
ai-mate -q -i myprompt.txt
```

Get a single response from prompt from stdin
```
echo "Is $(date) a national holiday day in Spain?" | ai-mate -q -i -
```

####  File to speech

Read a text file or stdin text phrase by phrase. Ensure the agent you choose has correct language and voice for your text.
In this mode, only the next agent settings are used: "tts", "voice" and "language".

from a txt file:
```
ai-mate -r myfile.txt --agent "main agent"
```

from stdin text:
```
ai-mate -r - --agent "main agent"
```

In this mode you can:

* Move to previous phrase by pressing `ARROW_UP`
* Move to next phrase by pressing `ARROW_DOWN`
* Stop / Resume playback by pressing `SPACE`

####  Separate agents

By default ai-mate uses `~/.ai-mate/settings` file.
You can create different setting fields for different agent groups, example:

```
philosophers.txt
scientists.txt
employees.txt
```

And then load each as you need:
```
ai-mate -c philosophers.txt --debate "Aristoteles" "Ptahhotep" "how to achieve harmony?"
```

####  Useful to know

ai-mate self contains espeak-ng-data, the whisper tiny & small models and kokoro model and voices which will be autoextracted when running ai-mate if they are not found in next locations:

- `~/.ai-mate/espeak-ng-data.tar.gz`
- `~/.whisper-models/ggml-tiny.bin`
- `~/.whisper-models/ggml-small.bin`
- `~/.cache/k/0.onnx`
- `~/.cache/k/0.bin`

* If you want to avoid sound interruptions you can use `ptt` mode or increase the `sound_threshold_peak` for your microphone levels.
* If you want to use OpenTTS, start the docker service first: `docker run --rm --platform=linux/amd64 -p 5500:5500 synesthesiam/opentts:all` (it will pull the image the first time). Adjust the platform as needed depending on your hardware.
* If you have problems starting ai-mate you can remove `~/ai-mate/settings` so it recreates the default configuration
* By default whisper tiny is used (from ~/.whisper-models/ggml-small.bin). If you need better speech recognition, download a better whisper model and update the `whisper_model_path` setting.

If you need help:

```
ai-mate --help
```

## Acceleration support

Do you have GPU? (nvidia? an apple computer?) Great! then ai-mate speed is at lighting speed =)

* To be able to use acceleration, pick the built version for your hardware from [Releases list](https://github.com/DavidValin/ai-mate/releases)
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
| en |       🇺🇸  English            |  🏆 Best support   |    ✅ Kokoro    ✅ OpenTTS     |
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

## Build ai-mate from source code

Simplest way:
```
cargo install ai-mate
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
