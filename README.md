![ai mate screenshot](https://github.com/DavidValin/ai-mate/raw/main/preview.png)

voice chat with your local ai models from your terminal simply!
See it in action: [Demo](https://www.youtube.com/watch?v=x0RAX3-PLnE)

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

## Installation

Install dependencies:

- Download Docker: `https://docs.docker.com/engine/install` (needed for STT)
- Download Ollama: `https://ollama.com/download` (needed for ai responses)
- Pull an ollama model: `ollama pull llama3.2:3b` (or the model you want to use)
- Download Whisper model: `https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-medium-q5_0.bin?download=true` (needed for TTS).
- (Only Windows) Install Windows Terminal (which supports emojis): `https://apps.microsoft.com/detail/9n0dx20hk701` (use this terminal to run ai-mate)

### Option A - Download a built binary for your operating system

Download from `https://github.com/DavidValin/ai-mate/releases`
Move the binary to a folder in your $PATH so you can use `ai-mate` command anywhere

### Option B - Build ai-mate from source code

Use cross_build.sh script, get help on how to use it:

```
./cross_build.sh -h
```

- * Mac build only works from native MacOS
- * Windows build only works from native Windows (requires https://visualstudio.microsoft.com/visual-cpp-build-tools)

Examples:
```
./cross_build.sh --os linux --arch amd64,arm64
./cross_build.sh --os windows --arch amd64
./cross_build.sh --os macos --arch arm64,amd64
```

The built binaries will be placed under `./dist`

## How to use it

- start ollama: `ollama serve`
- run ai mate: `ai-mate`

Below are the default parameters, which you can override, example:

```
ai-mate \
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
```

If you want to use OpenTTS, start the docker service first: `docker run --rm --platform=linux/amd64 -p 5500:5500 synesthesiam/opentts:all` (it will pull the image the first time). Adjust the platform as needed depending on your hardware. This container contains within all the voices for all languages.

If you need help:

```
ai-mate --help
```

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

Have fun o:)
