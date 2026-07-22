//! Transcription worker thread. Owns the transcribe.cpp model and session;
//! the model is loaded lazily on the first job so enabling the feature does
//! not slow down app startup, and a failed load is retried on the next job.

use std::collections::HashMap;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Mutex, PoisonError};
use std::thread;

use horizon_core::{PanelId, SpeechBackend, SpeechProfile, SpeechTask};
use transcribe_cpp::{Backend, CancelToken, Model, ModelOptions, RunOptions, Session, Task};

/// `Model::load_with` cannot be interrupted, so loads are serialized across
/// all workers: a replacement worker cannot begin loading until the
/// cancelled one finished its load (and its prompt exit releases the
/// model), keeping two multi-GB models from loading concurrently.
static MODEL_LOAD_LOCK: Mutex<()> = Mutex::new(());

/// Models already loaded in this process, keyed by resolved path + backend.
/// `Model` is a cheap `Arc` handle and `session()` takes `&self`, so profiles
/// pointing at the same GGUF share one residency instead of loading the file
/// (and its GPU allocation) once per profile. Guarded by `MODEL_LOAD_LOCK`.
static LOADED_MODELS: Mutex<Option<HashMap<String, Model>>> = Mutex::new(None);

pub struct Job {
    pub pcm: Vec<f32>,
    pub target: PanelId,
}

pub enum WorkerEvent {
    /// The model finished loading; reports the backend actually selected
    /// (interesting when the config says `auto`).
    ModelLoaded {
        backend: String,
    },
    Done {
        target: PanelId,
        text: String,
    },
    Failed {
        target: PanelId,
        message: String,
    },
}

pub struct WorkerHandle {
    job_tx: Sender<Job>,
    event_rx: Receiver<WorkerEvent>,
    cancel: CancelToken,
}

/// Dropping the handle (e.g. a live config rebuild replacing the speech
/// system) must not leak an in-flight inference: cancel it so the worker
/// thread unblocks, sees its closed job channel, and exits — releasing the
/// model instead of running to completion beside a freshly loaded one.
impl Drop for WorkerHandle {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

impl WorkerHandle {
    pub fn spawn(profile: &SpeechProfile, backend: SpeechBackend) -> std::io::Result<Self> {
        let (job_tx, job_rx) = channel();
        let (event_tx, event_rx) = channel();
        let settings = Settings::from_profile(profile, backend);
        let cancel = CancelToken::new();
        let worker_cancel = cancel.clone();
        thread::Builder::new()
            .name("speech-transcribe".into())
            .spawn(move || worker_loop(&settings, &worker_cancel, &job_rx, &event_tx))?;
        Ok(Self {
            job_tx,
            event_rx,
            cancel,
        })
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
    target_language: Option<String>,
    backend: Backend,
}

impl Settings {
    fn from_profile(profile: &SpeechProfile, backend: SpeechBackend) -> Self {
        let language = match profile.language.trim() {
            "" | "auto" => None,
            code => Some(code.to_string()),
        };
        let task = match profile.task {
            SpeechTask::Transcribe => Task::Transcribe,
            SpeechTask::Translate => Task::Translate,
        };
        let target_language = match (task, profile.target_language.trim()) {
            (Task::Translate, code) if !code.is_empty() => Some(code.to_string()),
            _ => None,
        };
        Self {
            // `~/models/...` must work like every other path in the config.
            model_path: horizon_core::dir_search::expand_tilde(profile.model.trim())
                .to_string_lossy()
                .into_owned(),
            language,
            task,
            target_language,
            backend: match backend {
                SpeechBackend::Auto => Backend::Auto,
                SpeechBackend::Cpu => Backend::Cpu,
                SpeechBackend::Cuda => Backend::Cuda,
                SpeechBackend::Vulkan => Backend::Vulkan,
                SpeechBackend::Metal => Backend::Metal,
            },
        }
    }
}

fn worker_loop(settings: &Settings, cancel: &CancelToken, job_rx: &Receiver<Job>, event_tx: &Sender<WorkerEvent>) {
    let mut session: Option<Session> = None;
    while let Ok(job) = job_rx.recv() {
        if cancel.is_cancelled() {
            return;
        }
        let target = job.target;
        let result = ensure_session(settings, cancel, &mut session).and_then(|loaded_backend| {
            if let Some(backend) = loaded_backend {
                let _ = event_tx.send(WorkerEvent::ModelLoaded { backend });
            }
            let Some(session) = session.as_mut() else {
                return Err("transcription session unavailable".to_string());
            };
            let options = RunOptions {
                task: settings.task,
                language: settings.language.clone(),
                target_language: settings.target_language.clone(),
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
                text: sanitize_transcript(&text),
            },
            Err(message) => {
                tracing::warn!(%message, "speech transcription error");
                WorkerEvent::Failed { target, message }
            }
        };
        let _ = event_tx.send(event);
    }
}

/// Collapse all internal whitespace (including newlines) to single spaces
/// and strip every remaining control character. Dictated text must never
/// carry `\n`/`\r` (Enter on non-bracketed terminals) nor C0/C1 bytes like
/// ESC or NUL, which `paste_bytes` only strips in bracketed-paste mode.
fn sanitize_transcript(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .filter(|character| !character.is_control())
        .collect()
}

/// Lazily create the model-backed session on first use; leaves `session`
/// populated on success so later jobs reuse it. Returns the selected
/// backend name when this call performed the load.
fn ensure_session(
    settings: &Settings,
    cancel: &CancelToken,
    session: &mut Option<Session>,
) -> Result<Option<String>, String> {
    if session.is_some() {
        return Ok(None);
    }
    if settings.model_path.trim().is_empty() {
        return Err("no speech model configured (features.speech.model)".to_string());
    }
    let _load_permit = MODEL_LOAD_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
    if cancel.is_cancelled() {
        return Err("speech worker replaced before model load".to_string());
    }
    let options = ModelOptions {
        backend: settings.backend,
        ..ModelOptions::default()
    };
    let cache_key = format!("{}|{:?}", settings.model_path, settings.backend);
    let cached = {
        let guard = LOADED_MODELS.lock().unwrap_or_else(PoisonError::into_inner);
        guard.as_ref().and_then(|map| map.get(&cache_key).cloned())
    };
    let model = if let Some(model) = cached {
        model
    } else {
        let model = Model::load_with(&settings.model_path, &options)
            .map_err(|error| format!("failed to load speech model `{}`: {error}", settings.model_path))?;
        if cancel.is_cancelled() {
            // Replaced while loading: drop the model BEFORE releasing the load
            // lock so the waiting replacement never overlaps residence with it.
            drop(model);
            return Err("speech worker replaced during model load".to_string());
        }
        LOADED_MODELS
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get_or_insert_with(HashMap::new)
            .insert(cache_key, model.clone());
        model
    };
    if cancel.is_cancelled() {
        return Err("speech worker replaced during model load".to_string());
    }
    let backend = model.backend().clone();
    tracing::info!(model = %settings.model_path, %backend, "speech model loaded");
    let mut new_session = model
        .session()
        .map_err(|error| format!("failed to create transcription session: {error}"))?;
    new_session.set_cancel_token(cancel);
    *session = Some(new_session);
    Ok(Some(backend))
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

    #[test]
    fn sanitize_strips_controls_and_collapses_whitespace() {
        assert_eq!(
            super::sanitize_transcript("hei\x1b[31m der\r\nverden\u{0000}!"),
            "hei[31m der verden!"
        );
        assert_eq!(super::sanitize_transcript("  a\tb  c  "), "a b c");
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
