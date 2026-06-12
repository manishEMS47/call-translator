# Usage Guide

Open **http://127.0.0.1:5050** after running `./run.sh`.

## First Launch

Settings open automatically on first launch. Fill in:

1. **API Keys** — enter your [Deepgram](https://console.deepgram.com) and [Groq](https://console.groq.com) keys, click **Test** to verify. Optionally add a [60db](https://60db.ai) key to use 60db for STT and/or TTS
2. **Languages** — set "My Language" (what you speak) and "Their Language" (what the other person speaks)
3. **Providers** — choose the STT and TTS provider for each direction. STT: Deepgram or 60db. TTS: Piper (local) or 60db. When you pick 60db for TTS, select a 60db voice from the dropdown (loaded from your 60db account)
4. **Voice** — for Piper TTS, pick a local voice per language. One default voice is pre-installed; download more from the dropdown
5. **Audio Devices** — select your mic and speakers. BlackHole devices are configured automatically
6. Click **Save & Restart Engine**

> **Providers are per-direction.** You can, for example, keep Deepgram STT for both sides but switch the incoming TTS to a 60db voice — the pipeline behaves identically regardless of which provider you choose.

## Controls

| Button | What it does |
|--------|-------------|
| **Start / Stop** | Start or stop the translation engine. A call session is recorded between Start and Stop |
| **Mic Out** | Mute/unmute your microphone (outgoing translation) |
| **Mic In** | Mute/unmute incoming audio (their speech) |
| **Tab Audio** | Capture audio from a browser tab instead of BlackHole. Useful when you can't install BlackHole |
| **Monitor** | Play translated audio in the browser so you can hear what the other person will hear |
| **Compact** | Toggle compact chat mode — smaller bubbles, less spacing |
| **Saved** | Filter to show only bookmarked messages |
| **History** | Open call history — view past sessions, generate AI summaries |
| **Export** | Download the current transcript as a text file |
| **Clear** | Clear all messages from the current view |
| **Settings** | Open the settings panel |

## Live Chat

The main area shows a real-time chat with translations:

- **Blue bubbles** (right) — your speech, translated to their language
- **Purple bubbles** (left) — their speech, translated to your language
- **Italic text** below each bubble — the original (untranslated) text
- **Timing** — STT, translation, and TTS latency shown per message
- **Click a bubble** to copy the translated text
- **Star icon** on hover — bookmark a message for later

## Voice Management

In Settings > Voice:

- The dropdown shows **Downloaded** voices at the top, **Available** voices below
- Available voices show their size in MB
- Select an available voice and click the **download button** (arrow icon) to download it
- Click the **play button** to preview any downloaded voice
- When switching to a new language with no downloaded voices, you'll be prompted to download the default voice

## Call History

Click **History** to see past call sessions:

- Each session shows start/end time, languages, and message count
- Click a session to see the full transcript
- **Summary** — generate an AI-powered summary of the call using Groq
- **Delete** — permanently remove a call from history

## Audio Setup for Calls

For Google Meet, Zoom, or any call app:

1. Install [BlackHole](https://existential.audio/blackhole/) (both 2ch and 16ch)
2. In your call app, set **BlackHole 2ch** as microphone
3. In your call app, set **BlackHole 16ch** as speakers
4. Use **Google Chrome** for the call app — Safari has audio limitations

The translator automatically routes audio through BlackHole: your translated speech goes to the call, their speech comes back for translation.

## Keyboard Shortcuts

- Hover over any button to see a tooltip explaining what it does (in your selected language)
