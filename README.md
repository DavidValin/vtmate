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

- Docker: `https://docs.docker.com/engine/install` (needed for STT)
- Ollama: `https://ollama.com/download` (needed for ai responses)
- Whisper.cpp: `https://github.com/ggml-org/whisper.cpp`, see 'Quick Start' (needed for TTS)
- Rust: `https://rustup.rs` (needed to compile ai-mate from source)

Download models:

- llm model: `ollama pull llama3.2:3b` (or the model you want to use)
- whisper model (stt): `https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-q5_0.bin?download=true`

Windows only:

- install the Windows Terminal (that supports emojis): `https://apps.microsoft.com/detail/9n0dx20hk701`

On Linux / MacOS:

- Install alsa development libraries: called `libasound2-dev` or `alsa-lib-devel` or `alsa-lib`
- Install `pkg-config`

Build and install ai-mate:

```
cargo build --release
cargo install --path .
```

This installs the program called `ai-mate` under `~/.cargo/bin`. Make sure this directory is added to your path, otherwise add it.

## How to use it

Before starting, make sure ollama and OpenTTS are running:

- Terminal 1: `ollama serve`
- Terminal 2: `docker run --rm --platform=linux/amd64 -p 5500:5500 synesthesiam/opentts:all` (it will pull the image the first time). Adjust the platform as needed depending on your hardware. This container contains all the voices for all languages.

To start the conversation follow this instructions:

Below is the default parameters, which you can override, example:

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
ai-mate --ollama-model "llama3.2:3b --language es"
```

If you need help:

```
ai-mate --help
```

## Language support

By default everything run in english (speech recognition and audio playback). The next languages are supported:

```
Language ID         DEFAULT VOICE                              LANGUAGE NAME
____________________________________________________________________________

ar                  festival:ara_norm_ziad_hts                 arabic
bn                  flite:cmu_indic_ben_rm                     bengali
ca                  festival:upc_ca_ona_hts                    catalan
cs                  festival:czech_machac                      czech
de                  glow-speak:de_thorsten                     german
el                  glow-speak:el_rapunzelina                  greek
en                  larynx:cmu_fem-glow_tts                    english
es                  larynx:karen_savage-glow_tts               spanish
fi                  glow-speak:fi_harri_tapani_ylilammi        finnish
fr                  larynx:gilles_le_blanc-glow_tts            french
gu                  flite:cmu_indic_guj_ad                     gujarati
hi                  flite:cmu_indic_hin_ab                     hindi
hu                  glow-speak:hu_diana_majlinger              hungarian
it                  larynx:riccardo_fasol-glow_tts             italian
ja                  coqui-tts:ja_kokoro                        japanese
kn                  flite:cmu_indic_kan_plv                    kannada
ko                  glow-speak:ko_kss                          korean
mr                  flite:cmu_indic_mar_aup                    marathi
nl                  glow-speak:nl_rdh                          dutch
pa                  flite:cmu_indic_pan_amp                    punjabi
ru                  glow-speak:ru_nikolaev                     russian
sv                  glow-speak:sv_talesyntese                  swedish
sw                  glow-speak:sw_biblia_takatifu              swahili
ta                  flite:cmu_indic_tam_sdr                    tamil
te                  marytts:cmu-nk-hsmm                        telugu
tr                  marytts:dfki-ot-hsmm                       turkish
zh                  coqui-tts:zh_baker                         mandarin chinese
```

Feel free to contribute using a PR.
Have fun o:)
