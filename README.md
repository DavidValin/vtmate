![ai mate screenshot](https://github.com/DavidValin/ai-mate/raw/main/preview.png)

ai mate is a terminal based audio conversation system between a user and an AI model that runs locally in your machine.

- llm system: ollama
- speech to text (stt): whisper.cpp
- text to speech (tts): OpenTTS

## How it works

`RECORD -> STT -> LLM -> REPLY -> TTS -> PLAYBACK`

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
- Download Whisper.cpp: `https://github.com/ggml-org/whisper.cpp`, see 'Quick Start' (needed for TTS)
- Download Whisper model (stt): `https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-q5_0.bin?download=true`
- (Only Windows) Install Windows Terminal (which supports emojis): `https://apps.microsoft.com/detail/9n0dx20hk701` (use this terminal to run ai-mate)
- (Only MacOS / Linux) Install `pkg-config` and alsa development libraries (called `libasound2-dev` or `alsa-lib-devel` or `alsa-lib`)

### Option A - Download a built binary for your operating system

Download from `https://github.com/DavidValin/ai-mate/releases`
Move the binary to a folder in your $PATH so you can use `ai-mate` command anywhere

### Option B - Build ai-mate from source

If you have the ai-mate source code locally:

```
cargo build --release
cargo install --path .
```

Otherwise fetch, build and install it using cargo:

```
 `cargo install ai-mate`
```

The `ai-mate` program will be under `~/.cargo/bin`. Make sure this directory is added to your $PATH, otherwise add it.

## How to use it

Before starting, make sure ollama and OpenTTS are both running and your microphone and speakers ready:

- `ollama serve`
- `docker run --rm --platform=linux/amd64 -p 5500:5500 synesthesiam/opentts:all` (it will pull the image the first time). Adjust the platform as needed depending on your hardware. This container contains within all the voices for all languages.

To start the conversation follow this instructions:

Below are the default parameters, which you can override, example:

```
ai-mate \
  --language en \
  --sound-threshold-peak 0.10 \
  --end-silence-ms 850 \
  --whisper-model-path "$HOME/.whisper-models/ggml-large-v3-q5_0.bin" \
  --ollama-url "http://localhost:11434/api/generate" \
  --ollama-model "llama3.2:3b" \
  --opentts-base-url "http://0.0.0.0:5500/api/tts?vocoder=high&denoiserStrength=0.005&&speakerId=&ssml=false&ssmlNumbers=true&ssmlDates=true&ssmlCurrency=true&cache=false"
```

You can just override a specific variable, for example:

```
ai-mate --ollama-model "llama3.2:3b" --language es --whisper-model-path "$HOME/ggml-medium.bin"
```

If you need help:

```
ai-mate --help
```

## Language support

By default everything run in english (speech recognition and audio playback). The next languages are supported:

```
ID         LANGUAGE              DEFAULT VOICE
____________________________________________________________
ar         arabic                festival:ara_norm_ziad_hts                 
bn         bengali               flite:cmu_indic_ben_rm
ca         catalan               festival:upc_ca_ona_hts
cs         czech                 festival:czech_machac
de         german                glow-speak:de_thorsten
el         greek                 glow-speak:el_rapunzelina
en         english               larynx:cmu_fem-glow_tts
es         spanish               larynx:karen_savage-glow_tts
fi         finnish               glow-speak:fi_harri_tapani_ylilammi
fr         french                larynx:gilles_le_blanc-glow_tts
gu         gujarati              flite:cmu_indic_guj_ad
hi         hindi                 flite:cmu_indic_hin_ab
hu         hungarian             glow-speak:hu_diana_majlinger
it         italian               larynx:riccardo_fasol-glow_tts
ja         japanese              coqui-tts:ja_kokoro
kn         kannada               flite:cmu_indic_kan_plv
ko         korean                glow-speak:ko_kss
mr         marathi               flite:cmu_indic_mar_aup
nl         dutch                 glow-speak:nl_rdh
pa         punjabi               flite:cmu_indic_pan_amp
ru         russian               glow-speak:ru_nikolaev
sv         swedish               glow-speak:sv_talesyntese
sw         swahili               glow-speak:sw_biblia_takatifu
ta         tamil                 flite:cmu_indic_tam_sdr
te         telugu                marytts:cmu-nk-hsmm
tr         turkish               marytts:dfki-ot-hsmm
zh         mandarin chinese      coqui-tts:zh_baker
```

## Tricks

For conveniance create bash aliases with the options you want to use, example:

```
# English
alias ai-mate_qwen='ai-mate --ollama-model "qwen3:30b" --whisper-model-path "$HOME/ggml-medium.bin"'
alias ai-mate_llama='ai-mate --ollama-model "llama3:8b" --whisper-model-path "$HOME/ggml-medium.bin"'

# Spanish
alias ai-mate_es_qwen='ai-mate --ollama-model "qwen3:30b" --language es --whisper-model-path "$HOME/ggml-medium.bin"'
alias ai-mate_es_llama='ai-mate --ollama-model "llama3:8b" --language es --whisper-model-path "$HOME/ggml-medium.bin"'
```

Have fun o:)
