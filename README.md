![ai mate screenshot](https://github.com/DavidValin/ai-mate/raw/main/preview.png)

voice chat with your local ai models from your terminal simply!
See it in action: [Demo](https://www.youtube.com/watch?v=x0RAX3-PLnE)

### Status

- ✅ First beta released. Currently under heavy development
- ✅ Tested in MacOS
- ✅ Tested in Linux
- ⚠️ Windows version not ready yet

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

- Voice interrupt: `the agent stops talking if you interrupt via voice`
- Pause / resume: `press "<CONTROL> + <ALT> + p" to pause voice recording / resume. Useful to it running it during the day and switch it on when needed`
- Voice speed change: `change the agent voice speed by pressing <ARROW_UP> / <ARROW_DOWN>. Do this before asking anything new`
- Integrated `whisper`
- Integrated `kokoro TTS` system
- Interface with `OpenTTS` system
- Supports `ollama` or `llama-server` or `llamafile`
- 28 languages supported (`ai-mate --list-voices`)
- Use any gguf model from huggingface.com or ollama models (small models reply faster)

## LLM engine support

- ✅ ollama (default)
- ✅ llama-server / llamafile

You can run the models locally (by default) or remotely by configuring the base urls via cli option.

## TTS engine support

- ✅ kokoro tts (default and integrated)
- ✅ OpenTTS (requires external service)

## Installation

### 📌 1. **Download ai-mate**
- `https://github.com/DavidValin/ai-mate/releases`
- Move the binary to a folder in your $PATH so you can use `ai-mate` command anywhere

### 📌 2. **Download whisper model**
- Download model, example: `https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-medium-q5_0.bin?download=true`
- Place the model under  `~/.whisper-models/ggml-medium-q5_0.bin`

### 📌 3. **Install llm engine (needed for ai responses)**

Option A- ollama (the default)
- Install `https://ollama.com/download`.
- Pull the model you want to use with ai-mate, for instance: `ollama pull llama3.2:3b`.

Option B- llamafile support
- Download a llamafile `https://huggingface.co/mozilla-ai/Meta-Llama-3-8B-Instruct-llamafile/blob/main/Meta-Llama-3-8B-Instruct.Q8_0.llamafile` (this contains an ai model and the server in a single file).
- Once downloaded, if in windows `rename the .llamafile to .exe`; in linux / mac `chmod +x Meta-Llama-3-8B-Instruct.Q8_0.llamafile`.

Option C- llama-server support.
- Install llama.cpp: `https://github.com/ggml-org/llama.cpp`.
- Download a gguf model: `https://huggingface.co/QuantFactory/Meta-Llama-3-8B-Instruct-GGUF/resolve/main/Meta-Llama-3-8B-Instruct.Q8_0.gguf?download=true`.

### 📌 4. **(Windows only) Install supported terminal**

- Install Windows Terminal (which supports emojis): `https://apps.microsoft.com/detail/9n0dx20hk701` (use this terminal to run ai-mate)

### 5. **(Optional: OpenTTS support)**

- `docker pull synesthesiam/opentts:all`

## How to use it

Default configuration example:

```
ollama serve
ai-mate
```

llamafile example:

```
./Meta-Llama-3-8B-Instruct.Q8_0.llamafile
ai-mate --llm llama-server
```

llama-server example:

```
llama-server -m Meta-Llama-3-8B-Instruct.Q8_0.gguf --jinja -c 100000
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
  --whisper-model-path ~/.whisper-models/ggml-medium-q5_0.bin \
  --ollama-model "llama3.2:3b" \
  --ollama-url "http://localhost:11434/api/generate"
```

You can just override a specific variable, for example:

```
ai-mate --tts opentts --ollama-model "llama3.2:3b" --language ru
ai-mate --ollama-model "llama3.2:3b" --language zh
ai-mate --llm llama-server --language it
```

If you want to use OpenTTS, start the docker service first: `docker run --rm --platform=linux/amd64 -p 5500:5500 synesthesiam/opentts:all` (it will pull the image the first time). Adjust the platform as needed depending on your hardware. 

If you need help:

```
ai-mate --help
```


### Build ai-mate from source code

Use cross_build.sh script, get help on how to use it:

```
./cross_build.sh -h
```

* Mac build only works from native MacOS
* Windows build only works from native Windows (requires https://visualstudio.microsoft.com/visual-cpp-build-tools)

Examples:
```
./cross_build.sh --os linux --arch amd64,arm64
./cross_build.sh --os windows --arch amd64
./cross_build.sh --os macos --arch arm64,amd64
```

The built binaries will be placed under `./dist`

## Tricks

For conveniance create bash aliases with the options you want to use, example:

```
# English
alias ai-mate_qwen='ai-mate --ollama-model "qwen3:30b"'
alias ai-mate_llama='ai-mate --ollama-model "llama3:8b"'

# Spanish
alias ai-mate_es_qwen='ai-mate --ollama-model "qwen3:30b" --language es'
alias ai-mate_es_llama='ai-mate --ollama-model "llama3:8b" --language es'
```

## Useful to know

- ai-mate unzips `espeak-ng-data.tar.gz` in ~/.ai-mate directory
- kokoro-tiny autodownloads the models if not found locally under `~/.cache/k`

## Language support

| ID |           Language           |      Support       |        TTS supported          |
|----|------------------------------|--------------------|-------------------------------|
| en |        🇺🇸 English            |  🏆 Best support   |    ✅ Kokoro · ✅ OpenTTS     |
| es |         🇪🇸 Spanish           |  🏆 Best support   |    ✅ Kokoro · ✅ OpenTTS     |
| zh |     🇨🇳 Mandarin Chinese      |  🏆 Best support   |    ✅ Kokoro · ✅ OpenTTS     |
| ja |        🇯🇵 Japanese           |  🏆 Best support   |    ✅ Kokoro · ✅ OpenTTS     |
| pt |       🇵🇹 Portuguese          |  🏆 Best support   |    ✅ Kokoro · ❌ OpenTTS     |
| it |         🇮🇹 Italian           |  🏆 Best support   |    ✅ Kokoro · ✅ OpenTTS     |
| hi |          🇮🇳 Hindi            |  🏆 Best support   |    ✅ Kokoro · ✅ OpenTTS     |
| fr |         🇫🇷 French            |  🏆 Best support   |    ✅ Kokoro · ✅ OpenTTS     |
| ar |          🇸🇦 Arabic           |     Supported      |    ❌ Kokoro · ✅ OpenTTS     |
| bn |         🇧🇩 Bengali           |     Supported      |    ❌ Kokoro · ✅ OpenTTS     |
| ca |         🇪🇸 Catalan           |     Supported      |    ❌ Kokoro · ✅ OpenTTS     |
| cs |          🇨🇿 Czech            |     Supported      |    ❌ Kokoro · ✅ OpenTTS     |
| de |          🇩🇪 German           |     Supported      |    ❌ Kokoro · ✅ OpenTTS     |
| el |          🇬🇷 Greek            |     Supported      |    ❌ Kokoro · ✅ OpenTTS     |
| fi |         🇫🇮 Finnish           |     Supported      |    ❌ Kokoro · ✅ OpenTTS     |
| gu |         🇮🇳 Gujarati          |     Supported      |    ❌ Kokoro · ✅ OpenTTS     |
| hu |        🇭🇺 Hungarian          |     Supported      |    ❌ Kokoro · ✅ OpenTTS     |
| kn |         🇮🇳 Kannada           |     Supported      |    ❌ Kokoro · ✅ OpenTTS     |
| ko |          🇰🇷 Korean           |     Supported      |    ❌ Kokoro · ✅ OpenTTS     |
| mr |         🇮🇳 Marathi           |     Supported      |    ❌ Kokoro · ✅ OpenTTS     |
| nl |          🇳🇱 Dutch            |     Supported      |    ❌ Kokoro · ✅ OpenTTS     |
| pa |         🇮🇳 Punjabi           |     Supported      |    ❌ Kokoro · ✅ OpenTTS     |
| ru |         🇷🇺 Russian           |     Supported      |    ❌ Kokoro · ✅ OpenTTS     |
| sv |         🇸🇪 Swedish           |     Supported      |    ❌ Kokoro · ✅ OpenTTS     |
| sw |        🇰🇪 Swahili            |     Supported      |    ❌ Kokoro · ✅ OpenTTS     |
| ta |          🇮🇳 Tamil            |     Supported      |    ❌ Kokoro · ✅ OpenTTS     |
| te |         🇮🇳 Telugu            |     Supported      |    ❌ Kokoro · ✅ OpenTTS     |
| tr |         🇹🇷 Turkish           |     Supported      |    ❌ Kokoro · ✅ OpenTTS     |

Have fun o:)
