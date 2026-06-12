"""Constants and settings management."""

import os
import json

BASE_DIR = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SETTINGS_FILE = os.path.join(BASE_DIR, "settings.json")
MODELS_DIR = os.path.join(BASE_DIR, "models")
DB_FILE = os.path.join(BASE_DIR, "calls.db")
LOG_FILE = os.path.join(BASE_DIR, "test-log.txt")

CMD_HOST = "127.0.0.1"
CMD_PORT = 5051

GROQ_MODEL = "llama-3.3-70b-versatile"
GROQ_CHAT_URL = "https://api.groq.com/openai/v1/chat/completions"
DEEPGRAM_API_URL = "https://api.deepgram.com/v1/projects"
SIXTYDB_VOICES_URL = "https://api.60db.ai/myvoices"
PIPER_VOICES_URL = "https://huggingface.co/rhasspy/piper-voices/resolve/v1.0.0"
USER_AGENT = "translator/1.0"

CALL_IDLE_TIMEOUT = 300  # 5 min silence -> auto-close call

# Preferred default voice per language (first medium-quality picked for unlisted)
DEFAULT_VOICES = {
    "ar": "ar_JO-kareem-medium",
    "ca": "ca_ES-upc_ona-medium",
    "cs": "cs_CZ-jirka-medium",
    "da": "da_DK-talesyntese-medium",
    "de": "de_DE-thorsten-medium",
    "el": "el_GR-rapunzelina-low",
    "en": "en_US-ryan-medium",
    "es": "es_ES-sharvard-medium",
    "fa": "fa_IR-amir-medium",
    "fi": "fi_FI-harri-medium",
    "fr": "fr_FR-siwis-medium",
    "hu": "hu_HU-anna-medium",
    "it": "it_IT-riccardo-x_low",
    "ka": "ka_GE-natia-medium",
    "ko": "ko_KR-kss-low",
    "nl": "nl_NL-mls-medium",
    "no": "no_NO-talesyntese-medium",
    "pl": "pl_PL-darkman-medium",
    "pt": "pt_BR-faber-medium",
    "ro": "ro_RO-mihai-medium",
    "ru": "ru_RU-denis-medium",
    "sv": "sv_SE-nst-medium",
    "tr": "tr_TR-dfki-medium",
    "uk": "uk_UA-ukrainian_tts-medium",
    "vi": "vi_VN-vais1000-medium",
    "zh": "zh_CN-huayan-medium",
}

DEFAULT_SETTINGS = {
    "deepgram_api_key": "",
    "sixtydb_api_key": "",
    "groq_api_key": "",
    "tts_outgoing_voice": "",
    "tts_incoming_voice": "",
    "mic_device": "default",
    "speaker_device": "default",
    "meet_input_device": "BlackHole 16ch",
    "meet_output_device": "BlackHole 2ch",
    "endpointing_ms": 300,
    "my_language": "en",
    "their_language": "en",
    # Provider selection (per direction). STT: "deepgram" | "60db". TTS: "piper" | "60db".
    "stt_provider_outgoing": "deepgram",
    "stt_provider_incoming": "deepgram",
    "tts_provider_outgoing": "piper",
    "tts_provider_incoming": "piper",
    # 60db voice ids, used when the matching TTS provider is "60db".
    "tts_60db_voice_outgoing": "",
    "tts_60db_voice_incoming": "",
}


def load_settings():
    if os.path.exists(SETTINGS_FILE):
        with open(SETTINGS_FILE) as f:
            saved = json.load(f)
        return {**DEFAULT_SETTINGS, **saved}
    # First launch -- pre-populate from env vars
    settings = dict(DEFAULT_SETTINGS)
    settings["deepgram_api_key"] = os.environ.get("DEEPGRAM_API_KEY", "")
    settings["sixtydb_api_key"] = os.environ.get("SIXTYDB_API_KEY", "")
    settings["groq_api_key"] = os.environ.get("GROQ_API_KEY", "")
    return settings


def get_sixtydb_key():
    settings = load_settings()
    return settings.get("sixtydb_api_key") or os.environ.get("SIXTYDB_API_KEY", "")


def save_settings_to_file(settings):
    with open(SETTINGS_FILE, "w") as f:
        json.dump(settings, f, indent=2)


def get_groq_key():
    settings = load_settings()
    return settings.get("groq_api_key") or os.environ.get("GROQ_API_KEY", "")
