//! The session of a browser host: the browser, the page, and the link of this
//! computer.
//!
//! The session belongs to the machine of the browser, not to a link. One link
//! sends what a person and an agent ask for and receives the pushed picture; a
//! link that ends leaves the browser and the page running, and the next link
//! attaches to the same page, with the text that a person typed still in it.
//! That is the promise of a Firefox host, and it is why the session is a process
//! of its own rather than the life of one `ssh` child.
//!
//! The page's own driver (geckodriver and Firefox of this machine) produces
//! screenshots; this module re-encodes one of them as a JPEG of the setting's
//! quality and pushes it when it changed. One frame is in flight: the next one
//! waits for the acknowledgement of the last, so a slow link shows fewer and
//! newer frames instead of a queue of the past.

use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;

use horizon_browser::host_driver::{
    FrameGate, HostDriverFrame, HostDriverFrameFormat, HostDriverPage, HostDriverPageState, HostDriverReply,
    HostDriverRequest, HostDriverWelcome, HostSystem, MAX_HOST_FRAME_SIDE,
};
use horizon_browser::{
    BrowserCommand, BrowserConfig, BrowserEvent, BrowserSession, BrowserSessionConfig, FrameData, FrameSlot,
    VideoCaptureHandle, start_session,
};

use super::coordination::WireCoordination;
use super::{HostDriverEngine, HostDriverOptions, Link, LinkOutcome, Next, Step, paths};

/// How long the session of a browser host waits for a link before it stops the
/// browser and ends. A cut link comes back in seconds; a person who closed the
/// panel and went home does not leave a Firefox running for a day.
pub(super) const LINK_IDLE: Duration = Duration::from_secs(600);

/// How long one pass of the loop waits for a request before it looks at the
/// page and the frame again.
pub(super) const PASS_WAIT: Duration = Duration::from_millis(20);

/// Most events of the page that one pass applies, so a page that floods its
/// channel cannot starve the link.
const EVENT_BURST: usize = 64;

/// Bound of the stop of one session of a browser host.
const STOP_TIMEOUT: Duration = Duration::from_secs(5);

/// The viewport of a session that no panel has sized yet.
const DEFAULT_VIEWPORT: (u32, u32) = (1280, 800);

/// What the browser of a host session is, as the page loop sees it.
///
/// A test drives the loop with a page of its own, so every rule of the link (one
/// frame in flight, the order of the inputs, the page that came back) is tested
/// without Firefox.
pub(super) trait HostPage: Send {
    fn send(&self, command: BrowserCommand) -> bool;
    fn next_event(&self) -> Option<BrowserEvent>;
    fn shutdown(&mut self);
}

/// The browser of this machine behind a host session.
struct LocalPage {
    session: Option<Box<BrowserSession>>,
}

impl HostPage for LocalPage {
    fn send(&self, command: BrowserCommand) -> bool {
        self.session.as_ref().is_some_and(|session| session.send(command))
    }

    fn next_event(&self) -> Option<BrowserEvent> {
        self.session.as_ref()?.event_rx.try_recv().ok()
    }

    fn shutdown(&mut self) {
        if let Some(session) = self.session.take() {
            let signal = (*session).shutdown_signal();
            let _ = signal.wait(STOP_TIMEOUT) || signal.force_cleanup(STOP_TIMEOUT);
        }
    }
}

/// One session of a browser host, which outlives the links that attach to it.
pub(super) struct HostSession {
    page: Box<dyn HostPage>,
    slot: Arc<FrameSlot>,
    coordination: Arc<WireCoordination>,
    outbox: mpsc::Receiver<HostDriverReply>,
    quality: u32,
    viewport: (u32, u32),
    /// Whether this link came back to the page of the link before.
    page_state: HostDriverPageState,
    loading: bool,
    /// The address, title and resumed mark that this session last sent.
    sent_page: HostDriverPage,
    /// Links that served this session, so the first answer can say that the page
    /// is new and every later one that it is the page of the link before.
    links: u32,
    /// The frame sequence that this session last sent, so a pass does not send a
    /// picture that the person already sees.
    last_sent: Option<u64>,
}

