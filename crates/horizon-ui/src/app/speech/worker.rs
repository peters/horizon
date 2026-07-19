//! Transcription worker thread. Owns the transcribe.cpp model and session;
//! the model is loaded lazily on the first job so enabling the feature does
//! not slow down app startup, and a failed load is retried on the next job.

use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread;

use horizon_core::{PanelId, SpeechBackend, SpeechConfig, SpeechTask};
use transcribe_cpp::{Backend, Model, ModelOptions, RunOptions, Session, Task};

pub struct Job {
    pub pcm: Vec<f32>,
    pub target: PanelId,
}

pub enum WorkerEvent {
    Done { target: PanelId, text: String },
    Failed { message: String },
}

pub struct WorkerHandle {
    job_tx: Sender<Job>,
    event_rx: Receiver<WorkerEvent>,
}

impl WorkerHandle {
    pub fn spawn(config: &SpeechConfig) -> Self {
        let (job_tx, job_rx) = channel();
        let (event_tx, event_rx) = channel();
        let settings = Settings::from_config(config);
        thread::Builder::new()
            .name("speech-transcribe".into())
            .spawn(move || worker_loop(&settings, &job_rx, &event_tx))
            .expect("spawn speech transcribe thread");
        Self { job_tx, event_rx }
    }

    pub fn submit(&self, job: Job) {
        let _ = self.job_tx.send(job);
    }

    pub fn try_recv_event(&self) -> Option<WorkerEvent> {
        self.event_rx.try_recv().ok()
    }
}

struct Settings {
    model_path: String,
    language: Option<String>,
    task: Task,
    backend: Backend,
}

impl Settings {
    fn from_config(config: &SpeechConfig) -> Self {
        let language = match config.language.trim() {
            "" | "auto" => None,
            code => Some(code.to_string()),
        };
        Self {
            model_path: config.model.clone(),
            language,
            task: match config.task {
                SpeechTask::Transcribe => Task::Transcribe,
                SpeechTask::Translate => Task::Translate,
            },
            backend: match config.backend {
                SpeechBackend::Auto => Backend::Auto,
                SpeechBackend::Cpu => Backend::Cpu,
                SpeechBackend::Cuda => Backend::Cuda,
                SpeechBackend::Vulkan => Backend::Vulkan,
                SpeechBackend::Metal => Backend::Metal,
            },
        }
    }
}

fn worker_loop(settings: &Settings, job_rx: &Receiver<Job>, event_tx: &Sender<WorkerEvent>) {
    let mut session: Option<Session> = None;
    while let Ok(job) = job_rx.recv() {
        let target = job.target;
        let result = ensure_session(settings, &mut session).and_then(|session| {
            let options = RunOptions {
                task: settings.task,
                language: settings.language.clone(),
                ..RunOptions::default()
            };
            session
                .run(&job.pcm, &options)
                .map(|transcript| transcript.text)
                .map_err(|error| format!("transcription failed: {error}"))
        });
        let event = match result {
            Ok(text) => WorkerEvent::Done {
                target,
                text: text.trim().to_string(),
            },
            Err(message) => {
                tracing::warn!(%message, "speech transcription error");
                WorkerEvent::Failed { message }
            }
        };
        let _ = event_tx.send(event);
    }
}

#[cfg(test)]
mod tests {
    use transcribe_cpp::{Model, RunOptions};

    use super::super::capture::to_mono_16k;

    /// End-to-end pipeline check against a real model, gated on env vars so
    /// `cargo test --features speech` stays green without downloads:
    ///
    /// ```sh
    /// HORIZON_SPEECH_TEST_MODEL=/path/whisper-tiny-Q8_0.gguf \
    /// HORIZON_SPEECH_TEST_WAV=/path/jfk.wav \
    ///   cargo test --features speech speech_pipeline -- --nocapture
    /// ```
    #[test]
    fn speech_pipeline_transcribes_reference_wav() {
        let Ok(model_path) = std::env::var("HORIZON_SPEECH_TEST_MODEL") else {
            eprintln!("skipped: HORIZON_SPEECH_TEST_MODEL not set");
            return;
        };
        let Ok(wav_path) = std::env::var("HORIZON_SPEECH_TEST_WAV") else {
            eprintln!("skipped: HORIZON_SPEECH_TEST_WAV not set");
            return;
        };

        let (samples, sample_rate, channels) = read_pcm16_wav(&wav_path);
        let pcm = to_mono_16k(&samples, sample_rate, channels);
        assert!(pcm.len() > 16_000, "expected at least 1s of audio");

        let model = Model::load(&model_path).expect("load speech model");
        let mut session = model.session().expect("create session");
        let transcript = session.run(&pcm, &RunOptions::default()).expect("transcribe");
        eprintln!("backend: {} transcript: {}", model.backend(), transcript.text);
        assert!(!transcript.text.trim().is_empty(), "transcript must not be empty");
    }

    /// Minimal RIFF/WAVE reader for 16-bit PCM test fixtures.
    fn read_pcm16_wav(path: &str) -> (Vec<f32>, u32, usize) {
        let bytes = std::fs::read(path).expect("read wav fixture");
        assert!(bytes.len() > 44 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WAVE");
        let mut offset = 12;
        let mut format: Option<(u32, usize, u16)> = None;
        while offset + 8 <= bytes.len() {
            let id = &bytes[offset..offset + 4];
            let size = u32::from_le_bytes(bytes[offset + 4..offset + 8].try_into().unwrap()) as usize;
            let body = &bytes[offset + 8..(offset + 8 + size).min(bytes.len())];
            match id {
                b"fmt " => {
                    let channels = u16::from_le_bytes(body[2..4].try_into().unwrap());
                    let sample_rate = u32::from_le_bytes(body[4..8].try_into().unwrap());
                    let bits = u16::from_le_bytes(body[14..16].try_into().unwrap());
                    format = Some((sample_rate, usize::from(channels), bits));
                }
                b"data" => {
                    let (sample_rate, channels, bits) = format.expect("fmt chunk before data");
                    assert_eq!(bits, 16, "fixture must be 16-bit PCM");
                    let samples: Vec<f32> = body
                        .chunks_exact(2)
                        .map(|pair| f32::from(i16::from_le_bytes([pair[0], pair[1]])) / 32_768.0)
                        .collect();
                    return (samples, sample_rate, channels);
                }
                _ => {}
            }
            offset += 8 + size + (size & 1);
        }
        panic!("no data chunk found in {path}");
    }
}

fn ensure_session<'a>(settings: &Settings, session: &'a mut Option<Session>) -> Result<&'a mut Session, String> {
    if session.is_none() {
        if settings.model_path.trim().is_empty() {
            return Err("no speech model configured (features.speech.model)".to_string());
        }
        let options = ModelOptions {
            backend: settings.backend,
            ..ModelOptions::default()
        };
        let model = Model::load_with(&settings.model_path, &options)
            .map_err(|error| format!("failed to load speech model `{}`: {error}", settings.model_path))?;
        tracing::info!(
            model = %settings.model_path,
            backend = %model.backend(),
            "speech model loaded"
        );
        let new_session = model
            .session()
            .map_err(|error| format!("failed to create transcription session: {error}"))?;
        *session = Some(new_session);
    }
    Ok(session.as_mut().expect("session initialized above"))
}
