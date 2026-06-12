//! Main engine coordinator.
//!
//! Two pipelines:
//!   OUTGOING: Mic -> Deepgram(ru) -> Translate(ru->en) -> TTS(en) -> Speakers
//!   INCOMING: BlackHole 16ch -> Deepgram(en) -> Translate(en->ru) -> TTS(ru) -> Speakers

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Instant;

use anyhow::{Context, Result};
use crossbeam_channel::{bounded, Sender};
use log::{error, info, warn};

use crate::audio;
use crate::audio::capture::{AudioCapture, AudioChunk};
use crate::audio::playback::AudioPlayback;
use crate::protocol::Event;
use crate::stt::{SttConfig, SttSession};
use crate::translation::{TranslationDirection, TranslationEngine};
use crate::tts::{SixtyDbTts, TtsEngine, TtsSynthesizer};

// ---------------------------------------------------------------------------
// EngineConfig
// ---------------------------------------------------------------------------

pub struct EngineConfig {
    pub deepgram_api_key: String,
    pub sixtydb_api_key: String,
    pub groq_api_key: String,
    pub tts_en_model: String,
    pub tts_en_config: String,
    pub tts_ru_model: String,
    pub tts_ru_config: String,
    pub mic_device: String,
    pub speaker_device: String,
    pub meet_input_device: String,
    pub meet_output_device: String,
    pub sample_rate: u32,
    pub endpointing_ms: u32,
    pub my_language: String,
    pub their_language: String,
    /// STT provider per direction: "deepgram" or "60db".
    pub stt_provider_out: String,
    pub stt_provider_in: String,
    /// TTS provider per direction: "piper" or "60db".
    pub tts_provider_out: String,
    pub tts_provider_in: String,
    /// 60db voice ids (used when the corresponding TTS provider is "60db").
    pub tts_60db_voice_out: String,
    pub tts_60db_voice_in: String,
}