impl HostSession {
    /// Start the browser of this machine in the named profile.
    ///
    /// # Errors
    /// The profile folder cannot be created, or the browser or its driver does
    /// not start. The words name what a person must install.
    pub(super) fn start(options: &HostDriverOptions) -> Result<Self, String> {
        let profile_root = paths::profile_root(options.profile())
            .map_err(|error| error.to_string())?
            .to_string_lossy()
            .into_owned();
        paths::ensure_profile_root(std::path::Path::new(&profile_root)).map_err(|error| error.to_string())?;
        let (sender, outbox) = mpsc::channel();
        let coordination = WireCoordination::new(sender);
        let slot = Arc::new(FrameSlot::new());
        let config = BrowserConfig {
            backend: options.engine().backend(),
            headless: true,
            profile_root: Some(profile_root.into()),
            ..BrowserConfig::default()
        };
        let session = start_session(BrowserSessionConfig {
            browser: config,
            panel_local_id: options.profile().to_string(),
            initial_url: None,
            width: DEFAULT_VIEWPORT.0,
            height: DEFAULT_VIEWPORT.1,
            frame_slot: Arc::clone(&slot),
            coordination: Some(Arc::clone(&coordination) as Arc<dyn horizon_browser::BrowserCoordination>),
            capture_directory: None,
            video: Arc::new(VideoCaptureHandle::default()),
            remote: None,
            attach: None,
            resume_target: None,
        })
        .map_err(|error| {
            format!(
                "the browser of this machine did not start in {}: {error}",
                options.engine().backend().display_name()
            )
        })?;
        tracing::info!(
            profile = options.profile(),
            engine = options.engine().as_str(),
            "the session of a browser host started"
        );
        Ok(Self {
            page: Box::new(LocalPage {
                session: Some(Box::new(session)),
            }),
            slot,
            coordination,
            outbox,
            quality: horizon_browser::host_driver::DEFAULT_HOST_FRAME_QUALITY,
            viewport: DEFAULT_VIEWPORT,
            page_state: HostDriverPageState::New,
            loading: true,
            sent_page: HostDriverPage::default(),
            links: 0,
            last_sent: None,
        })
    }

    /// A session over a page of a test, which holds no browser of its own.
    #[cfg(test)]
    pub(super) fn over_page(page: Box<dyn HostPage>, slot: Arc<FrameSlot>) -> Self {
        let (sender, outbox) = mpsc::channel();
        Self {
            page,
            slot,
            coordination: WireCoordination::new(sender),
            outbox,
            quality: horizon_browser::host_driver::DEFAULT_HOST_FRAME_QUALITY,
            viewport: DEFAULT_VIEWPORT,
            page_state: HostDriverPageState::New,
            loading: true,
            sent_page: HostDriverPage::default(),
            links: 0,
            last_sent: None,
        }
    }

    pub(super) fn stop(&mut self) {
        self.page.shutdown();
    }

    /// One reply that the page loop of the driver produced for this link.
    fn take_reply(&self) -> Option<HostDriverReply> {
        self.outbox.try_recv().ok()
    }

    /// Attach a link: the first link shows a new page, and every later one the
    /// page that the session kept.
    fn attach(&mut self, quality: u32) -> HostDriverPageState {
        if self.links > 0 {
            self.page_state = HostDriverPageState::Same;
        }
        self.links = self.links.saturating_add(1);
        self.quality = quality;
        self.page_state
    }

    /// Apply one request of this computer.
    fn apply(&mut self, request: HostDriverRequest, gate: &mut FrameGate) -> LinkOutcome {
        match request {
            HostDriverRequest::Hello { .. } => LinkOutcome::Refuse {
                code: "protocol_hello".to_string(),
                message: "the link of a browser host is greeted once; start the panel again".to_string(),
            },
            HostDriverRequest::Input { input } => self.command(BrowserCommand::Input(input)),
            HostDriverRequest::Navigate { url } => self.command(BrowserCommand::Navigate(url)),
            HostDriverRequest::Reload => self.command(BrowserCommand::Reload),
            HostDriverRequest::Back => self.command(BrowserCommand::Back),
            HostDriverRequest::Forward => self.command(BrowserCommand::Forward),
            HostDriverRequest::Viewport { width, height } => {
                self.viewport = (width, height);
                self.command(BrowserCommand::SetViewport { width, height })
            }
            HostDriverRequest::FrameAck { seq } => {
                if gate.acknowledge(seq) {
                    self.last_sent = Some(seq);
                }
                LinkOutcome::Continue
            }
            HostDriverRequest::Semantic { action_id, action } => {
                self.coordination
                    .queue_action(action_id, "browser_host_driver".to_string(), *action);
                LinkOutcome::Continue
            }
            HostDriverRequest::Signals { signals } => {
                self.coordination.accept_signals(signals);
                LinkOutcome::Continue
            }
            HostDriverRequest::Close => LinkOutcome::Close,
        }
    }

    fn command(&mut self, command: BrowserCommand) -> LinkOutcome {
        if self.page.send(command) {
            LinkOutcome::Continue
        } else {
            LinkOutcome::Note {
                code: "browser_not_ready".to_string(),
                message: "the browser of this machine does not accept a command of the page yet".to_string(),
            }
        }
    }

    /// Apply the events that the page reported since the last pass.
    fn apply_events(&mut self) {
        for _ in 0..EVENT_BURST {
            let Some(event) = self.page.next_event() else {
                break;
            };
            self.apply_event(event);
        }
    }

