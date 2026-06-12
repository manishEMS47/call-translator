/// Speech-to-text via Deepgram Nova-3 streaming WebSocket API.
///
/// Sends raw PCM audio over a persistent WebSocket connection.
/// Deepgram handles VAD/endpointing internally and returns `speech_final`
/// events when an utterance is complete.

use std::io::ErrorKind;
use std::time::Instant;

use anyhow::{bail, Context, Result};
use log::{debug, info};
use serde::Deserialize;
use tungstenite::client::IntoClientRequest;
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{connect, Message, WebSocket};

use super::{f32_to_pcm16_le, set_nonblocking, SttResult, SttSession};

// ---------------------------------------------------------------------------
// DeepgramStt — config holder, creates sessions
// ---------------------------------------------------------------------------

pub struct DeepgramStt {
    api_key: String,
    language: String,
    /// Milliseconds of silence before Deepgram fires speech_final (endpointing).
    endpointing_ms: u32,
}

impl DeepgramStt {
    pub fn new(api_key: String, language: String, endpointing_ms: u32) -> Self {
        // Map our lang codes to Deepgram-compatible codes
        let language = match language.as_str() {
            "pt" => "pt-BR",
            "no" => "nb",     // Norwegian Bokmål
            code => code,
        }.to_string();
        Self { api_key, language, endpointing_ms }
    }

    /// Open a WebSocket session to Deepgram.
    /// `sample_rate` is the rate of audio you'll send (after downsampling).
    pub fn create_session(&self, sample_rate: u32) -> Result<DeepgramSession> {
        let url = format!(
            "wss://api.deepgram.com/v1/listen\
             ?model=nova-3\
             &language={}\
             &encoding=linear16\
             &sample_rate={}\
             &channels=1\
             &interim_results=true\
             &endpointing={}",
            self.language, sample_rate, self.endpointing_ms
        );

        // Build request via into_client_request() so tungstenite adds proper
        // WebSocket handshake headers, then inject the Authorization header on top.
        let mut request = url
            .into_client_request()
            .context("Failed to build Deepgram request")?;
        request.headers_mut().insert(
            "Authorization",
            format!("Token {}", self.api_key)
                .parse()
                .context("Invalid API key header value")?,
        );

        info!(
            "Connecting to Deepgram (lang={}, {}Hz, endpointing={}ms)...",
            self.language, sample_rate, self.endpointing_ms
        );

        let (mut ws, _) = connect(request).context("Failed to connect to Deepgram WebSocket")?;

        // Non-blocking so we can poll without blocking the audio loop.
        set_nonblocking(&mut ws)?;

        info!("Deepgram session connected");
        Ok(DeepgramSession {
            ws,
            audio_sent_secs: 0.0,
            last_send_time: Instant::now(),
            sample_rate,
        })
    }
}

// ---------------------------------------------------------------------------
// DeepgramSession — active WebSocket connection
// ---------------------------------------------------------------------------

pub struct DeepgramSession {
    ws: WebSocket<MaybeTlsStream<std::net::TcpStream>>,
    /// Total seconds of audio sent to Deepgram (accumulated from sample count + rate).
    audio_sent_secs: f64,
    /// Instant when the latest audio chunk was sent.
    last_send_time: Instant,
    /// Sample rate of audio being sent.
    sample_rate: u32,
}

impl SttSession for DeepgramSession {
    /// Send audio samples (f32 mono). Converts to i16 PCM internally.
    fn send_audio(&mut self, samples: &[f32]) -> Result<()> {
        let bytes = f32_to_pcm16_le(samples);

        match self.ws.send(Message::Binary(bytes)) {
            Ok(()) => {
                self.audio_sent_secs += samples.len() as f64 / self.sample_rate as f64;
                self.last_send_time = Instant::now();
                Ok(())
            }
            Err(tungstenite::Error::Io(e)) if e.kind() == ErrorKind::WouldBlock => {
                // Non-blocking socket buffer full — drop this chunk silently
                Ok(())
            }
            Err(e) => Err(anyhow::anyhow!("Failed to send audio to Deepgram: {}", e)),
        }
    }

    /// Poll for a finalized segment (is_final=true).
    /// Non-blocking — returns None immediately if no data is available.
    /// Uses is_final instead of speech_final for lower latency — no endpointing wait.
    fn poll_transcript(&mut self) -> Result<Option<SttResult>> {
        loop {
            match self.ws.read() {
                Ok(Message::Text(text)) => {
                    debug!("Deepgram: {}", &text[..text.len().min(200)]);
                    match serde_json::from_str::<DgResponse>(&text) {
                        Ok(resp) if resp.is_final == Some(true) => {
                            let transcript = resp
                                .channel
                                .and_then(|c| c.alternatives.into_iter().next())
                                .map(|a| a.transcript)
                                .unwrap_or_default();

                            if !transcript.trim().is_empty() {
                                // STT latency: how far behind real-time is Deepgram?
                                // audio_sent_secs = total audio duration sent
                                // utterance_end = start + duration (Deepgram's clock)
                                // The gap = (audio_sent - utterance_end) seconds of audio
                                //   that Deepgram still had buffered when it returned this result.
                                // Plus the network RTT from last send to now.
                                // Simplified: time since last audio send + processing backlog
                                let utterance_end_secs = resp.start.unwrap_or(0.0)
                                    + resp.duration.unwrap_or(0.0);
                                let backlog_secs = self.audio_sent_secs - utterance_end_secs;
                                let since_last_send_ms = self.last_send_time.elapsed().as_millis() as u64;
                                let stt_latency_ms = (backlog_secs * 1000.0).max(0.0) as u64
                                    + since_last_send_ms;

                                info!("Deepgram is_final: '{}' (stt={}ms)", transcript, stt_latency_ms);
                                return Ok(Some(SttResult {
                                    text: transcript,
                                    stt_latency_ms,
                                }));
                            }
                        }
                        Ok(_) => {}
                        Err(e) => debug!("Deepgram parse error: {}", e),
                    }
                }
                Ok(_) => {}
                Err(tungstenite::Error::Io(e)) if e.kind() == ErrorKind::WouldBlock => {
                    return Ok(None);
                }
                Err(e) => bail!("Deepgram WebSocket error: {}", e),
            }
        }
    }

    fn close(&mut self) {
        let _ = self.ws.send(Message::Binary(vec![]));
        let _ = self.ws.close(None);
    }
}

// ---------------------------------------------------------------------------
// Deepgram response types
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct DgResponse {
    is_final: Option<bool>,
    start: Option<f64>,
    duration: Option<f64>,
    channel: Option<DgChannel>,
}

#[derive(Deserialize)]
struct DgChannel {
    alternatives: Vec<DgAlternative>,
}

#[derive(Deserialize)]
struct DgAlternative {
    transcript: String,
}
