use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::time::{Duration, Instant};

use horizon_core::speech_model::{SpeechModelInfo, read_speech_model_info};

const EDIT_DEBOUNCE: Duration = Duration::from_millis(250);
const REFRESH_INTERVAL: Duration = Duration::from_millis(500);

type ModelFileIdentity = (u64, u128);

/// App-owned cache that keeps all model filesystem work off the UI thread.
pub(in crate::app) struct SpeechModelInfoCache {
    worker: Option<ModelInfoWorker>,
    current_path: String,
    current_identity: Option<ModelFileIdentity>,
    current_state: SpeechModelInfoState,
    last_checked: Option<Instant>,
    path_changed_at: Option<Instant>,
    in_flight: bool,
}

#[derive(Clone)]
pub(super) enum SpeechModelInfoState {
    Pending,
    Available(SpeechModelInfo),
    Unavailable,
}

struct ModelInfoWorker {
    request_tx: Sender<ModelInfoRequest>,
    result_rx: Receiver<ModelInfoResult>,
}

struct ModelInfoRequest {
    path: String,
    previous_identity: Option<ModelFileIdentity>,
    repaint_ctx: egui::Context,
}

struct ModelInfoResult {
    path: String,
    outcome: ModelInfoOutcome,
}

enum ModelInfoOutcome {
    Unchanged,
    Loaded {
        identity: ModelFileIdentity,
        info: Option<SpeechModelInfo>,
    },
    Unavailable,
}

impl SpeechModelInfoCache {
    pub(in crate::app) const fn new() -> Self {
        Self {
            worker: None,
            current_path: String::new(),
            current_identity: None,
            current_state: SpeechModelInfoState::Unavailable,
            last_checked: None,
            path_changed_at: None,
            in_flight: false,
        }
    }

    pub(super) fn model_info(&mut self, ctx: &egui::Context, path: &str) -> SpeechModelInfoState {
        self.model_info_at(ctx, path, Instant::now())
    }

    fn model_info_at(&mut self, ctx: &egui::Context, path: &str, now: Instant) -> SpeechModelInfoState {
        self.poll_result(now);

        let trimmed = path.trim();
        if self.current_path != trimmed {
            self.current_path.clear();
            self.current_path.push_str(trimmed);
            self.current_identity = None;
            self.current_state = SpeechModelInfoState::Pending;
            self.last_checked = None;
            self.path_changed_at = Some(now);
        }
        if trimmed.is_empty() {
            self.current_state = SpeechModelInfoState::Unavailable;
            return self.current_state.clone();
        }

        let debounce_remaining = self
            .path_changed_at
            .and_then(|changed_at| EDIT_DEBOUNCE.checked_sub(now.saturating_duration_since(changed_at)))
            .filter(|remaining| !remaining.is_zero());
        let refresh_remaining = self
            .last_checked
            .and_then(|checked_at| REFRESH_INTERVAL.checked_sub(now.saturating_duration_since(checked_at)))
            .filter(|remaining| !remaining.is_zero());

        if !self.in_flight {
            if let Some(remaining) = debounce_remaining.or(refresh_remaining) {
                ctx.request_repaint_after(remaining);
            } else {
                self.path_changed_at = None;
                self.start_request(ctx, now);
            }
        }

        self.current_state.clone()
    }

    fn poll_result(&mut self, now: Instant) {
        let Some(worker) = self.worker.as_ref() else {
            return;
        };
        match worker.result_rx.try_recv() {
            Ok(result) => {
                self.in_flight = false;
                if result.path == self.current_path {
                    match result.outcome {
                        ModelInfoOutcome::Unchanged => {}
                        ModelInfoOutcome::Loaded { identity, info } => {
                            self.current_identity = Some(identity);
                            self.current_state =
                                info.map_or(SpeechModelInfoState::Unavailable, SpeechModelInfoState::Available);
                        }
                        ModelInfoOutcome::Unavailable => {
                            self.current_identity = None;
                            self.current_state = SpeechModelInfoState::Unavailable;
                        }
                    }
                    self.last_checked = Some(now);
                }
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                self.worker = None;
                self.in_flight = false;
            }
        }
    }

    fn start_request(&mut self, ctx: &egui::Context, now: Instant) {
        if self.worker.is_none() {
            self.worker = ModelInfoWorker::spawn();
        }
        let Some(worker) = self.worker.as_ref() else {
            self.current_state = SpeechModelInfoState::Unavailable;
            self.last_checked = Some(now);
            ctx.request_repaint_after(REFRESH_INTERVAL);
            return;
        };
        let request = ModelInfoRequest {
            path: self.current_path.clone(),
            previous_identity: self.current_identity,
            repaint_ctx: ctx.clone(),
        };
        if worker.request_tx.send(request).is_ok() {
            self.in_flight = true;
        } else {
            self.worker = None;
            self.current_state = SpeechModelInfoState::Unavailable;
            self.last_checked = Some(now);
            ctx.request_repaint_after(REFRESH_INTERVAL);
        }
    }
}

impl ModelInfoWorker {
    fn spawn() -> Option<Self> {
        let (request_tx, request_rx) = mpsc::channel();
        let (result_tx, result_rx) = mpsc::channel();
        let spawn = std::thread::Builder::new()
            .name("speech-model-info".to_string())
            .spawn(move || run_model_info_worker(&request_rx, &result_tx));
        match spawn {
            Ok(_handle) => Some(Self { request_tx, result_rx }),
            Err(error) => {
                tracing::warn!(%error, "failed to spawn speech model metadata worker");
                None
            }
        }
    }
}

