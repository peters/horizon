//! Transcription worker thread. Owns the transcribe.cpp model and session;
//! the model is loaded lazily on the first job so enabling the feature does
//! not slow down app startup, and a failed load is retried on the next job.

use std::collections::HashMap;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Condvar, Mutex, PoisonError, Weak};
use std::thread;
use std::time::{Duration, Instant};

use horizon_core::{PanelId, SpeechBackend, SpeechProfile, SpeechTask};
use transcribe_cpp::{Backend, CancelToken, Model, ModelOptions, RunOptions, Session, Task};

/// `Model::load_with` cannot be interrupted, so loads are serialized across
/// all workers: two multi-GB models are never *loaded* concurrently. This
/// alone does not bound residency — an already-loaded model stays resident
/// while its worker runs — so loaders also wait on `RETIRING` below.
static MODEL_LOAD_LOCK: Mutex<()> = Mutex::new(());

/// Models loaded in this process, keyed by resolved path + backend, held
/// WEAKLY. Profiles pointing at the same GGUF share one residency instead of
/// loading the file (and its GPU allocation) once per profile, but the cache
/// never keeps a model alive on its own: the strong handles live in the
/// workers using it, so switching config to a different model frees the old
/// one instead of pinning multi-GB allocations for the process lifetime.
static LOADED_MODELS: Mutex<Option<HashMap<String, Weak<Model>>>> = Mutex::new(None);

/// Cancelled workers that may not have exited yet. Cancellation is
/// cooperative and transcribe.cpp does not observe the token mid-run for
/// every model family, so a cancelled worker can still be inside a native
/// `Session::run` — holding its multi-GB model resident — long after its
/// handle was dropped. Serializing *loads* is therefore not enough: a
/// replacement would load a second model beside the outgoing one and can
/// exhaust GPU memory on a live config change. Loaders wait here first.
static RETIRING: Mutex<Vec<Weak<WorkerLife>>> = Mutex::new(Vec::new());
static RETIRED: Condvar = Condvar::new();

/// How long a loader waits for outgoing workers. A wedged native call must
/// degrade to the old (overlapping) behaviour rather than block dictation
/// forever.
const RETIRE_TIMEOUT: Duration = Duration::from_secs(30);

/// Owned by the worker thread and dropped only when that thread returns,
/// i.e. once its model handle is released.
struct WorkerLife;

impl Drop for WorkerLife {
    fn drop(&mut self) {
        // Prune under the lock before signalling: the strong count is already
        // zero here, so a waiter that re-checks its predicate cannot miss this
        // exit and sleep until the timeout.
        RETIRING
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|life| life.strong_count() > 0);
        RETIRED.notify_all();
    }
}

/// Block until every cancelled worker has released its model, or the timeout
/// expires. Called on a worker thread, never on the UI thread — an in-flight
/// inference can take seconds and the frame loop must keep running.
fn await_retiring_workers() {
    let deadline = Instant::now() + RETIRE_TIMEOUT;
    let mut retiring = RETIRING.lock().unwrap_or_else(PoisonError::into_inner);
    loop {
        retiring.retain(|life| life.strong_count() > 0);
        if retiring.is_empty() {
            return;
        }
        let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
            tracing::warn!(
                pending = retiring.len(),
                "cancelled speech worker still running after {RETIRE_TIMEOUT:?}; loading anyway"
            );
            return;
        };
        let (next, _) = RETIRED
            .wait_timeout(retiring, remaining)
            .unwrap_or_else(PoisonError::into_inner);
        retiring = next;
    }
}

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
    /// Alive until the worker thread returns; registered as retiring on drop
    /// so the next loader waits for this worker's model to be released.
    life: Weak<WorkerLife>,
}

/// Dropping the handle (e.g. a live config rebuild replacing the speech
/// system) must not leak an in-flight inference: cancel it so the worker
/// thread unblocks, sees its closed job channel, and exits — releasing the
/// model instead of running to completion beside a freshly loaded one.
impl Drop for WorkerHandle {
    fn drop(&mut self) {
        self.cancel.cancel();
        // Registering (rather than joining here) keeps the UI thread free:
        // the wait happens on whichever worker thread next loads a model.
        if self.life.strong_count() > 0 {
            RETIRING
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(self.life.clone());
        }
    }
}

