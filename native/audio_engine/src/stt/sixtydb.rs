/// Speech-to-text via the 60db streaming WebSocket API.
///
/// Mirrors the Deepgram backend: streams raw 16 kHz PCM up over a persistent
/// non-blocking WebSocket and returns finalized utterances. Audio is sent as
/// base64-encoded LINEAR16 PCM inside JSON `audio` frames (the "browser PCM"
/// mode); the server replies with `transcription` events carrying `is_final`
/// and `speech_final` flags.
///
/// Docs: https://docs.60db.ai/websocket-api/stt

use std::io::ErrorKind;
use std::time::Instant;

use anyhow::{bail, Context, Result};
use base64::Engine as _;
use log::{debug, info, warn};
use serde::Deserialize;
use serde_json::json;
use tungstenite::client::IntoClientRequest;
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{connect, Message, WebSocket};

use super::{f32_to_pcm16_le, set_nonblocking, SttResult, SttSession};

/// 60db requires utterance_end_ms >= 300.
const MIN_UTTERANCE_END_MS: u32 = 300;

// ---------------------------------------------------------------------------
// SixtyDbStt — config holder, creates sessions
// ---------------------------------------------------------------------------

pub struct SixtyDbStt {
    api_key: String,
    language: String,
    /// Silence (ms) after speech before an utterance is finalized.
    utterance_end_ms: u32,
}

impl SixtyDbStt {
    pub fn new(api_key: String, language: String, endpointing_ms: u32) -> Self {
        Self {
            api_key,
            language,
            utterance_end_ms: endpointing_ms.max(MIN_UTTERANCE_END_MS),
        }
    }

    /// Open a WebSocket session to 60db and send the `start` config message.
    /// `sample_rate` is the rate of audio you'll send (after downsampling).
    pub fn create_session(&self, sample_rate: u32) -> Result<SixtyDbSession> {
        // 60db authenticates via the apiKey query parameter.
        let url = format!("wss://api.60db.ai/ws/stt?apiKey={}", self.api_key);
        let request = url
            .into_client_request()
            .context("Failed to build 60db STT request")?;

        info!(
            "Connecting to 60db STT (lang={}, {}Hz, utterance_end={}ms)...",
            self.language, sample_rate, self.utterance_end_ms
        );

        let (mut ws, _) = connect(request).context("Failed to connect to 60db STT WebSocket")?;

        // Send the start/config message while still blocking so it flushes.
        let start = json!({
            "type": "start",
            "languages": [self.language],
            "config": {
                "encoding": "linear",
                "sample_rate": sample_rate,
                "continuous_mode": true,
                "utterance_end_ms": self.utterance_end_ms,
            }
        });
        ws.send(Message::Text(start.to_string()))
            .context("Failed to send 60db start message")?;

        // Non-blocking so we can poll without blocking the audio loop.
        set_nonblocking(&mut ws)?;

        info!("60db STT session connected");
        Ok(SixtyDbSession {
            ws,
            audio_sent_secs: 0.0,
            last_send_time: Instant::now(),
            sample_rate,
        })
    }
}

// ---------------------------------------------------------------------------
// SixtyDbSession — active WebSocket connection
// ---------------------------------------------------------------------------

pub struct SixtyDbSession {
    ws: WebSocket<MaybeTlsStream<std::net::TcpStream>>,
    /// Total seconds of audio sent (accumulated from sample count + rate).
    audio_sent_secs: f64,
    /// Instant when the latest audio chunk was sent.
    last_send_time: Instant,
    /// Sample rate of audio being sent.
    sample_rate: u32,
}