impl EngineConfig {
    pub fn from_env() -> Self {
        let base = std::env::var("TRANSLATOR_MODELS_DIR").unwrap_or_else(|_| "./models".into());

        Self {
            deepgram_api_key: std::env::var("DEEPGRAM_API_KEY").unwrap_or_default(),
            sixtydb_api_key: std::env::var("SIXTYDB_API_KEY").unwrap_or_default(),
            groq_api_key: std::env::var("GROQ_API_KEY").unwrap_or_default(),
            tts_en_model: std::env::var("TRANSLATOR_TTS_EN_MODEL")
                .unwrap_or_else(|_| format!("{}/piper-en/en_US-ryan-medium.onnx", base)),
            tts_en_config: std::env::var("TRANSLATOR_TTS_EN_CONFIG")
                .unwrap_or_else(|_| format!("{}/piper-en/en_US-ryan-medium.onnx.json", base)),
            tts_ru_model: std::env::var("TRANSLATOR_TTS_RU_MODEL")
                .unwrap_or_else(|_| format!("{}/piper-ru/ru_RU-denis-medium.onnx", base)),
            tts_ru_config: std::env::var("TRANSLATOR_TTS_RU_CONFIG")
                .unwrap_or_else(|_| format!("{}/piper-ru/ru_RU-denis-medium.onnx.json", base)),
            mic_device: std::env::var("TRANSLATOR_MIC_DEVICE")
                .unwrap_or_else(|_| "default".into()),
            speaker_device: std::env::var("TRANSLATOR_SPEAKER_DEVICE")
                .unwrap_or_else(|_| "default".into()),
            meet_input_device: std::env::var("TRANSLATOR_MEET_INPUT")
                .unwrap_or_else(|_| "BlackHole 16ch".into()),
            meet_output_device: std::env::var("TRANSLATOR_MEET_OUTPUT")
                .unwrap_or_else(|_| "BlackHole 2ch".into()),
            sample_rate: std::env::var("TRANSLATOR_SAMPLE_RATE")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(48000),
            endpointing_ms: std::env::var("TRANSLATOR_ENDPOINTING_MS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(300),
            my_language: std::env::var("TRANSLATOR_MY_LANG").unwrap_or_else(|_| "ru".into()),
            their_language: std::env::var("TRANSLATOR_THEIR_LANG").unwrap_or_else(|_| "en".into()),
            stt_provider_out: std::env::var("TRANSLATOR_STT_PROVIDER_OUT")
                .unwrap_or_else(|_| "deepgram".into()),
            stt_provider_in: std::env::var("TRANSLATOR_STT_PROVIDER_IN")
                .unwrap_or_else(|_| "deepgram".into()),
            tts_provider_out: std::env::var("TRANSLATOR_TTS_PROVIDER_OUT")
                .unwrap_or_else(|_| "piper".into()),
            tts_provider_in: std::env::var("TRANSLATOR_TTS_PROVIDER_IN")
                .unwrap_or_else(|_| "piper".into()),
            tts_60db_voice_out: std::env::var("TRANSLATOR_TTS_60DB_VOICE_OUT").unwrap_or_default(),
            tts_60db_voice_in: std::env::var("TRANSLATOR_TTS_60DB_VOICE_IN").unwrap_or_default(),
        }
    }
}

// ---------------------------------------------------------------------------
// Engine
// ---------------------------------------------------------------------------

#[derive(Debug, PartialEq)]
enum EngineState {
    Idle,
    Running,
    ShuttingDown,
}

pub struct Engine {
    state: EngineState,
    config: EngineConfig,
    event_tx: Sender<Event>,
    pipeline_handles: Vec<thread::JoinHandle<()>>,
    stop_flag: Option<Arc<AtomicBool>>,
    mute_outgoing: Arc<AtomicBool>,
    mute_incoming: Arc<AtomicBool>,
}

impl Engine {
    pub fn new(config: EngineConfig, event_tx: Sender<Event>) -> Self {
        Self {
            state: EngineState::Idle,
            config,
            event_tx,
            pipeline_handles: Vec::new(),
            stop_flag: None,
            mute_outgoing: Arc::new(AtomicBool::new(false)),
            mute_incoming: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn is_shutting_down(&self) -> bool {
        self.state == EngineState::ShuttingDown
    }

    pub fn handle_command(&mut self, cmd: crate::protocol::Command) -> Vec<Event> {
        use crate::protocol::Command;

        match cmd {
            Command::Ping => vec![Event::Pong],

            Command::Start { pipelines } => {
                if self.state == EngineState::Running {
                    return vec![Event::Error {
                        message: "Pipelines already running. Stop first.".into(),
                    }];
                }

                if let Err(e) = audio::list_devices() {
                    info!("Could not enumerate audio devices: {:#}", e);
                }

                if let Err(msg) = self.validate_keys(&pipelines) {
                    return vec![Event::Error { message: msg }];
                }

                match self.start_pipelines(&pipelines) {
                    Ok(()) => {
                        self.state = EngineState::Running;
                        vec![
                            Event::Log {
                                level: "info".into(),
                                message: format!("Starting pipelines: {:?}", pipelines),
                            },
                            Event::Started {
                                pipelines: pipelines.clone(),
                            },
                        ]
                    }
                    Err(e) => {
                        error!("Failed to start pipelines: {:#}", e);
                        vec![Event::Error {
                            message: format!("Failed to start pipelines: {:#}", e),
                        }]
                    }
                }
            }

            Command::Stop => {
                self.stop_pipelines();
                self.state = EngineState::Idle;
                vec![
                    Event::Log {
                        level: "info".into(),
                        message: "Pipelines stopped".into(),
                    },
                    Event::Stopped,
                ]
            }

            Command::SetConfig { key, value } => {
                self.apply_config(&key, &value);
                vec![Event::Log {
                    level: "info".into(),
                    message: format!("Config set: {} = {}", key, value),
                }]
            }

            Command::ListDevices => match audio::list_devices() {
                Ok((input, output)) => vec![Event::DeviceList { input, output }],
                Err(e) => vec![Event::Error {
                    message: format!("Failed to list devices: {:#}", e),
                }],
            },

            Command::TtsPreview { lang, voice } => {
                let models_base = std::env::var("TRANSLATOR_MODELS_DIR")
                    .unwrap_or_else(|_| "./models".into());
                let model_path = format!("{}/piper-{}/{}.onnx", models_base, lang, voice);
                let config_path = format!("{}/piper-{}/{}.onnx.json", models_base, lang, voice);
                let text = match lang.as_str() {
                    "ru" => "Привет, это тест голоса для перевода.",
                    "de" => "Hallo, dies ist ein Stimmtest.",
                    "fr" => "Bonjour, ceci est un test de voix.",
                    "es" => "Hola, esta es una prueba de voz.",
                    "it" => "Ciao, questo è un test vocale.",
                    "pt" => "Olá, este é um teste de voz.",
                    "zh" => "你好，这是语音测试。",
                    "ar" => "مرحبا، هذا اختبار صوتي.",
                    "hi" => "नमस्ते, यह एक आवाज़ परीक्षण है।",
                    "tr" => "Merhaba, bu bir ses testidir.",
                    "nl" => "Hallo, dit is een stemtest.",
                    "pl" => "Cześć, to jest test głosu.",
                    "uk" => "Привіт, це тест голосу.",
                    _ => "Hello, this is a voice preview test.",
                };

                match TtsEngine::new(&config_path, &model_path, self.config.sample_rate) {
                    Ok(mut tts) => {
                        match tts.synthesize(text) {
                            Ok(samples) => {
                                // Play through default speakers
                                let speaker = self.config.speaker_device.clone();
                                let sr = self.config.sample_rate;
                                let (tx, rx) = crossbeam_channel::bounded(4);
                                match AudioPlayback::new(&speaker, sr, rx) {
                                    Ok(playback) => {
                                        let _ = tx.send(samples);
                                        drop(tx);
                                        // Wait for playback to finish
                                        std::thread::sleep(std::time::Duration::from_secs(3));
                                        drop(playback);
                                        vec![Event::TtsPreviewDone]
                                    }
                                    Err(e) => vec![Event::Error {
                                        message: format!("Preview playback failed: {:#}", e),
                                    }],
                                }
                            }
                            Err(e) => vec![Event::Error {
                                message: format!("Preview synthesis failed: {:#}", e),
                            }],
                        }
                    }
                    Err(e) => vec![Event::Error {
                        message: format!("Preview TTS load failed: {:#}", e),
                    }],
                }
            }

            Command::Shutdown => {
                let mut events = Vec::new();
                if self.state == EngineState::Running {
                    self.stop_pipelines();
                    events.push(Event::Stopped);
                }
                self.state = EngineState::ShuttingDown;
                events
            }
        }
    }

    fn apply_config(&mut self, key: &str, value: &serde_json::Value) {
        match key {
            "endpointing_ms" => {
                if let Some(v) = value.as_u64() {
                    self.config.endpointing_ms = v as u32;
                    info!("Updated endpointing_ms to {}", v);
                }
            }
            "mute_outgoing" => {
                let muted = value.as_bool().unwrap_or(false);
                self.mute_outgoing.store(muted, Ordering::SeqCst);
                info!("Outgoing mute: {}", muted);
            }
            "mute_incoming" => {
                let muted = value.as_bool().unwrap_or(false);
                self.mute_incoming.store(muted, Ordering::SeqCst);
                info!("Incoming mute: {}", muted);
            }
            _ => warn!("Unknown config key: {}", key),
        }
    }

    /// Verify the API keys required by the selected providers are present
    /// before we try to start any pipeline. Returns an error message to surface
    /// to the UI when a needed key is missing.
    fn validate_keys(&self, pipelines: &[String]) -> std::result::Result<(), String> {
        let c = &self.config;
        let mut needs_deepgram = false;
        let mut needs_sixtydb = false;

        for p in pipelines {
            let (stt_provider, tts_provider) = match p.as_str() {
                "outgoing" => (&c.stt_provider_out, &c.tts_provider_out),
                "incoming" => (&c.stt_provider_in, &c.tts_provider_in),
                _ => continue,
            };
            match stt_provider.as_str() {
                "60db" | "sixtydb" => needs_sixtydb = true,
                _ => needs_deepgram = true,
            }
            if matches!(tts_provider.as_str(), "60db" | "sixtydb") {
                needs_sixtydb = true;
            }
        }

        if needs_deepgram && c.deepgram_api_key.is_empty() {
            return Err("DEEPGRAM_API_KEY is not set".into());
        }
        if needs_sixtydb && c.sixtydb_api_key.is_empty() {
            return Err("SIXTYDB_API_KEY is not set (required for the selected 60db provider)".into());
        }
        Ok(())
    }

    /// Build the STT config for a direction from its provider + language.
    fn stt_for(&self, provider: &str, language: &str) -> SttConfig {
        let key = match provider {
            "60db" | "sixtydb" => self.config.sixtydb_api_key.clone(),
            _ => self.config.deepgram_api_key.clone(),
        };
        SttConfig::new(provider, key, language.to_string(), self.config.endpointing_ms)
    }

    fn start_pipelines(&mut self, pipelines: &[String]) -> Result<()> {
        let stop_flag = Arc::new(AtomicBool::new(false));
        self.stop_flag = Some(stop_flag.clone());

        info!("Loading translation models...");
        let translator = Arc::new(
            TranslationEngine::new(&self.config.groq_api_key)
                .context("Failed to initialize translation engine")?,
        );

        info!("Loading STT/TTS providers, spawning pipelines...");

        for pipeline_name in pipelines {
            match pipeline_name.as_str() {
                "outgoing" => {
                    // OUTGOING: my speech -> their language out.
                    let stt = self.stt_for(&self.config.stt_provider_out, &self.config.my_language);
                    let tts = build_tts(
                        &self.config.tts_provider_out,
                        &self.config.tts_en_config,
                        &self.config.tts_en_model,
                        &self.config.sixtydb_api_key,
                        &self.config.tts_60db_voice_out,
                        self.config.sample_rate,
                    )
                    .context("Failed to load TTS provider (outgoing)")?;
                    let handle = spawn_pipeline(
                        "outgoing",
                        self.config.mic_device.clone(),
                        self.config.meet_output_device.clone(),
                        self.config.sample_rate,
                        stt,
                        translator.clone(),
                        TranslationDirection::new(&self.config.my_language, &self.config.their_language),
                        &self.config.my_language,
                        tts,
                        self.event_tx.clone(),
                        stop_flag.clone(),
                        self.mute_outgoing.clone(),
                    )?;
                    self.pipeline_handles.push(handle);
                }
                "incoming" => {
                    // INCOMING: their speech -> my language out.
                    let stt = self.stt_for(&self.config.stt_provider_in, &self.config.their_language);
                    let tts = build_tts(
                        &self.config.tts_provider_in,
                        &self.config.tts_ru_config,
                        &self.config.tts_ru_model,
                        &self.config.sixtydb_api_key,
                        &self.config.tts_60db_voice_in,
                        self.config.sample_rate,
                    )
                    .context("Failed to load TTS provider (incoming)")?;
                    let handle = spawn_pipeline(
                        "incoming",
                        self.config.meet_input_device.clone(),
                        self.config.speaker_device.clone(),
                        self.config.sample_rate,
                        stt,
                        translator.clone(),
                        TranslationDirection::new(&self.config.their_language, &self.config.my_language),
                        &self.config.their_language,
                        tts,
                        self.event_tx.clone(),
                        stop_flag.clone(),
                        self.mute_incoming.clone(),
                    )?;
                    self.pipeline_handles.push(handle);
                }
                other => warn!("Unknown pipeline: {}", other),
            }
        }

        Ok(())
    }

    fn stop_pipelines(&mut self) {
        if let Some(flag) = self.stop_flag.take() {
            flag.store(true, Ordering::SeqCst);
        }

        for handle in self.pipeline_handles.drain(..) {
            let name = handle.thread().name().unwrap_or("unnamed").to_string();
            info!("Waiting for pipeline thread '{}' to stop...", name);
            if let Err(e) = handle.join() {
                error!("Pipeline thread '{}' panicked: {:?}", name, e);
            }
        }
        info!("All pipeline threads stopped");
    }
}

// ---------------------------------------------------------------------------
// Pipeline spawning
// ---------------------------------------------------------------------------

/// Construct a TTS synthesizer for the given provider.
///
/// `piper_config`/`piper_model` are used for the local Piper backend;
/// `sixtydb_key`/`sixtydb_voice` for the 60db backend. Both return audio at
/// `sample_rate`, so the pipeline is provider-agnostic downstream.
fn build_tts(
    provider: &str,
    piper_config: &str,
    piper_model: &str,
    sixtydb_key: &str,
    sixtydb_voice: &str,
    sample_rate: u32,
) -> Result<Box<dyn TtsSynthesizer>> {
    match provider {
        "60db" | "sixtydb" => {
            let tts = SixtyDbTts::new(sixtydb_key.to_string(), sixtydb_voice.to_string(), sample_rate)?;
            Ok(Box::new(tts))
        }
        _ => {
            let tts = TtsEngine::new(piper_config, piper_model, sample_rate)?;
            Ok(Box::new(tts))
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn spawn_pipeline(
    direction: &str,
    capture_device: String,
    playback_device: String,
    sample_rate: u32,
    stt: SttConfig,
    translator: Arc<TranslationEngine>,
    translate_direction: TranslationDirection,
    source_lang: &str,
    tts: Box<dyn TtsSynthesizer>,
    event_tx: Sender<Event>,
    stop_flag: Arc<AtomicBool>,
    mute_flag: Arc<AtomicBool>,
) -> Result<thread::JoinHandle<()>> {
    let dir_name = direction.to_string();
    let src_lang = source_lang.to_string();

    let handle = thread::Builder::new()
        .name(format!("pipeline-{}", direction))
        .spawn(move || {
            if let Err(e) = run_pipeline(
                &dir_name,
                &capture_device,
                &playback_device,
                sample_rate,
                stt,
                &translator,
                translate_direction,
                &src_lang,
                tts,
                &event_tx,
                &stop_flag,
                &mute_flag,
            ) {
                error!("{} pipeline failed: {:#}", dir_name, e);
                let _ = event_tx.send(Event::Error {
                    message: format!("{} pipeline failed: {:#}", dir_name, e),
                });
            }
            info!("{} pipeline thread exiting", dir_name);
        })
        .context("Failed to spawn pipeline thread")?;

    Ok(handle)
}

// ---------------------------------------------------------------------------
// Core pipeline logic
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn run_pipeline(
    direction: &str,
    capture_device: &str,
    playback_device: &str,
    sample_rate: u32,
    stt: SttConfig,
    translator: &TranslationEngine,
    translate_direction: TranslationDirection,
    source_lang: &str,
    mut tts: Box<dyn TtsSynthesizer>,
    event_tx: &Sender<Event>,
    stop_flag: &AtomicBool,
    mute_flag: &AtomicBool,
) -> Result<()> {
    info!(
        "[{}] Starting pipeline: capture='{}', playback='{}'",
        direction, capture_device, playback_device
    );

    let (audio_tx, audio_rx) = bounded::<AudioChunk>(512);
    let (playback_tx, playback_rx) = bounded::<Vec<f32>>(64);
    // Transcripts go here; the processor thread picks them up without blocking audio.
    let (proc_tx, proc_rx) = bounded::<(String, u64)>(16);

    let capture = AudioCapture::new(capture_device, audio_tx)
        .with_context(|| format!("[{}] Failed to create AudioCapture", direction))?;
    let capture_rate = capture.sample_rate();

    let playback = AudioPlayback::new(playback_device, sample_rate, playback_rx)
        .with_context(|| format!("[{}] Failed to create AudioPlayback", direction))?;

    // Connect to the STT provider — stream at 16kHz to save bandwidth
    let stt_sample_rate = 16_000_u32;
    let mut session = stt
        .create_session(stt_sample_rate)
        .with_context(|| format!("[{}] Failed to connect to STT provider", direction))?;

    capture
        .start()
        .with_context(|| format!("[{}] Failed to start capture", direction))?;
    playback
        .start()
        .with_context(|| format!("[{}] Failed to start playback", direction))?;

    let drained = audio_rx.try_iter().count();
    if drained > 0 {
        info!("[{}] Drained {} stale audio chunks", direction, drained);
    }

    info!("[{}] Pipeline running", direction);

    // Echo suppression: ignore STT results while TTS is playing back through speakers.
    let echo_suppress = Arc::new(AtomicBool::new(false));

    // Processor thread: translate + TTS, runs independently so audio loop is never blocked.
    let proc_translator = translator.clone();
    let proc_playback_tx = playback_tx.clone();
    let proc_event_tx = event_tx.clone();
    let proc_direction = direction.to_string();
    let proc_source_lang = source_lang.to_string();
    let proc_sample_rate = sample_rate;
    let _proc_handle = std::thread::spawn(move || {
        while let Ok((text, stt_ms)) = proc_rx.recv() {
            let _audio_len = process_utterance(
                &proc_direction,
                &text,
                stt_ms,
                &proc_translator,
                &translate_direction,
                &proc_source_lang,
                &mut *tts,
                proc_sample_rate,
                &proc_playback_tx,
                &proc_event_tx,
            );
        }
    });

    info!("[{}] Capture rate: {}Hz, STT rate: {}Hz", direction, capture_rate, stt_sample_rate);

    loop {
        if stop_flag.load(Ordering::SeqCst) {
            info!("[{}] Stop flag set, exiting", direction);
            break;
        }

        match audio_rx.recv_timeout(std::time::Duration::from_millis(100)) {
            Ok(chunk) => {
                if mute_flag.load(Ordering::Relaxed) {
                    continue;
                }
                let samples_16k = resample(&chunk.samples, capture_rate, stt_sample_rate);
                if let Err(e) = session.send_audio(&samples_16k) {
                    warn!("[{}] STT send error: {:#}", direction, e);
                }
            }
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                info!("[{}] Audio capture channel disconnected", direction);
                break;
            }
        }

        // Poll for completed utterances (non-blocking) — just queue, don't process here
        match session.poll_transcript() {
            Ok(Some(result)) => {
                if echo_suppress.load(Ordering::SeqCst) {
                    info!("[{}] Echo suppressed: '{}'", direction, result.text);
                } else {
                    if let Err(e) = proc_tx.try_send((result.text, result.stt_latency_ms)) {
                        warn!("[{}] Processor channel full, dropping transcript: {}", direction, e);
                    }
                }
            }
            Ok(None) => {}
            Err(e) => {
                error!("[{}] STT error: {:#}", direction, e);
                let _ = event_tx.send(Event::Error {
                    message: format!("[{}] STT error: {:#}", direction, e),
                });
                break;
            }
        }
    }

    session.close();
    let _ = capture.stop();
    let _ = playback.stop();
    drop(playback_tx);

    info!("[{}] Pipeline stopped cleanly", direction);
    Ok(())
}

// ---------------------------------------------------------------------------
// Utterance processing: transcript -> translate -> TTS -> playback
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn process_utterance(
    direction: &str,
    text: &str,
    stt_ms: u64,
    translator: &TranslationEngine,
    translate_direction: &TranslationDirection,
    source_lang: &str,
    tts: &mut dyn TtsSynthesizer,
    sample_rate: u32,
    playback_tx: &Sender<Vec<f32>>,
    event_tx: &Sender<Event>,
) -> usize {
    info!("[{}] Transcript: '{}'", direction, text);

    let _ = event_tx.send(Event::Transcript {
        direction: direction.to_string(),
        text: text.to_string(),
        lang: source_lang.to_string(),
    });

    let translate_start = Instant::now();
    let translated = match translator.translate(text, translate_direction) {
        Ok(t) => t,
        Err(e) => {
            error!("[{}] Translation error: {:#}", direction, e);
            let _ = event_tx.send(Event::Error {
                message: format!("[{}] Translation failed: {:#}", direction, e),
            });
            return 0;
        }
    };
    let translate_ms = translate_start.elapsed().as_millis() as u64;

    info!("[{}] Translation: '{}'", direction, translated);
    let _ = event_tx.send(Event::Translation {
        direction: direction.to_string(),
        text: translated.clone(),
    });

    if translated.trim().is_empty() {
        return 0;
    }

    let tts_start = Instant::now();
    let audio = match tts.synthesize(&translated) {
        Ok(samples) => samples,
        Err(e) => {
            error!("[{}] TTS error: {:#}", direction, e);
            let _ = event_tx.send(Event::Error {
                message: format!("[{}] TTS failed: {:#}", direction, e),
            });
            return 0;
        }
    };
    let tts_ms = tts_start.elapsed().as_millis() as u64;

    let audio_len = audio.len();

    if !audio.is_empty() {
        // Downsample to 16kHz for browser monitor (good quality, ~40KB per phrase)
        let monitor_rate = 16000u32;
        let monitor_samples = resample(&audio, sample_rate, monitor_rate);
        let mut pcm_bytes = Vec::with_capacity(monitor_samples.len() * 2);
        for &s in &monitor_samples {
            let i = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
            pcm_bytes.extend_from_slice(&i.to_le_bytes());
        }
        use base64::Engine as _;
        let b64 = base64::engine::general_purpose::STANDARD.encode(&pcm_bytes);
        let _ = event_tx.send(Event::TtsAudio {
            direction: direction.to_string(),
            sample_rate: monitor_rate,
            audio_b64: b64,
        });

        if let Err(e) = playback_tx.try_send(audio) {
            warn!("[{}] Playback channel full or disconnected: {}", direction, e);
        }
    }

    let _ = event_tx.send(Event::Metrics {
        stt_ms,
        translate_ms,
        tts_ms,
    });

    audio_len
}

// ---------------------------------------------------------------------------
// Audio utility
// ---------------------------------------------------------------------------

/// Resample audio from `from_rate` to `to_rate` using linear interpolation.
/// Handles arbitrary rate ratios (e.g. 24000→16000, 48000→16000).
fn resample(samples: &[f32], from_rate: u32, to_rate: u32) -> Vec<f32> {
    if from_rate == to_rate || samples.is_empty() {
        return samples.to_vec();
    }
    let ratio = from_rate as f64 / to_rate as f64;
    let output_len = (samples.len() as f64 / ratio) as usize;
    (0..output_len)
        .map(|i| {
            let src = i as f64 * ratio;
            let idx = src as usize;
            let frac = src - idx as f64;
            if idx + 1 < samples.len() {
                samples[idx] * (1.0 - frac as f32) + samples[idx + 1] * frac as f32
            } else {
                samples[idx.min(samples.len() - 1)]
            }
        })
        .collect()
}
