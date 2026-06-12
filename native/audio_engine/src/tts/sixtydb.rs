/// Text-to-speech via the 60db streaming WebSocket API.
///
/// Holds a persistent WebSocket connection. Each `synthesize` call opens a
/// short-lived context, streams the text in, flushes, and collects the
/// returned LINEAR16 PCM chunks into f32 samples — a blocking request/response
/// that matches the pipeline's synchronous TTS contract (Piper behaves the
/// same way). Reconnects automatically if the socket drops.
///
/// Docs: https://docs.60db.ai/websocket-api/tts

use std::net::TcpStream;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use base64::Engine as _;
use log::{debug, info, warn};
use serde::Deserialize;
use serde_json::json;
use tungstenite::client::IntoClientRequest;
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{connect, Message, WebSocket};

use super::TtsSynthesizer;

/// Sample rates 60db's LINEAR16 encoding supports.
const SUPPORTED_RATES: [u32; 4] = [8000, 16000, 24000, 48000];

/// Max time to wait for a single synthesis exchange before giving up.
const READ_TIMEOUT: Duration = Duration::from_secs(15);

type Ws = WebSocket<MaybeTlsStream<TcpStream>>;

pub struct SixtyDbTts {
    api_key: String,
    voice_id: String,
    /// Sample rate the pipeline expects back.
    output_sample_rate: u32,
    /// Rate we actually request from 60db (nearest supported), resampled to
    /// `output_sample_rate` if they differ.
    request_rate: u32,
    speed: f32,
    stability: f32,
    similarity: f32,
    /// Persistent connection, lazily (re)established.
    ws: Option<Ws>,
    /// Monotonic counter for unique context ids.
    ctx_counter: u64,
}

impl SixtyDbTts {
    /// Create a 60db TTS synthesizer. `voice_id` must be a valid 60db voice.
    pub fn new(api_key: String, voice_id: String, output_sample_rate: u32) -> Result<Self> {
        if voice_id.trim().is_empty() {
            bail!("60db TTS requires a voice_id (set the 60db voice in Settings)");
        }
        let request_rate = nearest_supported_rate(output_sample_rate);
        info!(
            "Initializing 60db TTS: voice_id={}, request_rate={}Hz, output_rate={}Hz",
            voice_id, request_rate, output_sample_rate
        );
        let mut me = Self {
            api_key,
            voice_id,
            output_sample_rate,
            request_rate,
            speed: 1.0,
            stability: 50.0,
            similarity: 75.0,
            ws: None,
            ctx_counter: 0,
        };
        // Connect eagerly so config errors (bad key) surface at startup.
        me.connect()?;
        info!("60db TTS ready");
        Ok(me)
    }

    /// Establish the WebSocket connection and read the initial
    /// `connection_established` handshake.
    fn connect(&mut self) -> Result<()> {
        let url = format!("wss://api.60db.ai/ws/tts?apiKey={}", self.api_key);
        let request = url
            .into_client_request()
            .context("Failed to build 60db TTS request")?;
        let (mut ws, _) = connect(request).context("Failed to connect to 60db TTS WebSocket")?;
        set_read_timeout(&mut ws, READ_TIMEOUT)?;
        // Drain the initial handshake message (best-effort).
        if let Ok(Message::Text(t)) = ws.read() {
            debug!("60db TTS handshake: {}", &t[..t.len().min(200)]);
        }
        self.ws = Some(ws);
        Ok(())
    }