    fn apply_event(&mut self, event: BrowserEvent) {
        match event {
            BrowserEvent::UrlChanged(url) => {
                self.sent_page.url = url;
                self.loading = true;
            }
            BrowserEvent::Title(title) => self.sent_page.title = title,
            BrowserEvent::Loading(loading) => self.loading = loading,
            BrowserEvent::PageAttached { resumed, .. } => self.sent_page.resumed = resumed,
            _ => {}
        }
    }

    fn page_message(&self) -> HostDriverPage {
        HostDriverPage {
            url: self.sent_page.url.clone(),
            title: self.sent_page.title.clone(),
            resumed: self.page_state.resumed() || self.sent_page.resumed,
            loading: self.loading,
        }
    }

    fn welcome(&self, profile: &str, engine: HostDriverEngine) -> HostDriverWelcome {
        HostDriverWelcome {
            version: horizon_browser::host_driver::HOST_DRIVER_VERSION,
            horizon_version: env!("CARGO_PKG_VERSION").to_string(),
            system: HostSystem::here(),
            engine: engine.as_str().to_string(),
            profile: profile.to_string(),
            page: self.page_state,
            width: self.viewport.0,
            height: self.viewport.1,
            quality: self.quality,
        }
    }

    /// The newest picture, when this session may send one. An acknowledged link
    /// shows the frame after it; the picture of an unchanged page travels
    /// nowhere, because the frame slot publishes a new sequence only when the
    /// page drew something new.
    fn next_frame(&mut self, gate: &mut FrameGate) -> Option<HostDriverReply> {
        if !gate.may_send() {
            return None;
        }
        let frame = self.slot.latest()?;
        if self.last_sent == Some(frame.seq) {
            return None;
        }
        let key = gate.take_key();
        self.last_sent = Some(frame.seq);
        frame_reply(&frame, self.quality, key)
    }
}

/// One pushed picture of `frame`, or a refusal when the picture is too large for
/// a panel.
fn frame_reply(frame: &FrameData, quality: u32, key: bool) -> Option<HostDriverReply> {
    if frame.width > MAX_HOST_FRAME_SIDE || frame.height > MAX_HOST_FRAME_SIDE {
        return Some(HostDriverReply::Refusal {
            code: "frame_too_large".to_string(),
            message: format!(
                "the page of this browser host is {} by {} pixels, and a panel shows at most {MAX_HOST_FRAME_SIDE} pixels on one side",
                frame.width, frame.height
            ),
            fatal: false,
        });
    }
    let mut jpeg = Vec::new();
    let quality = u8::try_from(quality.clamp(1, 100)).unwrap_or(60);
    jpeg_encoder::Encoder::new(&mut jpeg, quality)
        .encode(
            &frame.rgb,
            u16::try_from(frame.width).ok()?,
            u16::try_from(frame.height).ok()?,
            jpeg_encoder::ColorType::Rgb,
        )
        .ok()?;
    Some(HostDriverReply::Frame(HostDriverFrame {
        seq: frame.seq,
        format: HostDriverFrameFormat::Jpeg,
        quality: u32::from(quality),
        width: frame.width,
        height: frame.height,
        key,
        data: base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &jpeg),
    }))
}

/// Serve one link of this computer until it ends.
pub(super) fn serve_link(
    session: &mut HostSession,
    link: &mut Link,
    profile: &str,
    engine: HostDriverEngine,
) -> LinkOutcome {
    let mut gate = FrameGate::new();
    match greet(session, link, profile, engine) {
        LinkOutcome::Continue => {}
        outcome => return finish(link, outcome),
    }
    let outcome = pass(session, link, &mut gate);
    finish(link, outcome)
}

/// Greet this computer: the version of the wire, the system of the host, and the
/// page that this link came back to.
fn greet(session: &mut HostSession, link: &mut Link, profile: &str, engine: HostDriverEngine) -> LinkOutcome {
    let next = link.next(Duration::from_secs(20));
    let request = match request_of(next) {
        Step::Request(request) => request,
        // A link that says nothing for 20 seconds is not a panel of this host.
        Step::Silent | Step::Ended => return LinkOutcome::Closed,
        Step::Refuse(outcome) => return outcome,
    };
    let HostDriverRequest::Hello {
        version,
        system,
        quality,
        ..
    } = request
    else {
        return greeting_refusal("the first message of a link of a browser host is its greeting");
    };
    if version != horizon_browser::host_driver::HOST_DRIVER_VERSION {
        return LinkOutcome::Answer(horizon_browser::host_driver::version_refusal(version));
    }
    if let Some(words) = system.copy_refusal(&HostSystem::here()) {
        return greeting_refusal(&words);
    }
    session.attach(horizon_browser::host_driver::clamp_quality(quality));
    if link
        .send(&HostDriverReply::Welcome(session.welcome(profile, engine)))
        .is_err()
    {
        return LinkOutcome::Closed;
    }
    if link.send(&HostDriverReply::Page(session.page_message())).is_err() {
        return LinkOutcome::Closed;
    }
    LinkOutcome::Continue
}

