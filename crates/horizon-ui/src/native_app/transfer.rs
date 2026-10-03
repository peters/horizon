//! Bounded image decoding and persistence outside the native UI event loop.
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};

use smithay_clipboard::native::Event;
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;

use super::{KeyboardAwareApp, ObservedKeyboardInputs};

type Paths = Vec<std::path::PathBuf>;
struct Job {
    generation: u64,
    event: Event,
}
struct Completion {
    generation: u64,
    event: Completed,
}
enum Completed {
    Paste {
        recipient: u64,
        payload: Option<Payload>,
    },
    Drop {
        surface: u64,
        position: [f64; 2],
        paths: Option<Paths>,
    },
}
enum Payload {
    Files(Paths),
    Text(String),
}

pub(super) struct TransferWorker {
    sender: Option<SyncSender<Job>>,
    incoming: Option<Receiver<Completion>>,
    workers: Vec<std::thread::JoinHandle<()>>,
}

impl TransferWorker {
    pub(super) fn start(observed: ObservedKeyboardInputs) -> std::io::Result<Self> {
        let wake = observed.clone();
        Self::spawn(move |event| decode(&observed, event), move || wake.wake_native_input())
    }

    fn spawn(
        decode: impl Fn(Event) -> Completed + Send + Sync + 'static,
        wake: impl Fn() + Send + Sync + 'static,
    ) -> std::io::Result<Self> {
        let (sender, jobs) = sync_channel::<Job>(4);
        let (done, incoming) = sync_channel(4);
        let jobs = std::sync::Arc::new(std::sync::Mutex::new(jobs));
        let decode = std::sync::Arc::new(decode);
        let wake = std::sync::Arc::new(wake);
        let mut pool = Self {
            sender: Some(sender),
            incoming: Some(incoming),
            workers: Vec::new(),
        };
        for index in 0..4 {
            let (jobs, done, decode, wake) = (jobs.clone(), done.clone(), decode.clone(), wake.clone());
            pool.workers.push(
                std::thread::Builder::new()
                    .name(format!("native-image-files-{index}"))
                    .spawn(move || {
                        loop {
                            let job = jobs.lock().unwrap_or_else(std::sync::PoisonError::into_inner).recv();
                            let Ok(job) = job else {
                                break;
                            };
                            let completion = Completion {
                                generation: job.generation,
                                event: decode(job.event),
                            };
                            if done.send(completion).is_err() {
                                break;
                            }
                            wake();
                        }
                    })?,
            );
        }
        Ok(pool)
    }

    fn submit(&self, generation: u64, event: Event) -> bool {
        self.sender
            .as_ref()
            .is_some_and(|sender| sender.try_send(Job { generation, event }).is_ok())
    }

    fn poll(&self, generation: u64) -> Vec<Completed> {
        self.incoming.as_ref().map_or_else(Vec::new, |incoming| {
            incoming
                .try_iter()
                .filter(|completion| completion.generation == generation)
                .map(|completion| completion.event)
                .collect()
        })
    }
}

impl Drop for TransferWorker {
    fn drop(&mut self) {
        self.sender.take();
        self.incoming.take();
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}

fn decode(observed: &ObservedKeyboardInputs, event: Event) -> Completed {
    match event {
        Event::Paste {
            recipient,
            mime,
            bytes,
            text,
            fallback,
        } => {
            let payload = observed
                .decode_native_transfer(&mime, &bytes)
                .ok()
                .map(Payload::Files)
                .or_else(|| {
                    text.or_else(|| {
                        smithay_clipboard::native::is_text_mime(&mime)
                            .then(|| smithay_clipboard::native::decode_clipboard_text(&mime, &bytes))
                    })
                    .or_else(|| fallback.and_then(smithay_clipboard::native::TextFallback::read_text))
                    .map(Payload::Text)
                });
            Completed::Paste { recipient, payload }
        }
        Event::Drop {
            surface,
            position,
            mime,
            bytes,
        } => Completed::Drop {
            surface,
            position,
            paths: observed.decode_native_transfer(&mime, &bytes).ok(),
        },
        _ => unreachable!("only clipboard and drop payloads are submitted"),
    }
}

impl KeyboardAwareApp<'_> {
    pub(super) fn queue_native_transfer(&mut self, event_loop: &ActiveEventLoop, event: Event) {
        let recipient = if let Event::Paste { recipient, .. } = &event {
            Some(*recipient)
        } else {
            None
        };
        if let Event::Drop { surface, .. } = &event {
            if self.observed_keyboard_inputs.native_window(*surface).is_none() {
                return;
            }
            self.inner.window_event(
                event_loop,
                winit::window::WindowId::from(*surface),
                WindowEvent::HoveredFileCancelled,
            );
        }
        let generation = self.observed_keyboard_inputs.native_transfer_generation();
        if !self
            .transfer_worker
            .as_ref()
            .is_some_and(|worker| worker.submit(generation, event))
        {
            if let Some(recipient) = recipient {
                self.observed_keyboard_inputs.cancel_native_paste_request(recipient);
            }
            tracing::debug!("native file persistence rejected: worker unavailable or queue full");
        }
    }

