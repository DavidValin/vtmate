## the final AI voice conversational system all running in your terminal!

As easy as it sounds. Includes all you need to voice chat with ai models.
Finally the cross platform voice ui you were waiting on now available
for MacOS, Windows and Linux, no need for external installations.

<iframe src="https://github.com/sponsors/DavidValin/card" title="Sponsor DavidValin" height="225" width="600" style="border: 0;"></iframe>

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
- 📌 Voice interrupt: `the agent stops talking if you interrupt via voice`
- 📌 Recording Pause / Resume: `toggle "<SPACE>" key to pause / resume voice recording only`
- 📌 Stop PlayBack: `press "<ESCAPE>" ONCE to stop the playback for the current response`
- 📌 Interrupt: `press "<ESCAPE>" TWICE to interrupt the current response alltogether`
- 📌 Push to Talk mode (PTT): `run it with --ptt and keep <SPACE> while talking and release to stop recording`
- 📌 Voice speed change: `change the agent voice speed by pressing <ARROW_UP> / <ARROW_DOWN> (applicable to next response)`
- 📌 Voice change (realtime): `change the agent voice by pressing <ARROW_LEFT> / <ARROW_RIGHT> (applicable to next response)`
- 📌 Integrated `whisper`
- 📌 Integrated `kokoro TTS` system
- 📌 Interface with `OpenTTS` system
- 📌 Supports `ollama` or `llama-server` or `llamafile`
- 📌 28 languages supported (`ai-mate --list-voices`)
- 📌 Use any gguf model from huggingface.com or ollama models (small models reply faster)

## LLM integration

- ✅ ollama - all versions (default)
- ✅ llama-server / llamafile - all versions
- ✅ openclaw / clawbot (voice chat with your agent by connecting ai-mate to the endpoint)

You can run the models locally (by default) or remotely by configuring the base urls via cli option.

## TTS engine support

- ✅ Kokoro TTS (default and integrated)
- ✅ OpenTTS (requires external service)

## Acceleration support

Do you have GPU? (nvidia? an apple computer?) Great! then ai-mate speed is at lighting speed =)

* To be able to use acceleration, pick the built version for your hardware from [Releases list](https://github.com/DavidValin/ai-mate/releases)
* For CUDA install CUDA Toolkit. For Vulkan install VULKAN SDK

```
Platform   Arch    CPU    OpenBLAS   CUDA   Metal   Vulkan
--------   ----    ---    --------   ----   -----   ------
macOS      ARM64   ✅    optional     n/a     ✅      ❌
Linux      AMD64   ✅       ✅        ✅      n/a     ✅
Linux      ARM64   ✅       ✅        ❌      n/a     ✅
Windows    x86     ⚠️       ⚠️        ⚠️      n/a     ⚠️
Windows    ARM64   ⚠️       ⚠️        ⚠️      n/a     ⚠️
```

## Installation

### 📌 1. **Download ai-mate**
- `https://github.com/DavidValin/ai-mate/releases`
- Move the binary to a folder in your $PATH so you can use `ai-mate` command anywhere

### 📌 2. **Install llm engine (needed for ai responses)**

Option A- ollama (the default)
- Install `https://ollama.com/download`.
- Pull the model you want to use with ai-mate, for instance: `ollama pull llama3.2:3b`.

Option B- llamafile support
- Download a llamafile `https://huggingface.co/mozilla-ai/Meta-Llama-3-8B-Instruct-llamafile/blob/main/Meta-Llama-3-8B-Instruct.Q8_0.llamafile` (this contains an ai model and the server in a single file).
- Once downloaded, if in windows `rename the .llamafile to .exe`; in linux / mac `chmod +x Meta-Llama-3-8B-Instruct.Q8_0.llamafile`.

Option C- llama-server support.
- Install llama.cpp: `https://github.com/ggml-org/llama.cpp`.
- Download a gguf model: `https://huggingface.co/QuantFactory/Meta-Llama-3-8B-Instruct-GGUF/resolve/main/Meta-Llama-3-8B-Instruct.Q8_0.gguf?download=true`.

### 📌 3. **(Windows only) Install supported terminal**

- Install Windows Terminal (which supports emojis): `https://apps.microsoft.com/detail/9n0dx20hk701` (use this terminal to run ai-mate)

### 📌 4. **(Optional) OpenTTS support**

- `docker pull synesthesiam/opentts:all`

## How to use it

Default configuration example:

```
ollama serve
ai-mate
```

Push to Talk (PTT) example:

```
ollama serve
ai-mate --ptt --model "llama3:8b"
```

llamafile example:

```
./Meta-Llama-3-8B-Instruct.Q8_0.llamafile --server
ai-mate --llm llama-server
```

llama-server example:

```
llama-server -m Meta-Llama-3-8B-Instruct.Q8_0.gguf
ai-mate --llm llama-server
```

Below are the default parameters, which you can override, example:

```
ai-mate \
  --llm ollama \
  --tts kokoro \
  --language en \
  --sound-threshold-peak 0.10 \
  --end-silence-ms 850 \
  --whisper-model-path ~/.whisper-models/ggml-tiny.bin \
  --model "llama3.2:3b" \
  --ollama-url "http://localhost:11434"
```

You can just override a specific variable, for example:

```
ai-mate --tts opentts --model "llama3.2:3b" --language ru
ai-mate --model "llama3.2:3b" --language zh
ai-mate --llm llama-server --language it
ai-mate --language es --whisper-model-path ~/.whisper-models/ggml-medium-q5_0.bin`
```

If you want to use OpenTTS, start the docker service first: `docker run --rm --platform=linux/amd64 -p 5500:5500 synesthesiam/opentts:all` (it will pull the image the first time). Adjust the platform as needed depending on your hardware. 

If you need help:

```
ai-mate --help
```

## Build ai-mate from source code

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

## Tricks

For conveniance create bash aliases with the options you want to use, example:

```
# English
alias ai-mate_qwen='ai-mate --model "qwen3:30b"'
alias ai-mate_llama='ai-mate --model "llama3:8b"'

# Spanish
alias ai-mate_es_qwen='ai-mate --model "qwen3:30b" --language es'
alias ai-mate_es_llama='ai-mate --model "llama3:8b" --language es'
```

## Useful to know

ai-mate self contains espeak-ng-data, the whisper tiny & small models and kokoro model and voices which will be autoextracted when running ai-mate if they are not found in next locations:

- `~/.ai-mate/espeak-ng-data.tar.gz`
- `~/.whisper-models/ggml-tiny.bin`
- `~/.whisper-models/ggml-small.bin`
- `~/.cache/k/0.onnx`
- `~/.cache/k/0.bin`

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

Have fun o:)
