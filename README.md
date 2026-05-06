## vtmate

The final AI voice conversational system all running in your terminal! vtmate is a Powerful terminal-based voice ai toolkit with many realistic voices, extremely low latency, 28 languages supported. Allows you to voice conversate with local ai models, pipe data and save into files. 

The program self contains (1.2GB) all TTS models and voices and necessary files to recognize speech and speak with voice with no external intallations ensuring maximum portability.

* [⬇️ Download](https://github.com/DavidValin/vtmate/releases) (⭐ MacOS ⭐ Linux and ⭐ Windows supported)
* [🤠 Quicksheet (PDF)](https://raw.githubusercontent.com/DavidValin/vtmate/refs/heads/main/docs/en/quicksheet.pdf) (🖨️ print ready for easy access)
* [🎥 Video Overview](https://www.youtube.com/watch?v=TfNcgVsR3oc)

### Video demonstration
<details>
<summary>(🇬🇧 English) Conversation mode demo</summary>

https://github.com/user-attachments/assets/8baef926-59dd-4887-b51c-b64efc885fb2

</details>
<details>
<summary>(🇬🇧 English) Debate mode demo</summary>

https://github.com/user-attachments/assets/063b069a-38aa-472c-b477-7382bb063008

</details>

<details>
<summary>(🇬🇧 English) Reading mode demo</summary>

https://github.com/user-attachments/assets/8b9e982c-ba97-4aeb-8e55-1db6a92bc164

</details>

#### **Sponsor this project**
[![Sponsor vtmate](https://img.shields.io/static/v1?label=Sponsor&message=%E2%9D%A4&logo=GitHub&color=%23fe8e86)](https://github.com/sponsors/DavidValin)

![vtmate screenshot](https://github.com/DavidValin/vtmate/raw/main/preview.png)

![how it works](https://github.com/DavidValin/vtmate/raw/main/docs/en/diagrams/how-it-works.png)

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
- 📌 Integrated `kokoro TTS` and `supersonic 2 TTS` systems (no external intallation required)
- 📌 Interface with `OpenTTS` system (requires external docker service)
- 📌 Use any gguf model from huggingface.com (using llama-server) or any ollama model

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
name = explainer
language = en
tts = supersonic2
voice = F1
voice_speed = 1.1
provider = ollama
baseurl = http://127.0.0.1:11434
model = llama3.2:3b
system_prompt = "You are a helpful AI assistant. Your only funcion is to explain things as simple as possible in no more than 150 words or 450 words if the user asks for a longer explanation."
sound_threshold_peak = 0.12
end_silence_ms = 2500
ptt = true
whisper_model_path = ~/.whisper-models/ggml-tiny.bin
```

* By default all agents are set in `PTT` mode, you have to keep `SPACE` pressed to talk. If you want to use `LIVE` mode, make sure you adjust your microphone levels correctly and adjust `sound_threshold_peak` and `end_silence_ms` settings to your need
* ⚠️ Currently you cannot mix kokoro and supersonic tts systems (pick one).
* Voice mixing is supported for kokoro TTS system only, you can create a voice by mixing 2 kokoro voices by percentage. Example mixing 50% of bm_daniel and 50% of am_puck: set voice name to `bm_daniel.5+am_puck.5`

To see explanation of each field:
```
vtmate --help
```

## How to use it

The first agent defined in `~/vtmate/settings` will always be selected agent when running vtmate, unless `-a <agent_name>` is used.

Before running vtmate make sure ollama is running: `ollama serve`.
Optionally, if you want to use llama.cpp make sure llama-server is running.

All cli options:

```
  -a <agent_name>                       set a specific initial agent
  -p <prompt>                           initialize with a text prompt
  -q                                    quiet mode: produces a single response and exit (requires `-p` or `-i`)
  -i <file.txt>                         initialize with a file prompt
  -i -                                  initialize with prompt from STDIN (runs in quiet mode)
  -s                                    save the conversation to text and audio file in ~/.vtmate/conversations or ~/.vtmate/read-files
  --debate <AGENT1> <AGENT2> [SUBJECT]  initialize a debate between 2 agents with an initial prompt
  --debate <AGENT1> <AGENT2> -i <FILE>  initialize a debate between 2 agents with an initial prompt from file
  --debate <AGENT1> <AGENT2> -i –       initialize a debate between 2 agents with an initial prompt from STDIN
  -r <file.txt>                         read a file with voice, phrase by phrase (no llm involved)
  -r -                                  read text from STDIN with voice, phrase by phrase (no llm involved). Use - for STDIN (runs in quiet mode)
  -c <settings_file>                    use a specific settings file
  --list-voices                         list all voices for all languages and tts systems
  --ptt <true/false>                    override for this session the ptt setting for all agents independently of its settings
  --verbose                             run the program in verbose mode
  --version                             print the vtmate installed version
  --help                                show help
```

For quick reference get the printable [Quicksheet (PDF)](https://raw.githubusercontent.com/DavidValin/vtmate/refs/heads/main/docs/en/quicksheet.pdf)

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
* Be able to save the conversation in a wav and text file by adding `-s` option. It will save it in `~/.vtmate/conversations` folder
* For quick reference get the printable [Quicksheet (PDF)](https://raw.githubusercontent.com/DavidValin/vtmate/refs/heads/main/docs/en/quicksheet.pdf)

### Debate mode

![debate mode](https://github.com/DavidValin/vtmate/raw/main/docs/en/diagrams/debate-mode.png)

Initialize a debate between two agents and be able to participate in the debate by speaking at any time. To create a good debate adjust the system prompts of each agent and give a detailed initial input.
In debate mode is good idea to set `--ptt <true/false>` option so that the ptt value is not switched on each agent turn.

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

* When running in LIVE mode just talk. You can also pause/resume recording by pressing `SPACE` once
* When running in PTT mode: keep `SPACE` pushed while talking, and then release
* Press `SCAPE` **once** during a mid response to cancel it and stop the debate
* Press `SCAPE` **twice** for resetting the session
* Press double `u` to undo last response
* You can also start/stop a debate from conversation mode by pressing `Control+D` and picking the debate agents.
* Be able to save the conversation in a wav and text file by adding `-s` option. It will save it in `~/.vtmate/conversations` folder
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

###  Tools

Available tools:

* `glob`: Search for files using glob patterns like **/*.js or src/**/*.ts
* `grep`: Fast content search across files in a directory using full regex syntax
* `read_file`: Reads specific line ranges from a file
* `apply_patch`: Applies a unified diff patch to a file
* `bash_command`: Executes a bash command on the host system
* `search`: Searches the web for a term (q) and return a list of results (title+url)
* `web_fetch`: Fetches a single web page using a url and returns a JSON containing its content and links

vtmate supports **dynamic HTTP request tools** — custom API call definitions loaded from JSON files. Each definition registers a new tool that the LLM can call.

Create a JSON file in `~/.vtmate/tools/http_requests/` with this structure:

```json
{
  "tool_definition": {
    "name": "get_weather",
    "description": "Get current weather for a city",
    "parameters": {
      "city": {
        "type": "string",
        "description": "City name"
      },
      "units": {
        "type": "string",
        "description": "Metric or imperial",
        "default": "metric"
      }
    }
  },
  "tool_http_handler": {
    "method": "GET",
    "url": "https://api.weather.com/v1/forecast?city=PICK_FROM['city']&units=PICK_FROM['units']",
    "headers": {
      "Authorization": "Bearer your_api_key"
    },
    "body": {}
  }
}
```

The `tool_definition` provides the JSON schema for the LLM tool call. The `tool_http_handler` translates the LLM's call into an actual HTTP request — values can reference call arguments with `PICK_FROM['key']` template syntax. Parameters with a `default` are optional; all others are required.

To enable a tool for an agent, add its name to the `tools` setting in `~/.vtmate/settings`:

```ini
tools = web_fetch, bash_command, search, get_weather
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
| en |   🇬🇧  English            |  🏆 Best support   |    ✅ SS2    ✅ Kokoro    ✅ OpenTTS     | > 38 voices
| es |   🇪🇸  Spanish            |  🏆 Best support   |    ✅ SS2    ✅ Kokoro    ✅ OpenTTS     | > 14 voices
| fr |   🇫🇷  French             |  🏆 Best support   |    ✅ SS2    ✅ Kokoro    ✅ OpenTTS     | > 12 voices
| zh |   🇨🇳  Mandarin Chinese   |  🥈 Good support   |    ❌ SS2    ✅ Kokoro    ✅ OpenTTS     | > 9 voices
| ja |   🇯🇵  Japanese           |  🥈 Good support   |    ❌ SS2    ✅ Kokoro    ✅ OpenTTS     | > 6 voices
| pt |   🇵🇹  Portuguese         |  🥈 Good support   |    ✅ SS2    ✅ Kokoro    ❌ OpenTTS     | > 13 voices
| ko |   🇰🇷  Korean             |  🥈 Good support   |    ✅ SS2    ❌ Kokoro    ✅ OpenTTS     | 11 voices
| it |   🇮🇹  Italian            |  🥈 Good support   |    ❌ SS2    ✅ Kokoro    ✅ OpenTTS     | > 3 voices
| hi |   🇮🇳  Hindi              |  🥈 Good support   |    ❌ SS2    ✅ Kokoro    ✅ OpenTTS     | > 4 voices
| ar |   🇸🇦  Arabic             |     Supported      |    ❌ SS2    ❌ Kokoro    ✅ OpenTTS     | 1 voice
| bn |   🇧🇩  Bengali            |     Supported      |    ❌ SS2    ❌ Kokoro    ✅ OpenTTS     | 1 voice
| ca |   🇪🇸  Catalan            |     Supported      |    ❌ SS2    ❌ Kokoro    ✅ OpenTTS     | 1 voice
| cs |   🇨🇿  Czech              |     Supported      |    ❌ SS2    ❌ Kokoro    ✅ OpenTTS     | 1 voice
| de |   🇩🇪  German             |     Supported      |    ❌ SS2    ❌ Kokoro    ✅ OpenTTS     | 1 voice
| el |   🇬🇷  Greek              |     Supported      |    ❌ SS2    ❌ Kokoro    ✅ OpenTTS     | 1 voice
| fi |   🇫🇮  Finnish            |     Supported      |    ❌ SS2    ❌ Kokoro    ✅ OpenTTS     | 1 voice
| gu |   🇮🇳  Gujarati           |     Supported      |    ❌ SS2    ❌ Kokoro    ✅ OpenTTS     | 1 voice
| hu |   🇭🇺  Hungarian          |     Supported      |    ❌ SS2    ❌ Kokoro    ✅ OpenTTS     | 1 voice
| kn |   🇮🇳  Kannada            |     Supported      |    ❌ SS2    ❌ Kokoro    ✅ OpenTTS     | 1 voice
| mr |   🇮🇳  Marathi            |     Supported      |    ❌ SS2    ❌ Kokoro    ✅ OpenTTS     | 1 voice
| nl |   🇳🇱  Dutch              |     Supported      |    ❌ SS2    ❌ Kokoro    ✅ OpenTTS     | 1 voice
| pa |   🇮🇳  Punjabi            |     Supported      |    ❌ SS2    ❌ Kokoro    ✅ OpenTTS     | 1 voice
| ru |   🇷🇺  Russian            |     Supported      |    ❌ SS2    ❌ Kokoro    ✅ OpenTTS     | 1 voice
| sv |   🇸🇪  Swedish            |     Supported      |    ❌ SS2    ❌ Kokoro    ✅ OpenTTS     | 1 voice
| sw |   🇰🇪  Swahili            |     Supported      |    ❌ SS2    ❌ Kokoro    ✅ OpenTTS     | 1 voice
| ta |   🇮🇳  Tamil              |     Supported      |    ❌ SS2    ❌ Kokoro    ✅ OpenTTS     | 1 voice
| te |   🇮🇳  Telugu             |     Supported      |    ❌ SS2    ❌ Kokoro    ✅ OpenTTS     | 1 voice
| tr |   🇹🇷  Turkish            |     Supported      |    ❌ SS2    ❌ Kokoro    ✅ OpenTTS     | 1 voice

## Acceleration support

Do you have GPU? (nvidia? an apple computer?) Great! then vtmate speed is at lighting speed =)

* To be able to use acceleration, pick the built version for your hardware from [Releases list](https://github.com/DavidValin/vtmate/releases)
* For CUDA install CUDA Toolkit. For Vulkan install VULKAN SDK

```
macOS:            ✅ CPU    ✅ Metal
Linux (amd64):    ✅ CPU    ✅ CUDA     ⚠️ Vulkan
Linux (arm64):    ✅ CPU    ⚠️ CUDA     ❌ Vulkan
Windows (x86_64)  ✅ CPU    ⚠️ CUDA     ⚠️ Vulkan
Windows (arm64)   ❌ CPU    ❌ CUDA     ❌ Vulkan
```

⚠️ Currently working on full static builds for all OS with Openblas + CUDA + Vulkan support. In the meantime, pick a release available from [Releases list](https://github.com/DavidValin/vtmate/releases) or build one yourself.

## Build vtmate from source code

**Simplest way:**
```
cargo install vtmate
```
**From git repository:**
```
git clone https://github.com/DavidValin/vtmate
cargo build --release
```

**Full configurable builds (OS, arch and gpu acceleration)**

see:
```
build_linux.sh
build_macos.sh
build_windows.sh
```

## Testing

Test tools:
```
cargo test \
  --test glob_test \
  --test grep_test \
  --test read_file_test \
  --test  apply_patch_test
```

Test all:
```
cargo test
```


Have fun o:)
