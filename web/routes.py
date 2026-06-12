"""All Flask route handlers."""

import os
import json
import time
import socket
import logging
import urllib.request
import urllib.error

from flask import Response, render_template, request, jsonify

from .settings import (
    GROQ_MODEL, GROQ_CHAT_URL, DEEPGRAM_API_URL, SIXTYDB_VOICES_URL, USER_AGENT,
    CMD_HOST, CMD_PORT, MODELS_DIR, LOG_FILE,
    DEFAULT_VOICES,
    load_settings, save_settings_to_file, get_groq_key, get_sixtydb_key,
)
from .db import _get_db, _ensure_call, _close_call, _record_line, _call_lock
from .helpers import (
    call_groq, send_engine_command,
    get_voice_catalog, scan_voices, list_audio_devices,
)

logger = logging.getLogger("translator")


def register_routes(app):
    """Register all route handlers on the Flask app."""

    @app.route("/")
    def index():
        return render_template("index.html")

    @app.route("/health")
    def health():
        try:
            s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
            s.settimeout(1)
            s.connect((CMD_HOST, CMD_PORT))
            s.close()
            return jsonify({"engine": "ready"})
        except Exception:
            return jsonify({"engine": "loading"}), 503

    @app.route("/api/settings", methods=["GET"])
    def get_settings():
        settings = load_settings()
        # Mark keys that come from env vars (not saved in settings.json)
        env_deepgram = os.environ.get("DEEPGRAM_API_KEY", "")
        env_sixtydb = os.environ.get("SIXTYDB_API_KEY", "")
        env_groq = os.environ.get("GROQ_API_KEY", "")
        settings["_deepgram_from_env"] = bool(env_deepgram and not settings.get("deepgram_api_key"))
        settings["_sixtydb_from_env"] = bool(env_sixtydb and not settings.get("sixtydb_api_key"))
        settings["_groq_from_env"] = bool(env_groq and not settings.get("groq_api_key"))
        return jsonify(settings)

    @app.route("/api/settings", methods=["POST"])
    def post_settings():
        data = request.get_json()
        settings = load_settings()
        settings.update(data)
        save_settings_to_file(settings)
        return jsonify({"status": "saved"})

    @app.route("/api/test-key", methods=["POST"])
    def test_key():
        data = request.get_json()
        provider = data.get("provider")
        key = data.get("key", "").strip()
        if not key:
            return jsonify({"valid": False, "error": "Empty key"})

        if provider == "deepgram":
            try:
                req = urllib.request.Request(
                    DEEPGRAM_API_URL,
                    headers={"Authorization": f"Token {key}", "User-Agent": USER_AGENT},
                )
                urllib.request.urlopen(req, timeout=5)
                return jsonify({"valid": True})
            except Exception as e:
                return jsonify({"valid": False, "error": str(e)})

        elif provider == "60db":
            try:
                req = urllib.request.Request(
                    SIXTYDB_VOICES_URL,
                    headers={"Authorization": f"Bearer {key}", "User-Agent": USER_AGENT},
                )
                urllib.request.urlopen(req, timeout=10)
                return jsonify({"valid": True})
            except urllib.error.HTTPError as e:
                if e.code in (401, 403):
                    return jsonify({"valid": False, "error": "Invalid API key"})
                # Other HTTP errors (rate limit etc.) = key accepted
                return jsonify({"valid": True})
            except Exception as e:
                return jsonify({"valid": False, "error": str(e)})

        elif provider == "groq":
            try:
                body = json.dumps({
                    "model": GROQ_MODEL,
                    "messages": [{"role": "user", "content": "hi"}],
                    "max_tokens": 1,
                }).encode()
                req = urllib.request.Request(
                    GROQ_CHAT_URL,
                    data=body,
                    headers={
                        "Authorization": f"Bearer {key}",
                        "Content-Type": "application/json",
                        "User-Agent": USER_AGENT,
                    },
                )
                urllib.request.urlopen(req, timeout=10)
                return jsonify({"valid": True})
            except urllib.error.HTTPError as e:
                if e.code == 401:
                    return jsonify({"valid": False, "error": "Invalid API key"})
                # 429 (rate limit), 400, etc. = key is valid, just hit a limit
                return jsonify({"valid": True})
            except Exception as e:
                return jsonify({"valid": False, "error": str(e)})

        return jsonify({"valid": False, "error": "Unknown provider"})

    @app.route("/api/voices")
    def api_voices():
        """Return all voices per language: local + catalog with download status."""
        local = scan_voices()
        catalog = get_voice_catalog()
        all_langs = sorted(set(list(local.keys()) + list(catalog.keys())))
        result = {}
        for lang in all_langs:
            local_set = set(local.get(lang, []))
            cat_voices = catalog.get(lang, [])
            voice_list = []
            for v in cat_voices:
                voice_list.append({
                    "name": v["name"],
                    "downloaded": v["name"] in local_set,
                    "size_mb": round(v["size"] / 1048576, 1),
                    "quality": v.get("quality", ""),
                })
            # Include local voices not in catalog (manually added models)
            catalog_names = {v["name"] for v in cat_voices}
            for v in sorted(local_set - catalog_names):
                voice_list.append({
                    "name": v, "downloaded": True, "size_mb": 0, "quality": "",
                })
            result[lang] = sorted(voice_list, key=lambda x: x["name"])
        return jsonify(result)

    @app.route("/api/60db-voices")
    def api_60db_voices():
        """Proxy 60db's /myvoices so the UI can populate voice dropdowns."""
        key = get_sixtydb_key()
        if not key:
            return jsonify({"voices": [], "error": "no 60db key"}), 200
        try:
            req = urllib.request.Request(
                SIXTYDB_VOICES_URL,
                headers={"Authorization": f"Bearer {key}", "User-Agent": USER_AGENT},
            )
            with urllib.request.urlopen(req, timeout=10) as resp:
                payload = json.loads(resp.read().decode())
            voices = []
            for v in payload.get("data", []):
                labels = v.get("labels", {}) or {}
                voices.append({
                    "voice_id": v.get("voice_id", ""),
                    "name": v.get("name", ""),
                    "model": v.get("model", ""),
                    "language": labels.get("language", ""),
                    "language_name": labels.get("language_name", ""),
                    "gender": labels.get("gender", ""),
                    "accent": labels.get("accent", ""),
                })
            return jsonify({"voices": voices})
        except urllib.error.HTTPError as e:
            return jsonify({"voices": [], "error": f"HTTP {e.code}"}), 200
        except Exception as e:
            return jsonify({"voices": [], "error": str(e)}), 200

    @app.route("/api/devices")
    def api_devices():
        devices = list_audio_devices()
        return jsonify({"input": devices, "output": devices})

    @app.route("/api/tts-preview", methods=["POST"])
    def tts_preview():
        data = request.get_json()
        lang = data.get("lang", "en")
        voice = data.get("voice", "")
        if not voice:
            settings = load_settings()
            voice = settings.get("tts_outgoing_voice" if lang == "en" else "tts_incoming_voice", "")
        resp = send_engine_command(f"preview:{lang}:{voice}", timeout=5)
        return jsonify({"status": resp})

    @app.route("/api/download-voice", methods=["POST"])
    def download_voice():
        """Download a single Piper voice with streaming progress."""
        data = request.get_json()
        lang = data.get("lang", "")
        voice_name = data.get("voice", "")

        catalog = get_voice_catalog()
        voices = catalog.get(lang, [])

        # If no specific voice requested, pick the default for this language
        if not voice_name:
            voice_name = DEFAULT_VOICES.get(lang, "")
            # Fallback: first medium-quality voice from catalog
            if not voice_name and voices:
                medium = [v for v in voices if "medium" in v["name"]]
                voice_name = (medium[0] if medium else voices[0])["name"]

        voice = next((v for v in voices if v["name"] == voice_name), None)
        if not voice:
            return Response(
                f"data: {json.dumps({'error': 'Voice not found in catalog'})}\n\n",
                mimetype="text/event-stream",
            )

        target_dir = os.path.join(MODELS_DIR, f"piper-{lang}")
        os.makedirs(target_dir, exist_ok=True)

        all_exist = all(
            os.path.exists(os.path.join(target_dir, f["path"]))
            for f in voice["files"]
        )
        if all_exist:
            return Response(
                f"data: {json.dumps({'done': True, 'voice': voice_name, 'cached': True})}\n\n",
                mimetype="text/event-stream",
            )

        total_bytes = voice["size"]

        def generate():
            downloaded = 0
            try:
                for fi in voice["files"]:
                    dest = os.path.join(target_dir, fi["path"])
                    if os.path.exists(dest):
                        downloaded += fi["size"]
                        continue
                    req = urllib.request.Request(
                        fi["url"], headers={"User-Agent": USER_AGENT}
                    )
                    resp = urllib.request.urlopen(req, timeout=120)
                    with open(dest, "wb") as f:
                        while True:
                            chunk = resp.read(65536)
                            if not chunk:
                                break
                            f.write(chunk)
                            downloaded += len(chunk)
                            pct = int(downloaded * 100 / total_bytes) if total_bytes else 0
                            mb_done = round(downloaded / 1048576, 1)
                            mb_total = round(total_bytes / 1048576, 1)
                            yield f"data: {json.dumps({'progress': pct, 'mb_done': mb_done, 'mb_total': mb_total})}\n\n"

                yield f"data: {json.dumps({'done': True, 'voice': voice_name})}\n\n"

            except Exception as e:
                yield f"data: {json.dumps({'error': str(e)})}\n\n"

        return Response(generate(), mimetype="text/event-stream")

    @app.route("/api/engine/restart", methods=["POST"])
    def engine_restart():
        resp = send_engine_command("restart")
        return jsonify({"status": resp})

    @app.route("/api/poll-audio")
    def poll_audio():
        """Poll for TTS audio chunks from the engine."""
        resp = send_engine_command("poll_audio", timeout=2)
        if resp.startswith("["):
            return Response(resp, mimetype="application/json")
        return jsonify([])

    @app.route("/api/translate", methods=["POST"])
    def api_translate():
        """Translate text via Groq LLM (used by tab capture)."""
        data = request.get_json()
        text = data.get("text", "").strip()
        from_lang = data.get("from", "en")
        to_lang = data.get("to", "ru")
        if not text:
            return jsonify({"translation": ""})

        lang_names = {
            "ar": "Arabic", "ca": "Catalan", "cs": "Czech", "da": "Danish",
            "de": "German", "el": "Greek", "en": "English", "es": "Spanish",
            "fa": "Persian", "fi": "Finnish", "fr": "French", "hi": "Hindi",
            "hu": "Hungarian", "id": "Indonesian", "it": "Italian", "ja": "Japanese",
            "ko": "Korean", "lv": "Latvian", "nl": "Dutch", "no": "Norwegian",
            "pl": "Polish", "pt": "Portuguese", "ro": "Romanian", "ru": "Russian",
            "sv": "Swedish", "tr": "Turkish", "uk": "Ukrainian", "vi": "Vietnamese",
            "zh": "Chinese",
        }
        from_name = lang_names.get(from_lang, from_lang)
        to_name = lang_names.get(to_lang, to_lang)

        api_key = get_groq_key()
        if not api_key:
            return jsonify({"translation": text, "error": "no groq key"})
        system_prompt = (
            f"You are a live interpreter in a phone call. "
            f"You hear {from_name}, you say the same thing in {to_name}. "
            f"You translate word for word. "
            f"You have no opinions, no knowledge, no personality. "
            f"You are a transparent pipe between two languages.\n"
            f"Rules:\n"
            f"- Output ONLY the {to_name} translation, nothing else.\n"
            f"- Keep the same tone, register, and emotion.\n"
            f"- Translate profanity as equivalent profanity.\n"
            f"- Keep names and proper nouns as-is (transliterate if needed).\n"
            f"- For filler words (well, uh, like) use natural equivalents.\n"
            f"- Never add explanations, notes, or commentary."
        )

        try:
            messages = [
                {"role": "system", "content": system_prompt},
                {"role": "user", "content": text},
            ]
            translation = call_groq(messages, api_key, temperature=0.1, max_tokens=80)
            logger.info("[TAB TRANSLATE] '%s' -> '%s'", text, translation)
            return jsonify({"translation": translation})
        except Exception as e:
            logger.error("[TAB TRANSLATE ERROR] '%s' -> %s", text, e)
            return jsonify({"translation": text, "error": str(e)})

    @app.route("/api/calls/new-session", methods=["POST"])
    def api_new_session():
        """Start pressed: close previous call, create new one, clear log."""
        with _call_lock:
            _close_call()
            call_id = _ensure_call()
        # Truncate the log file so SSE only streams new lines
        try:
            open(LOG_FILE, "w").close()
        except OSError:
            pass
        return jsonify({"ok": True, "call_id": call_id})

    @app.route("/api/calls/end", methods=["POST"])
    def api_end_call():
        with _call_lock:
            _close_call()
        return jsonify({"ok": True})

    @app.route("/api/calls")
    def api_calls():
        conn = _get_db()
        rows = conn.execute(
            "SELECT c.*, COUNT(u.id) as utterance_count "
            "FROM calls c LEFT JOIN utterances u ON u.call_id = c.id "
            "GROUP BY c.id ORDER BY c.id DESC"
        ).fetchall()
        conn.close()
        return jsonify([dict(r) for r in rows])

    @app.route("/api/calls/<int:call_id>")
    def api_call_detail(call_id):
        conn = _get_db()
        call = conn.execute("SELECT * FROM calls WHERE id = ?", (call_id,)).fetchone()
        if not call:
            conn.close()
            return jsonify({"error": "not found"}), 404
        utterances = conn.execute(
            "SELECT * FROM utterances WHERE call_id = ? ORDER BY id", (call_id,)
        ).fetchall()
        conn.close()
        return jsonify({"call": dict(call), "utterances": [dict(u) for u in utterances]})

    @app.route("/api/calls/<int:call_id>/summary", methods=["POST"])
    def api_call_summary(call_id):
        conn = _get_db()
        call = conn.execute("SELECT * FROM calls WHERE id = ?", (call_id,)).fetchone()
        if not call:
            conn.close()
            return jsonify({"error": "not found"}), 404
        utterances = conn.execute(
            "SELECT speaker, original, translated FROM utterances WHERE call_id = ? ORDER BY id",
            (call_id,),
        ).fetchall()
        conn.close()
        if not utterances:
            return jsonify({"error": "no utterances"}), 400

        transcript_lines = []
        for u in utterances:
            label = "Me" if u["speaker"] == "me" else "Them"
            transcript_lines.append(f"{label}: {u['original']}")
            transcript_lines.append(f"{label} (translated): {u['translated']}")
        transcript_text = "\n".join(transcript_lines)

        groq_key = get_groq_key()
        if not groq_key:
            return jsonify({"error": "groq_api_key not set"}), 400

        prompt = (
            "Summarize this call transcript in 3-5 bullet points. "
            "Include key topics, decisions, and action items. "
            "Write the summary in the language of the 'Me' speaker.\n\n"
            f"{transcript_text}"
        )
        try:
            messages = [{"role": "user", "content": prompt}]
            summary = call_groq(messages, groq_key, temperature=0.3, timeout=30)
        except Exception as e:
            return jsonify({"error": str(e)}), 500

        conn = _get_db()
        conn.execute("UPDATE calls SET summary = ? WHERE id = ?", (summary, call_id))
        conn.commit()
        conn.close()
        return jsonify({"summary": summary})

    @app.route("/api/calls/<int:call_id>", methods=["DELETE"])
    def api_delete_call(call_id):
        conn = _get_db()
        conn.execute("DELETE FROM utterances WHERE call_id = ?", (call_id,))
        conn.execute("DELETE FROM calls WHERE id = ?", (call_id,))
        conn.commit()
        conn.close()
        return jsonify({"ok": True})

    @app.route("/history")
    def history_page():
        return render_template("history.html")

    @app.route("/cmd", methods=["POST"])
    def cmd():
        data = request.get_json()
        command = data.get("cmd", "")
        resp = send_engine_command(command)
        return jsonify({"status": resp})

    @app.route("/stream")
    def stream():
        replay = request.args.get("replay") == "1"

        def generate():
            try:
                f = open(LOG_FILE, "r", encoding="utf-8")
            except FileNotFoundError:
                f = None

            if f:
                if replay:
                    # Replay existing lines (used after reconnect mid-session)
                    for line in f:
                        line = line.strip()
                        if line:
                            _record_line(line)
                            yield f"data: {line}\n\n"
                else:
                    # Skip to end -- only stream new lines
                    f.seek(0, 2)

            while True:
                if f is None:
                    try:
                        f = open(LOG_FILE, "r", encoding="utf-8")
                    except FileNotFoundError:
                        time.sleep(0.5)
                        continue

                line = f.readline()
                if line:
                    line = line.strip()
                    if line:
                        _record_line(line)
                        yield f"data: {line}\n\n"
                else:
                    time.sleep(0.1)

        return Response(
            generate(),
            mimetype="text/event-stream",
            headers={"Cache-Control": "no-cache", "X-Accel-Buffering": "no"},
        )