fn greeting_refusal(sentence: &str) -> LinkOutcome {
    LinkOutcome::Refuse {
        code: "protocol_hello".to_string(),
        message: format!("{sentence}; start the panel of this browser host again"),
    }
}

/// Read one request of a link.
fn request_of(next: Next) -> Step {
    match next {
        Next::Line(bytes) => match horizon_browser::host_driver::decode_request(&bytes) {
            Ok(request) => Step::Request(request),
            Err(error) => Step::Refuse(LinkOutcome::refusal(error)),
        },
        Next::TooLong { size, limit } => Step::Refuse(LinkOutcome::Answer(horizon_browser::host_driver::line_refusal(
            size, limit,
        ))),
        Next::Ended => Step::Ended,
        Next::Nothing => Step::Silent,
    }
}

/// One pass of a link: the requests of this computer, the events of the page,
/// and one picture when the last one was acknowledged.
fn pass(session: &mut HostSession, link: &mut Link, gate: &mut FrameGate) -> LinkOutcome {
    let mut sent_page = session.page_message();
    loop {
        session.apply_events();
        let page = session.page_message();
        if page != sent_page {
            sent_page = page.clone();
            if link.send(&HostDriverReply::Page(page)).is_err() {
                return LinkOutcome::Closed;
            }
        }
        if let Some(reply) = session.next_frame(gate) {
            if link.send(&reply).is_err() {
                return LinkOutcome::Closed;
            }
            if let HostDriverReply::Frame(frame) = &reply {
                gate.sent(frame.seq);
            }
            // A refusal of one picture carries no acknowledgement, so the next
            // pass may send the next picture of the page.
        }
        while let Some(reply) = session.take_reply() {
            if link.send(&reply).is_err() {
                return LinkOutcome::Closed;
            }
        }
        if link.flush().is_err() {
            return LinkOutcome::Closed;
        }
        let next = link.next(PASS_WAIT);
        match request_of(next) {
            Step::Request(request) => match session.apply(request, gate) {
                LinkOutcome::Continue => {}
                // A page that is not ready for one request keeps the link: the
                // person reads the words and the next request can succeed.
                LinkOutcome::Note { code, message } => {
                    if link
                        .send(&HostDriverReply::Refusal {
                            code,
                            message,
                            fatal: false,
                        })
                        .is_err()
                    {
                        return LinkOutcome::Closed;
                    }
                }
                outcome => return outcome,
            },
            // A silent pass is the normal case of an idle page.
            Step::Silent => {}
            Step::Ended => return LinkOutcome::Closed,
            Step::Refuse(outcome) => return outcome,
        }
    }
}

/// Write the answer of a link that ends, and flush it before the link closes.
fn finish(link: &mut Link, outcome: LinkOutcome) -> LinkOutcome {
    match &outcome {
        LinkOutcome::Answer(reply) => {
            let _ = link.send(reply);
        }
        LinkOutcome::Refuse { code, message } => {
            let _ = link.send(&HostDriverReply::Refusal {
                code: code.clone(),
                message: message.clone(),
                fatal: true,
            });
        }
        LinkOutcome::Close => {
            let _ = link.send(&HostDriverReply::Closed);
        }
        LinkOutcome::Continue | LinkOutcome::Closed | LinkOutcome::Note { .. } => {}
    }
    let _ = link.flush();
    outcome
}

#[cfg(test)]
mod tests {
    use super::HostPage;
    use horizon_browser::{BrowserCommand, BrowserEvent};

    #[test]
    fn the_engine_of_a_browser_host_names_its_backend() {
        assert_eq!(super::HostDriverEngine::Firefox.as_str(), "firefox");
        assert_eq!(
            super::HostDriverEngine::Firefox.backend(),
            horizon_browser::BackendKind::FirefoxBidi
        );
    }

    #[test]
    fn a_page_that_a_test_drives_is_the_shape_of_a_page_of_this_machine() {
        // The trait is the seam of the tests of the link: a page that records
        // what the wire asked for, and reports what a page reports.
        struct Recording {
            commands: std::sync::Mutex<Vec<BrowserCommand>>,
        }
        impl HostPage for Recording {
            fn send(&self, command: BrowserCommand) -> bool {
                self.commands
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(command);
                true
            }
            fn next_event(&self) -> Option<BrowserEvent> {
                None
            }
            fn shutdown(&mut self) {}
        }
        let page = Recording {
            commands: std::sync::Mutex::new(Vec::new()),
        };
        assert!(page.send(BrowserCommand::Reload));
        assert_eq!(page.commands.lock().map_or(0, |commands| commands.len()), 1);
    }
}