impl SttSession for SixtyDbSession {
    /// Send audio samples (f32 mono) as base64 LINEAR16 PCM in a JSON frame.
    fn send_audio(&mut self, samples: &[f32]) -> Result<()> {
        let bytes = f32_to_pcm16_le(samples);
        let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
        let frame = json!({
            "type": "audio",
            "audio": b64,
            "encoding": "linear",
            "sample_rate": self.sample_rate,
        });

        match self.ws.send(Message::Text(frame.to_string())) {
            Ok(()) => {
                self.audio_sent_secs += samples.len() as f64 / self.sample_rate as f64;
                self.last_send_time = Instant::now();
                Ok(())
            }
            Err(tungstenite::Error::Io(e)) if e.kind() == ErrorKind::WouldBlock => {
                // Non-blocking socket buffer full — drop this chunk silently.
                Ok(())
            }
            Err(e) => Err(anyhow::anyhow!("Failed to send audio to 60db: {}", e)),
        }
    }

    /// Poll for a finalized utterance. Non-blocking — returns `Ok(None)` when
    /// no finalized transcript is available yet.
    ///
    /// Finalization mirrors Deepgram's low-latency behavior: emit on the first
    /// `is_final` result, but when 60db's two-phase refinement is in play
    /// (`is_final:true, speech_final:false` followed by `speech_final:true`),
    /// skip the intermediate dict-corrected pass and wait for the canonical
    /// `speech_final` text so we never emit the same utterance twice.
    fn poll_transcript(&mut self) -> Result<Option<SttResult>> {
        loop {
            match self.ws.read() {
                Ok(Message::Text(text)) => {
                    debug!("60db: {}", &text[..text.len().min(200)]);
                    match serde_json::from_str::<SdbMessage>(&text) {
                        Ok(msg) => {
                            if let Some(result) = self.handle_message(&msg) {
                                return Ok(Some(result));
                            }
                        }
                        Err(e) => debug!("60db parse error: {}", e),
                    }
                }
                Ok(Message::Binary(_)) | Ok(_) => {}
                Err(tungstenite::Error::Io(e)) if e.kind() == ErrorKind::WouldBlock => {
                    return Ok(None);
                }
                Err(e) => bail!("60db WebSocket error: {}", e),
            }
        }
    }

    fn close(&mut self) {
        let _ = self.ws.send(Message::Text(json!({"type": "stop"}).to_string()));
        let _ = self.ws.close(None);
    }
}

impl SixtyDbSession {
    /// Turn a parsed message into a finalized [`SttResult`], or `None` if it's
    /// an interim/control message we should keep polling past.
    fn handle_message(&self, msg: &SdbMessage) -> Option<SttResult> {
        // Surface server-side errors without killing the pipeline.
        if msg.msg_type.as_deref() == Some("error") {
            if let Some(m) = &msg.message {
                warn!("60db STT error: {}", m);
            }
            return None;
        }

        if msg.is_final != Some(true) {
            return None;
        }
        // Skip the intermediate pass of two-phase refinement.
        if msg.speech_final == Some(false) {
            return None;
        }

        let transcript = msg.text.clone().unwrap_or_default();
        if transcript.trim().is_empty() {
            return None;
        }

        // STT latency: how far behind real-time is 60db? Use the last word's
        // end time as the utterance end (when available), else fall back to the
        // time since the last audio send. Matches the Deepgram metric's meaning.
        let utterance_end_secs = msg
            .words
            .as_ref()
            .and_then(|w| w.last())
            .and_then(|w| w.end)
            .unwrap_or(self.audio_sent_secs);
        let backlog_secs = self.audio_sent_secs - utterance_end_secs;
        let since_last_send_ms = self.last_send_time.elapsed().as_millis() as u64;
        let stt_latency_ms = (backlog_secs * 1000.0).max(0.0) as u64 + since_last_send_ms;

        info!("60db is_final: '{}' (stt={}ms)", transcript, stt_latency_ms);
        Some(SttResult {
            text: transcript,
            stt_latency_ms,
        })
    }
}

// ---------------------------------------------------------------------------
// 60db response types
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct SdbMessage {
    #[serde(rename = "type")]
    msg_type: Option<String>,
    text: Option<String>,
    is_final: Option<bool>,
    speech_final: Option<bool>,
    words: Option<Vec<SdbWord>>,
    /// Present on `error` messages.
    message: Option<String>,
}

#[derive(Deserialize)]
struct SdbWord {
    end: Option<f64>,
}