fn run_model_info_worker(request_rx: &Receiver<ModelInfoRequest>, result_tx: &Sender<ModelInfoResult>) {
    while let Ok(request) = request_rx.recv() {
        let expanded = horizon_core::dir_search::expand_tilde(&request.path);
        let outcome = std::fs::metadata(&expanded).map_or(ModelInfoOutcome::Unavailable, |metadata| {
            let modified = metadata
                .modified()
                .ok()
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |duration| duration.as_nanos());
            let identity = (metadata.len(), modified);
            if request.previous_identity == Some(identity) {
                ModelInfoOutcome::Unchanged
            } else {
                ModelInfoOutcome::Loaded {
                    identity,
                    info: read_speech_model_info(&expanded),
                }
            }
        });
        let result = ModelInfoResult {
            path: request.path,
            outcome,
        };
        let sent = result_tx.send(result).is_ok();
        request.repaint_ctx.request_repaint();
        if !sent {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::{self, TryRecvError};

    use horizon_core::speech_model::SpeechModelInfo;

    use super::{
        EDIT_DEBOUNCE, ModelInfoOutcome, ModelInfoResult, ModelInfoWorker, REFRESH_INTERVAL, SpeechModelInfoCache,
        SpeechModelInfoState,
    };

    fn cache_with_test_worker() -> (
        SpeechModelInfoCache,
        mpsc::Receiver<super::ModelInfoRequest>,
        mpsc::Sender<ModelInfoResult>,
    ) {
        let (request_tx, request_rx) = mpsc::channel();
        let (result_tx, result_rx) = mpsc::channel();
        let mut cache = SpeechModelInfoCache::new();
        cache.worker = Some(ModelInfoWorker { request_tx, result_rx });
        (cache, request_rx, result_tx)
    }

    fn model_info(language: &str) -> SpeechModelInfo {
        SpeechModelInfo {
            languages: vec![language.to_string()],
            ..SpeechModelInfo::default()
        }
    }

    #[test]
    fn path_changes_are_debounced_and_only_one_lookup_is_in_flight() {
        let ctx = egui::Context::default();
        let start = std::time::Instant::now();
        let (mut cache, request_rx, result_tx) = cache_with_test_worker();

        assert!(matches!(
            cache.model_info_at(&ctx, "/models/slow.gguf", start),
            SpeechModelInfoState::Pending
        ));
        assert!(matches!(request_rx.try_recv(), Err(TryRecvError::Empty)));

        let after_debounce = start + EDIT_DEBOUNCE;
        assert!(matches!(
            cache.model_info_at(&ctx, "/models/slow.gguf", after_debounce),
            SpeechModelInfoState::Pending
        ));
        let slow_request = request_rx.try_recv().expect("first lookup request");
        assert_eq!(slow_request.path, "/models/slow.gguf");

        let changed_at = after_debounce + std::time::Duration::from_millis(1);
        assert!(matches!(
            cache.model_info_at(&ctx, "/models/current.gguf", changed_at),
            SpeechModelInfoState::Pending
        ));
        assert!(matches!(
            cache.model_info_at(&ctx, "/models/current.gguf", changed_at + EDIT_DEBOUNCE),
            SpeechModelInfoState::Pending
        ));
        assert!(matches!(request_rx.try_recv(), Err(TryRecvError::Empty)));

        result_tx
            .send(ModelInfoResult {
                path: slow_request.path,
                outcome: ModelInfoOutcome::Loaded {
                    identity: (10, 1),
                    info: Some(model_info("stale")),
                },
            })
            .expect("stale result");
        assert!(matches!(
            cache.model_info_at(&ctx, "/models/current.gguf", changed_at + EDIT_DEBOUNCE),
            SpeechModelInfoState::Pending
        ));
        let current_request = request_rx.try_recv().expect("coalesced current lookup");
        assert_eq!(current_request.path, "/models/current.gguf");
        assert_eq!(current_request.previous_identity, None);
    }

    #[test]
    fn refresh_invalidates_metadata_when_storage_is_unavailable() {
        let ctx = egui::Context::default();
        let start = std::time::Instant::now();
        let (mut cache, request_rx, result_tx) = cache_with_test_worker();
        let expected = model_info("en");

        assert!(matches!(
            cache.model_info_at(&ctx, "/models/en.gguf", start),
            SpeechModelInfoState::Pending
        ));
        assert!(matches!(
            cache.model_info_at(&ctx, "/models/en.gguf", start + EDIT_DEBOUNCE),
            SpeechModelInfoState::Pending
        ));
        let initial_request = request_rx.try_recv().expect("initial lookup");
        result_tx
            .send(ModelInfoResult {
                path: initial_request.path,
                outcome: ModelInfoOutcome::Loaded {
                    identity: (20, 2),
                    info: Some(expected.clone()),
                },
            })
            .expect("initial result");

        let loaded_at = start + EDIT_DEBOUNCE + std::time::Duration::from_millis(1);
        assert!(matches!(
            cache.model_info_at(&ctx, "/models/en.gguf", loaded_at),
            SpeechModelInfoState::Available(info) if info == expected
        ));
        let refresh_at = loaded_at + REFRESH_INTERVAL;
        assert!(matches!(
            cache.model_info_at(&ctx, "/models/en.gguf", refresh_at),
            SpeechModelInfoState::Available(info) if info == expected
        ));
        let refresh_request = request_rx.try_recv().expect("refresh lookup");
        assert_eq!(refresh_request.previous_identity, Some((20, 2)));

        result_tx
            .send(ModelInfoResult {
                path: refresh_request.path,
                outcome: ModelInfoOutcome::Unavailable,
            })
            .expect("unavailable result");
        assert!(matches!(
            cache.model_info_at(
                &ctx,
                "/models/en.gguf",
                refresh_at + std::time::Duration::from_millis(1),
            ),
            SpeechModelInfoState::Unavailable
        ));
    }
}
