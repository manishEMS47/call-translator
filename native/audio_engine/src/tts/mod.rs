//! Text-to-speech provider abstraction.
//!
//! Two interchangeable backends sit behind a single [`TtsSynthesizer`] trait so
//! the audio pipeline treats them identically — both return f32 mono samples at
//! the pipeline's output sample rate:
//!   - [`piper`]   — local ONNX synthesis (espeak-ng phonemization).
//!   - [`sixtydb`] — 60db streaming WebSocket TTS.

pub mod piper;
mod sixtydb;

use anyhow::Result;
use log::info;

use self::piper::PiperTts;
pub use self::sixtydb::SixtyDbTts;

/// Converts text into f32 mono audio samples at a fixed output sample rate.
pub trait TtsSynthesizer: Send {
    fn synthesize(&mut self, text: &str) -> Result<Vec<f32>>;
}

/// Local Piper TTS engine.
///
/// Wraps the Piper backend (ONNX inference + espeak-ng phonemization) and
/// handles resampling from the model's native sample rate (typically 22050 Hz)
/// to the pipeline output rate (typically 48000 Hz).
pub struct TtsEngine {
    inner: PiperTts,
}

impl TtsEngine {
    /// Create a new Piper TTS engine.
    ///
    /// `config_path` — path to the Piper `.onnx.json` config file.
    /// `model_path`  — path to the Piper `.onnx` model file.
    /// `output_sample_rate` — target sample rate for the audio pipeline (e.g. 48000).
    pub fn new(config_path: &str, model_path: &str, output_sample_rate: u32) -> Result<Self> {
        info!(
            "Initializing Piper TTS engine: config={}, model={}, output_rate={}",
            config_path, model_path, output_sample_rate
        );
        let inner = PiperTts::new(config_path, model_path, output_sample_rate)?;
        info!("Piper TTS engine ready");
        Ok(Self { inner })
    }
}

impl TtsSynthesizer for TtsEngine {
    /// Synthesize text into f32 audio samples at `output_sample_rate`.
    fn synthesize(&mut self, text: &str) -> Result<Vec<f32>> {
        self.inner.synthesize(text)
    }
}
