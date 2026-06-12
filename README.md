# Realtime Call Translator

[![README на русском](https://img.shields.io/badge/README-Russian-blue)](README.ru.md)

Real-time speech translator for video/voice calls. Translates both sides of the conversation live — you speak your language, the other person hears theirs, and vice versa.

**How it works:** Your mic audio goes through Speech-to-Text, gets translated by an LLM, then synthesized back to speech and routed into your call. The same happens in reverse for the other person's audio.

Supports **29 languages** with STT, translation, and TTS. Voice models from [Piper](https://github.com/rhasspy/piper) — download any language directly from the web UI.

**Pluggable providers:** Speech-to-text can run on **Deepgram** or **60db**, and text-to-speech on local **Piper** or **60db** — selectable per direction (outgoing/incoming) from the web UI Settings. Mix and match freely; the pipeline behaves identically regardless of provider.

![macOS](https://img.shields.io/badge/platform-macOS_14+-lightgrey)
![License](https://img.shields.io/badge/license-MIT-blue)
![GitHub stars](https://img.shields.io/github/stars/LetovKai/call-translator)

> **Note:** macOS only (14+). Uses CoreAudio and cpal for audio capture. Windows/Linux support is not available yet — contributions welcome!

---

## Quick Start

**One-command setup** (macOS with Homebrew):

```bash
git clone https://github.com/LetovKai/call-translator.git
cd call-translator
./setup.sh
```

The script installs all dependencies, downloads voice models for English and Russian, and builds the project.

Then:

```bash
./run.sh
```

Open **http://127.0.0.1:5050** in **Google Chrome**. Settings open automatically on first launch — enter your API keys and configure languages there.

**[Usage Guide (USAGE.md)](USAGE.md)** — controls, voice management, audio setup, call history.

> **Browser:** Use **Chrome** — audio monitor and BlackHole routing work correctly. Safari has audio output limitations that prevent monitor playback. Other browsers are untested.

> You need two free API keys (free tiers available):
> - [Deepgram](https://console.deepgram.com) — speech-to-text
> - [Groq](https://console.groq.com) — translation (LLM)
>
> Optionally add a [60db](https://60db.ai) key to use 60db for speech-to-text and/or text-to-speech instead.

---

## Architecture

```
┌─────────────┐     ┌──────────────────┐     ┌───────────┐     ┌──────────────┐
│  Your Mic   │────>│ STT              │────>│ Groq LLM  │────>│ TTS          │──> Call
│  (your lang)│     │ Deepgram / 60db  │     │ (translate)│     │ Piper / 60db │   (BlackHole)
└─────────────┘     └──────────────────┘     └───────────┘     └──────────────┘

┌─────────────┐     ┌──────────────────┐     ┌───────────┐     ┌──────────────┐
│  Call Audio  │────>│ STT              │────>│ Groq LLM  │────>│ TTS          │──> Speakers
│ (their lang)│     │ Deepgram / 60db  │     │ (translate)│     │ Piper / 60db │
└─────────────┘     └──────────────────┘     └───────────┘     └──────────────┘
```

The STT and TTS provider for each direction is chosen in Settings. Both STT backends stream raw 16 kHz PCM over a WebSocket; both TTS backends return audio at the pipeline rate — so the providers are interchangeable behind a single interface.

- **Elixir** — orchestrator, process supervision, port management
- **Rust** — audio capture/playback, STT streaming (Deepgram / 60db), TTS synthesis (Piper / 60db), translation
- **Flask** — web UI for live transcript, settings, and controls

---

## Requirements

| Dependency | Purpose | Install |
|---|---|---|
| macOS 14+ | CoreAudio for audio I/O | — |
| [Homebrew](https://brew.sh) | Package manager | `/bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"` |
| Elixir | Application runtime | `brew install elixir` |
| Rust | Audio engine | `brew install rustup && rustup-init` |
| Python 3 | Web UI server | `brew install python@3` |
| espeak-ng | TTS phonemization | `brew install espeak-ng` |
| ONNX Runtime | Model inference | `brew install onnxruntime` |
| Flask | Web framework | via venv (see below) |
| [BlackHole](https://existential.audio/blackhole/) | Virtual audio routing | Manual download |
| Xcode CLT | C compiler for Rust | `xcode-select --install` |

**API Keys (free tiers available):**
- [Deepgram](https://console.deepgram.com) — speech-to-text (Nova-3 model)
- [Groq](https://console.groq.com) — translation via llama-3.3-70b
- [60db](https://60db.ai) — *optional*, alternative speech-to-text and/or text-to-speech provider (selectable per direction in Settings)

---

## Manual Installation

If you prefer to install everything step by step instead of using `setup.sh`:

### 1. System packages

```bash
xcode-select --install
brew install elixir rustup espeak-ng onnxruntime python@3
rustup-init -y --default-toolchain stable
source ~/.cargo/env

# Create virtual environment and install Flask
python3 -m venv .venv
source .venv/bin/activate
pip install flask
```

### 2. BlackHole audio driver

Download and install from [existential.audio/blackhole](https://existential.audio/blackhole/).

You need **both**:
- **BlackHole 16ch** — captures audio from your call app (Google Meet, Zoom, etc.)
- **BlackHole 2ch** — sends translated audio back to the call

Setup in your call app (Google Meet, Zoom, etc.):
1. Open the call in **Google Chrome** (not Safari)
2. Set **BlackHole 2ch** as the **microphone** in the call app
3. Set **BlackHole 16ch** as the **speakers** in the call app

> **Note:** Do NOT use a Multi-Output Device — it may cause audio issues. Set BlackHole devices directly in the call app settings.

### 3. Download voice models

TTS voices come from [Piper](https://github.com/rhasspy/piper). The setup script downloads English and Russian voices automatically. Additional voices can be downloaded from the web UI — select a language and click the download button.

To download manually:

```bash
mkdir -p models/piper-en models/piper-ru

# English (default)
curl -sL https://huggingface.co/rhasspy/piper-voices/resolve/v1.0.0/en/en_US/ryan/medium/en_US-ryan-medium.onnx \
  -o models/piper-en/en_US-ryan-medium.onnx
curl -sL https://huggingface.co/rhasspy/piper-voices/resolve/v1.0.0/en/en_US/ryan/medium/en_US-ryan-medium.onnx.json \
  -o models/piper-en/en_US-ryan-medium.onnx.json

# Russian (default)
curl -sL https://huggingface.co/rhasspy/piper-voices/resolve/v1.0.0/ru/ru_RU/denis/medium/ru_RU-denis-medium.onnx \
  -o models/piper-ru/ru_RU-denis-medium.onnx
curl -sL https://huggingface.co/rhasspy/piper-voices/resolve/v1.0.0/ru/ru_RU/denis/medium/ru_RU-denis-medium.onnx.json \
  -o models/piper-ru/ru_RU-denis-medium.onnx.json
```

Browse all available voices at [rhasspy.github.io/piper-samples](https://rhasspy.github.io/piper-samples/).

### 4. Environment variables

```bash
cp .env.example .env
```

Edit `.env`:

```
DEEPGRAM_API_KEY=your_key_here
GROQ_API_KEY=your_key_here
# Optional — only needed if you select a 60db STT or TTS provider in Settings
SIXTYDB_API_KEY=your_key_here
ORT_DYLIB_PATH=/opt/homebrew/lib/libonnxruntime.dylib
```

> API keys can also be entered (and tested) directly in the web UI Settings — `.env` is just a convenience for pre-filling them.

### 5. Build

```bash
mix deps.get
mix compile    # Compiles Elixir + Rust (first build takes a few minutes)
```

### 6. Run

```bash
./run.sh
```

Open **http://127.0.0.1:5050** in Chrome.

---

## Web UI Features

- **Live transcript** — chat-style bubbles with original text and translation
- **29 languages** — switch language pair from Settings, download voices with one click
- **Provider selection** — choose Deepgram or 60db for STT, and Piper or 60db for TTS, independently per direction
- **Voice selection** — multiple Piper voices per language with preview playback; 60db voices loaded from your account
- **Audio monitor** — hear translations in your browser (Chrome only)
- **Start/Stop** — control the engine without restarting
- **Mute** — independently mute outgoing or incoming pipelines
- **Bookmarks** — star important phrases, filter to show only starred
- **Export** — download the full transcript as a text file
- **Compact/Full view** — toggle between detailed and compact transcript
- **Latency metrics** — per-phrase STT, translation, TTS, and total latency
- **Dark/Light theme** — toggle with persistence

---

## Supported Languages

| Language | STT | Translation | TTS |
|----------|-----|-------------|-----|
| Arabic | + | + | + |
| Catalan | + | + | + |
| Chinese | + | + | + |
| Czech | + | + | + |
| Danish | + | + | + |
| Dutch | + | + | + |
| English | + | + | + |
| Finnish | + | + | + |
| French | + | + | + |
| German | + | + | + |
| Greek | + | + | + |
| Hindi | + | + | + |
| Hungarian | + | + | + |
| Indonesian | + | + | + |
| Italian | + | + | + |
| Japanese | + | + | — |
| Korean | + | + | — |
| Latvian | + | + | + |
| Norwegian | + | + | + |
| Persian | + | + | + |
| Polish | + | + | + |
| Portuguese | + | + | + |
| Romanian | + | + | + |
| Russian | + | + | + |
| Spanish | + | + | + |
| Swedish | + | + | + |
| Turkish | + | + | + |
| Ukrainian | + | + | + |
| Vietnamese | + | + | + |

TTS requires downloading a Piper voice model for the language (one-click from the web UI). Japanese and Korean have STT and translation but no Piper TTS voice available.

---

## Troubleshooting

**"Engine not starting"**
- Check that `.env` has valid API keys
- Verify `ORT_DYLIB_PATH` points to your onnxruntime library
- Run `mix compile` to check for build errors

**"No audio from call"**
- Ensure BlackHole 16ch is set up in a Multi-Output Device
- Check that your call app uses BlackHole 2ch as its microphone

**"TTS not working"**
- Verify `espeak-ng` is installed: `espeak-ng --version`
- Check that voice model files exist in `models/piper-{lang}/`
- Download voices from Settings in the web UI

**"No sound in monitor"**
- Use Chrome — Safari does not support audio output routing required for monitor
- Check your system audio output is set to speakers (not BlackHole)

**"Groq key shows invalid"**
- The key is likely valid — test by clicking "Test" in Settings
- Keys set via `.env` work automatically even if the UI field is empty

---

## License

MIT