    /// Run one synthesis exchange on the live connection.
    fn synthesize_once(&mut self, text: &str) -> Result<Vec<f32>> {
        self.ctx_counter += 1;
        let context_id = format!("ctx-{}", self.ctx_counter);
        let ws = self.ws.as_mut().context("60db TTS not connected")?;

        // 1. create_context
        ws.send(Message::Text(
            json!({
                "create_context": {
                    "context_id": context_id,
                    "voice_id": self.voice_id,
                    "audio_config": {
                        "audio_encoding": "LINEAR16",
                        "sample_rate_hertz": self.request_rate,
                    },
                    "speed": self.speed,
                    "stability": self.stability,
                    "similarity": self.similarity,
                }
            })
            .to_string(),
        ))
        .context("60db create_context send failed")?;

        // 2. send_text + 3. flush_context
        ws.send(Message::Text(
            json!({"send_text": {"context_id": context_id, "text": text}}).to_string(),
        ))
        .context("60db send_text failed")?;
        ws.send(Message::Text(
            json!({"flush_context": {"context_id": context_id}}).to_string(),
        ))
        .context("60db flush_context failed")?;

        // 4. Collect audio_chunk frames until flush_completed.
        let mut pcm: Vec<u8> = Vec::new();
        loop {
            match ws.read() {
                Ok(Message::Text(t)) => {
                    let msg: SdbTtsMessage = match serde_json::from_str(&t) {
                        Ok(m) => m,
                        Err(e) => {
                            debug!("60db TTS parse error: {}", e);
                            continue;
                        }
                    };
                    if let Some(err) = msg.error {
                        bail!("60db TTS error: {}", err.message.unwrap_or_default());
                    }
                    if let Some(chunk) = msg.audio_chunk {
                        if let Some(b64) = chunk.audio_content {
                            let bytes = base64::engine::general_purpose::STANDARD
                                .decode(b64.as_bytes())
                                .context("60db audio_chunk base64 decode failed")?;
                            pcm.extend_from_slice(&bytes);
                        }
                    }
                    if msg.flush_completed.is_some() {
                        break;
                    }
                }
                Ok(Message::Binary(b)) => pcm.extend_from_slice(&b),
                Ok(Message::Close(_)) => bail!("60db TTS connection closed mid-synthesis"),
                Ok(_) => {}
                Err(e) => bail!("60db TTS read failed: {}", e),
            }
        }

        // Best-effort close_context (ignore failures — context auto-expires).
        let _ = ws.send(Message::Text(
            json!({"close_context": {"context_id": context_id}}).to_string(),
        ));

        let samples = pcm16_le_to_f32(&pcm);
        if self.request_rate == self.output_sample_rate {
            Ok(samples)
        } else {
            Ok(resample_linear(
                &samples,
                self.request_rate,
                self.output_sample_rate,
            ))
        }
    }
}

impl TtsSynthesizer for SixtyDbTts {
    fn synthesize(&mut self, text: &str) -> Result<Vec<f32>> {
        let text = text.trim();
        if text.is_empty() {
            return Ok(Vec::new());
        }
        if self.ws.is_none() {
            self.connect()?;
        }
        match self.synthesize_once(text) {
            Ok(samples) => Ok(samples),
            Err(e) => {
                // Drop the (likely dead) connection so the next call reconnects.
                warn!("60db TTS synthesis failed, will reconnect: {:#}", e);
                self.ws = None;
                Err(e)
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Response types
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct SdbTtsMessage {
    audio_chunk: Option<AudioChunk>,
    flush_completed: Option<serde_json::Value>,
    error: Option<SdbTtsError>,
}

#[derive(Deserialize)]
struct AudioChunk {
    #[serde(rename = "audioContent")]
    audio_content: Option<String>,
}

#[derive(Deserialize)]
struct SdbTtsError {
    message: Option<String>,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Pick the supported 60db sample rate closest to `target`.
fn nearest_supported_rate(target: u32) -> u32 {
    SUPPORTED_RATES
        .into_iter()
        .min_by_key(|&r| (r as i64 - target as i64).abs())
        .unwrap_or(48000)
}

/// Decode little-endian i16 PCM bytes to f32 samples in [-1, 1].
fn pcm16_le_to_f32(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(2)
        .map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0)
        .collect()
}

/// Linear-interpolation resample for mono f32 audio.
fn resample_linear(samples: &[f32], from_rate: u32, to_rate: u32) -> Vec<f32> {
    if from_rate == to_rate || samples.is_empty() {
        return samples.to_vec();
    }
    let ratio = from_rate as f64 / to_rate as f64;
    let output_len = (samples.len() as f64 / ratio) as usize;
    (0..output_len)
        .map(|i| {
            let src = i as f64 * ratio;
            let idx = src as usize;
            let frac = (src - idx as f64) as f32;
            if idx + 1 < samples.len() {
                samples[idx] * (1.0 - frac) + samples[idx + 1] * frac
            } else {
                samples[idx.min(samples.len() - 1)]
            }
        })
        .collect()
}

/// Set a read timeout on the underlying TCP socket so a stalled synthesis
/// fails instead of hanging the processor thread forever.
fn set_read_timeout(ws: &mut Ws, timeout: Duration) -> Result<()> {
    match ws.get_mut() {
        MaybeTlsStream::Plain(s) => s.set_read_timeout(Some(timeout)).context("set_read_timeout (plain)")?,
        MaybeTlsStream::NativeTls(s) => s
            .get_ref()
            .set_read_timeout(Some(timeout))
            .context("set_read_timeout (tls)")?,
        _ => warn!("Unknown stream type, read timeout not set"),
    }
    Ok(())
}
