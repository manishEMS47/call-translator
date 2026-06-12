//! Speech-to-text provider abstraction.
//!
//! Two interchangeable streaming backends sit behind a single [`SttSession`]
//! trait so the audio pipeline behaves identically regardless of provider:
//!   - [`deepgram`] — Deepgram Nova-3 streaming WebSocket API.
//!   - [`sixtydb`]  — 60db streaming WebSocket STT API.
//!
//! Both stream raw 16 kHz PCM up over a non-blocking WebSocket and return
//! finalized utterances. The engine selects one per pipeline via [`SttConfig`].

mod deepgram;
mod sixtydb;

use anyhow::{Context, Result};
use tungstenite::stream::MaybeTlsStream;
use tungstenite::WebSocket;

/// A finalized transcript segment with measured STT latency.
pub struct SttResult {
    pub text: String,
    /// Real STT latency: wall-clock time from utterance end to result received.
    pub stt_latency_ms: u64,
}

/// A live streaming STT session.
///
/// The pipeline feeds audio with [`send_audio`](SttSession::send_audio) and
/// drains finalized utterances with [`poll_transcript`](SttSession::poll_transcript),
/// both non-blocking so the audio loop is never stalled.
pub trait SttSession: Send {
    /// Send audio samples (f32 mono, 16 kHz, [-1, 1]). Converts to i16 PCM internally.
    fn send_audio(&mut self, samples: &[f32]) -> Result<()>;

    /// Poll for a finalized segment. Non-blocking — returns `Ok(None)` when no
    /// finalized transcript is available yet.
    fn poll_transcript(&mut self) -> Result<Option<SttResult>>;

    /// Close the session cleanly.
    fn close(&mut self);
}

/// Provider-agnostic STT configuration. Holds credentials/options and creates
/// sessions on demand (one per pipeline direction).
pub enum SttConfig {
    Deepgram(deepgram::DeepgramStt),
    SixtyDb(sixtydb::SixtyDbStt),
}

impl SttConfig {
    /// Build an STT config for the given provider name (`"deepgram"` or `"60db"`).
    ///
    /// `endpointing_ms` is the silence threshold before an utterance is
    /// finalized — passed through to each provider's equivalent setting.
    pub fn new(provider: &str, api_key: String, language: String, endpointing_ms: u32) -> Self {
        match provider {
            "60db" | "sixtydb" => {
                SttConfig::SixtyDb(sixtydb::SixtyDbStt::new(api_key, language, endpointing_ms))
            }
            _ => SttConfig::Deepgram(deepgram::DeepgramStt::new(api_key, language, endpointing_ms)),
        }
    }

    /// Open a streaming session. `sample_rate` is the rate of audio you'll send.
    pub fn create_session(&self, sample_rate: u32) -> Result<Box<dyn SttSession>> {
        match self {
            SttConfig::Deepgram(s) => Ok(Box::new(s.create_session(sample_rate)?)),
            SttConfig::SixtyDb(s) => Ok(Box::new(s.create_session(sample_rate)?)),
        }
    }
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Convert mono f32 samples ([-1, 1]) to little-endian i16 PCM bytes.
pub(crate) fn f32_to_pcm16_le(samples: &[f32]) -> Vec<u8> {
    samples
        .iter()
        .flat_map(|&s| {
            let i = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
            i.to_le_bytes()
        })
        .collect()
}

/// Put the underlying TCP socket into non-blocking mode so reads/writes never
/// stall the audio loop.
pub(crate) fn set_nonblocking(
    ws: &mut WebSocket<MaybeTlsStream<std::net::TcpStream>>,
) -> Result<()> {
    use log::warn;
    match ws.get_mut() {
        MaybeTlsStream::Plain(s) => s.set_nonblocking(true).context("set_nonblocking (plain)")?,
        MaybeTlsStream::NativeTls(s) => s
            .get_ref()
            .set_nonblocking(true)
            .context("set_nonblocking (tls)")?,
        _ => warn!("Unknown stream type, non-blocking not set"),
    }
    Ok(())
}
