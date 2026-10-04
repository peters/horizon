//! Bounded image decoding and persistence outside the native UI event loop.
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, sync_channel};
use std::sync::{Arc, Condvar, Mutex};

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

#[derive(Default)]
struct QueueState {
    jobs: VecDeque<Job>,
    closed: bool,
}
#[derive(Default)]
struct Queue {
    state: Mutex<QueueState>,
    ready: Condvar,
    generation: AtomicU64,
}
impl Queue {
    fn reset(&self, generation: u64) {
        let mut state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        self.generation.store(generation, Ordering::Release);
        state.jobs.clear();
        self.ready.notify_all();
    }
}

pub(super) struct TransferWorker {
    queue: Arc<Queue>,
    incoming: Option<Receiver<Completion>>,
    workers: Vec<std::thread::JoinHandle<()>>,
}

impl TransferWorker {
    pub(super) fn start(observed: &ObservedKeyboardInputs) -> std::io::Result<Self> {
        let wake = observed.clone();
        let decoder = observed.clone();
        let pool = Self::spawn(
            move |event, generation, current| decode(&decoder, generation, event, current),
            move || wake.wake_native_input(),
        )?;
        pool.queue.reset(observed.native_transfer_generation());
        let weak = Arc::downgrade(&pool.queue);
        observed.native_persistence_resetter(move |generation| {
            if let Some(queue) = weak.upgrade() {
                queue.reset(generation);
            }
        });
        Ok(pool)
    }

    fn spawn(
        decode: impl Fn(Event, u64, &dyn Fn() -> bool) -> Completed + Send + Sync + 'static,
        wake: impl Fn() + Send + Sync + 'static,
    ) -> std::io::Result<Self> {
        let (done, incoming) = sync_channel(4);
        let queue = Arc::new(Queue::default());
        let decode = Arc::new(decode);
        let wake = Arc::new(wake);
        let mut pool = Self {
            queue: queue.clone(),
            incoming: Some(incoming),
            workers: Vec::new(),
        };
        for index in 0..4 {
            let (queue, done, decode, wake) = (queue.clone(), done.clone(), decode.clone(), wake.clone());
            pool.workers.push(
                std::thread::Builder::new()
                    .name(format!("native-image-files-{index}"))
                    .spawn(move || {
                        loop {
                            let job = {
                                let mut state = queue.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                                while state.jobs.is_empty() && !state.closed {
                                    state = queue
                                        .ready
                                        .wait(state)
                                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                                }
                                if state.closed {
                                    break;
                                }
                                state.jobs.pop_front()
                            };
                            let Some(job) = job else {
                                continue;
                            };
                            let current = || queue.generation.load(Ordering::Acquire) == job.generation;
                            if !current() {
                                continue;
                            }
                            let completion = Completion {
                                generation: job.generation,
                                event: decode(job.event, job.generation, &current),
                            };
                            if !current() {
                                continue;
                            }
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
        let mut state = self
            .queue
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.closed || state.jobs.len() >= 4 || self.queue.generation.load(Ordering::Acquire) != generation {
            return false;
        }
        state.jobs.push_back(Job { generation, event });
        self.queue.ready.notify_one();
        true
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
        {
            let mut state = self
                .queue
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state.closed = true;
            state.jobs.clear();
            self.queue.generation.fetch_add(1, Ordering::AcqRel);
            self.queue.ready.notify_all();
        }
        self.incoming.take();
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}

fn decode(observed: &ObservedKeyboardInputs, generation: u64, event: Event, current: &dyn Fn() -> bool) -> Completed {
    match event {
        Event::Paste {
            recipient,
            mime,
            bytes,
            text,
            fallback,
        } => {
            let payload = observed
                .decode_native_transfer(generation, &mime, &bytes)
                .ok()
                .map(Payload::Files)
                .or_else(|| {
                    text.or_else(|| {
                        smithay_clipboard::native::is_text_mime(&mime)
                            .then(|| smithay_clipboard::native::decode_clipboard_text(&mime, &bytes))
                    })
                    .or_else(|| fallback.and_then(|fallback| fallback.read_text_while(current)))
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
            paths: observed.decode_native_transfer(generation, &mime, &bytes).ok(),
        },
        _ => unreachable!("only clipboard and drop payloads are submitted"),
    }
}

impl KeyboardAwareApp<'_> {
    pub(super) fn request_native_paste(&mut self, surface: u64) -> bool {
        if self.transfer_worker.is_none() {
            return false;
        }
        let Some(recipient) = self.observed_keyboard_inputs.native_paste_request(surface) else {
            return false;
        };
        if self
            .clipboard
            .as_mut()
            .is_some_and(|bridge| bridge.request_paste(surface, recipient))
        {
            return true;
        }
        self.observed_keyboard_inputs.cancel_native_paste_request(recipient);
        false
    }

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
                    if self.observed_keyboard_inputs.native_window(surface).is_none() {
                        continue;
                    }
                    let Some(token) = self
                        .observed_keyboard_inputs
                        .native_drop_position(surface, position, paths)
                    else {
                        continue;
                    };
                    self.forward_drop_position(event_loop, surface, position);
                    self.inner.window_event(
                        event_loop,
                        winit::window::WindowId::from(surface),
                        WindowEvent::DroppedFile(token),
                    );
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
            move |_, _, _| {
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
    fn session_reset_cancels_active_reads_and_queued_decoding_before_new_work() {
        let (entered, started) = channel();
        let (wake, notifications) = channel();
        let decoded = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let count = decoded.clone();
        let worker = TransferWorker::spawn(
            move |_, generation, current| {
                count.fetch_add(1, Ordering::Relaxed);
                if generation == 0 {
                    entered.send(()).unwrap();
                    while current() {
                        std::thread::sleep(Duration::from_millis(1));
                    }
                }
                Completed::Paste {
                    recipient: 42,
                    payload: None,
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
        worker.queue.reset(1);
        assert!(!worker.submit(0, paste()));
        assert!(worker.submit(1, paste()));
        notifications.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(matches!(
            worker.poll(1).as_slice(),
            [Completed::Paste { recipient: 42, .. }]
        ));
        assert_eq!(decoded.load(Ordering::Relaxed), 5);
        drop(worker);
    }

    #[test]
    fn a_waiting_fallback_does_not_delay_another_image_completion() {
        let (entered, started) = channel();
        let (release, gate) = channel();
        let gate = std::sync::Mutex::new(gate);
        let (wake, notifications) = channel();
        let worker = TransferWorker::spawn(
            move |event, _, _| {
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
            move |_, _, _| {
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
            matches!(decode(&observed, 0, event, &|| true), Completed::Paste { recipient: 42, payload: Some(Payload::Text(text)) } if text == "fallback")
        );
    }
}