impl WorkerHandle {
    pub fn spawn(profile: &SpeechProfile, backend: SpeechBackend) -> std::io::Result<Self> {
        let (job_tx, job_rx) = channel();
        let (event_tx, event_rx) = channel();
        let settings = Settings::from_profile(profile, backend);
        let cancel = CancelToken::new();
        let worker_cancel = cancel.clone();
        let life = Arc::new(WorkerLife);
        let life_weak = Arc::downgrade(&life);
        thread::Builder::new().name("speech-transcribe".into()).spawn(move || {
            // Dropped last, after `worker_loop` released the model.
            let _life = life;
            worker_loop(&settings, &worker_cancel, &job_rx, &event_tx);
        })?;
        Ok(Self {
            job_tx,
            event_rx,
            cancel,
            life: life_weak,
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
    // Strong handle for this worker: keeps its model resident (and the shared
    // cache entry upgradable) for as long as the worker lives.
    let mut model: Option<Arc<Model>> = None;
    while let Ok(job) = job_rx.recv() {
        if cancel.is_cancelled() {
            return;
        }
        let target = job.target;
        let result = ensure_session(settings, cancel, &mut session, &mut model).and_then(|loaded_backend| {
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
    model_slot: &mut Option<Arc<Model>>,
) -> Result<Option<String>, String> {
    if session.is_some() {
        return Ok(None);
    }
    if settings.model_path.trim().is_empty() {
        return Err("no speech model configured (features.speech.model)".to_string());
    }
    if cancel.is_cancelled() {
        return Err("speech worker replaced before model load".to_string());
    }
    // Before the load lock, not under it: an outgoing worker may itself be
    // waiting to load, and holding the permit while waiting for it to exit
    // would deadlock the pair. Checking `cancel` first also keeps a retiring
    // worker from waiting on its own cohort.
    await_retiring_workers();
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
        let mut guard = LOADED_MODELS.lock().unwrap_or_else(PoisonError::into_inner);
        let map = guard.get_or_insert_with(HashMap::new);
        // Drop entries whose last strong handle went away with its workers.
        map.retain(|_, weak| weak.strong_count() > 0);
        map.get(&cache_key).and_then(Weak::upgrade)
    };
    let model = if let Some(model) = cached {
        model
    } else {
        let loaded = Model::load_with(&settings.model_path, &options)
            .map_err(|error| format!("failed to load speech model `{}`: {error}", settings.model_path))?;
        if cancel.is_cancelled() {
            // Replaced while loading: drop the model BEFORE releasing the load
            // lock so the waiting replacement never overlaps residence with it.
            drop(loaded);
            return Err("speech worker replaced during model load".to_string());
        }
        let model = Arc::new(loaded);
        LOADED_MODELS
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get_or_insert_with(HashMap::new)
            .insert(cache_key, Arc::downgrade(&model));
        model
    };
    if cancel.is_cancelled() {
        return Err("speech worker replaced during model load".to_string());
    }
    let backend = model.backend().clone();
    tracing::info!(model = %settings.model_path, %backend, "speech model ready");
    let mut new_session = model
        .session()
        .map_err(|error| format!("failed to create transcription session: {error}"))?;
    new_session.set_cancel_token(cancel);
    *session = Some(new_session);
    // Retain the strong handle so this worker's model stays resident (and
    // shareable) until the worker itself goes away.
    *model_slot = Some(model);
    Ok(Some(backend))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use transcribe_cpp::{Model, RunOptions};

    use super::super::capture::to_mono_16k;
    use super::{RETIRING, WorkerLife, await_retiring_workers};

    /// A loader must not proceed while a cancelled worker is still running:
    /// that worker still holds its multi-GB model, and loading beside it is
    /// what exhausts GPU memory on a live config change.
    #[test]
    fn loader_waits_for_a_retiring_worker_to_release_its_model() {
        let life = Arc::new(WorkerLife);
        RETIRING
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(Arc::downgrade(&life));

        let releaser = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(120));
            drop(life); // the worker thread returning
        });

        let started = Instant::now();
        await_retiring_workers();
        let waited = started.elapsed();
        releaser.join().expect("releaser thread panicked");

        assert!(
            waited >= Duration::from_millis(100),
            "returned after {waited:?} — did not wait for the worker to exit"
        );
        assert!(
            RETIRING
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_empty(),
            "the exited worker must be pruned from the retiring list"
        );
    }

    /// With nothing retiring, loading must not pay any wait at all.
    #[test]
    fn loader_does_not_wait_when_no_worker_is_retiring() {
        let started = Instant::now();
        await_retiring_workers();
        assert!(started.elapsed() < Duration::from_secs(1));
    }

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