    pub(super) fn drain_native_completions(&mut self, event_loop: &ActiveEventLoop) {
        let generation = self.observed_keyboard_inputs.native_transfer_generation();
        let completions = self
            .transfer_worker
            .as_ref()
            .map_or_else(Vec::new, |worker| worker.poll(generation));
        for completion in completions {
            match completion {
                Completed::Paste {
                    recipient,
                    payload: Some(Payload::Files(paths)),
                } => self.observed_keyboard_inputs.native_paste(recipient, paths),
                Completed::Paste {
                    recipient,
                    payload: Some(Payload::Text(text)),
                } => self.observed_keyboard_inputs.native_paste_text(recipient, text),
                Completed::Paste {
                    recipient,
                    payload: None,
                } => self.observed_keyboard_inputs.cancel_native_paste_request(recipient),
                Completed::Drop {
                    surface,
                    position,
                    paths: Some(paths),
                } => {
                    if self.observed_keyboard_inputs.native_window(surface).is_none()
                        || !self
                            .observed_keyboard_inputs
                            .native_drop_position(surface, position, paths.clone())
                    {
                        continue;
                    }
                    self.forward_drop_position(event_loop, surface, position);
                    for path in paths {
                        self.inner.window_event(
                            event_loop,
                            winit::window::WindowId::from(surface),
                            WindowEvent::DroppedFile(path),
                        );
                    }
                }
                Completed::Drop { paths: None, .. } => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::channel;
    use std::time::Duration;

    fn paste() -> Event {
        Event::Paste {
            recipient: 42,
            mime: "image/png".into(),
            bytes: vec![],
            text: None,
            fallback: None,
        }
    }

    #[test]
    fn blocked_persistence_never_blocks_submission_and_stale_completions_are_discarded() {
        let (entered, started) = channel();
        let (release, gate) = channel();
        let gate = std::sync::Mutex::new(gate);
        let (wake, notifications) = channel();
        let worker = TransferWorker::spawn(
            move |_| {
                entered.send(()).unwrap();
                gate.lock().unwrap().recv().unwrap();
                Completed::Paste {
                    recipient: 42,
                    payload: Some(Payload::Text("fallback".into())),
                }
            },
            move || {
                wake.send(()).unwrap();
            },
        )
        .unwrap();
        for _ in 0..4 {
            assert!(worker.submit(0, paste()));
            started.recv_timeout(Duration::from_secs(2)).unwrap();
        }
        for _ in 0..4 {
            assert!(worker.submit(0, paste()));
        }
        assert!(!worker.submit(0, paste()));
        for _ in 0..8 {
            release.send(()).unwrap();
        }
        for _ in 0..8 {
            notifications.recv_timeout(Duration::from_secs(2)).unwrap();
            assert!(worker.poll(1).is_empty());
        }
        drop(worker);
    }

    #[test]
    fn a_waiting_fallback_does_not_delay_another_image_completion() {
        let (entered, started) = channel();
        let (release, gate) = channel();
        let gate = std::sync::Mutex::new(gate);
        let (wake, notifications) = channel();
        let worker = TransferWorker::spawn(
            move |event| {
                let Event::Paste { recipient, .. } = event else {
                    panic!("fixture")
                };
                if recipient == 42 {
                    entered.send(()).unwrap();
                    gate.lock().unwrap().recv().unwrap();
                }
                Completed::Paste {
                    recipient,
                    payload: None,
                }
            },
            move || {
                wake.send(()).unwrap();
            },
        )
        .unwrap();
        assert!(worker.submit(0, paste()));
        started.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(worker.submit(
            0,
            Event::Paste {
                recipient: 43,
                mime: "image/png".into(),
                bytes: vec![],
                text: None,
                fallback: None
            }
        ));
        notifications.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(matches!(
            worker.poll(0).as_slice(),
            [Completed::Paste { recipient: 43, .. }]
        ));
        release.send(()).unwrap();
        drop(worker);
    }

    #[test]
    fn shutdown_releases_a_worker_blocked_on_unconsumed_results() {
        let (entered, started) = channel();
        let worker = TransferWorker::spawn(
            move |_| {
                entered.send(()).unwrap();
                Completed::Paste {
                    recipient: 42,
                    payload: None,
                }
            },
            || {},
        )
        .unwrap();
        for _ in 0..8 {
            assert!(worker.submit(0, paste()));
            started.recv_timeout(Duration::from_secs(2)).unwrap();
        }
        drop(worker);
    }

    #[test]
    fn corrupt_signed_image_keeps_same_offer_text_fallback() {
        let observed = ObservedKeyboardInputs::default();
        let event = Event::Paste {
            recipient: 42,
            mime: "image/png".into(),
            bytes: b"\x89PNG\r\n\x1a\ninvalid".to_vec(),
            text: Some("fallback".into()),
            fallback: None,
        };
        assert!(
            matches!(decode(&observed, event), Completed::Paste { recipient: 42, payload: Some(Payload::Text(text)) } if text == "fallback")
        );
    }
}
