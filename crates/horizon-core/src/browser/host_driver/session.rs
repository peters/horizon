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

use std::io::{BufReader, Write as _};
use std::sync::Arc;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use horizon_browser::host_driver::{
    FrameGate, HostDriverFrame, HostDriverFrameFormat, HostDriverPage, HostDriverPageState, HostDriverReply,
    HostDriverRequest, HostDriverWelcome, HostSystem, MAX_HOST_DRIVER_LINE_BYTES, MAX_HOST_FRAME_SIDE,
    read_line as read_wire_line,
};
use horizon_browser::{
    BrowserCommand, BrowserConfig, BrowserEvent, BrowserSession, BrowserSessionConfig, FrameData, FrameSlot,
    VideoCaptureHandle, start_session,
};

use super::coordination::WireCoordination;
use super::{HostDriverEngine, HostDriverOptions, Link, LinkEnd, LinkOutcome, Next, Step, paths};

/// How long one pass of the loop waits for a request before it looks at the
/// page and the frame again.
pub(super) const PASS_WAIT: Duration = Duration::from_millis(20);

/// Most events of the page that one pass applies, so a page that floods its
/// channel cannot starve the link.
const EVENT_BURST: usize = 64;

/// Bound of the stop of one session of a browser host.
const STOP_TIMEOUT: Duration = Duration::from_secs(5);

/// How long a link has to greet the session. The greeting is the first line that
/// this computer writes on a link, so a link that says nothing for this long is
/// not a panel, and the next connection may take the session over.
const GREET_WAIT: Duration = Duration::from_secs(5);

/// The connections that arrive on the socket of the session while one link
/// serves it, and the silences that decide what happens to them.
pub(super) struct NewLinks<'a> {
    pub(super) listener: &'a std::os::unix::net::UnixListener,
    /// The silence of the serving link that a new connection takes the page over
    /// from.
    pub(super) take_over: Duration,
    /// The silence after which the session closes the serving link.
    pub(super) silent: Duration,
    /// The profile of this session, for the words of a refusal.
    pub(super) profile: &'a str,
}

impl NewLinks<'_> {
    /// The connection that waits on the socket, when one arrived, and otherwise
    /// nothing.
    fn waiting(&self) -> Option<std::os::unix::net::UnixStream> {
        match self.listener.accept() {
            // A socket that `accept` gives takes the mode of its listener on this
            // system, and the listener polls, so the link is put back into the
            // blocking mode that its two ends read and write with.
            Ok((stream, _)) => {
                let _ = stream.set_nonblocking(false);
                Some(stream)
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => None,
            Err(error) => {
                tracing::warn!(%error, "the socket of a browser host session could not accept a link");
                None
            }
        }
    }
}

/// Tell a new connection why the page of its profile is not free, and close it.
///
/// A second panel of one profile is a second page on one browser, and this
/// session shows one page. The person reads what to close instead of a link that
/// says nothing.
///
/// The greeting of the new link is read first, in a thread of its own: a link
/// that this session closes with its answer still unread answers the write of
/// the greeting with a reset, and this computer would then read no words at all.
fn refuse_busy_link(stream: std::os::unix::net::UnixStream, profile: &str) {
    let reply = HostDriverReply::Refusal {
        code: "link_busy".to_string(),
        message: format!(
            "the page of the profile {profile} of this browser host is attached to another link: close the other panel of this profile, then use Retry"
        ),
        fatal: true,
    };
    std::thread::Builder::new()
        .name("browser-host-refusal".to_string())
        .spawn(move || {
            let _ = stream.set_read_timeout(Some(GREET_WAIT));
            if let Ok(reader) = stream.try_clone() {
                let mut reader = BufReader::new(reader);
                let _ = read_wire_line(&mut reader, MAX_HOST_DRIVER_LINE_BYTES);
            }
            if let Ok(line) = horizon_browser::host_driver::encode_line(&reply) {
                let mut writer = &stream;
                let _ = writer.write_all(&line);
                let _ = writer.flush();
            }
            let _ = stream.shutdown(std::net::Shutdown::Both);
        })
        .ok();
}

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

/// A page that accepts every command of a link and reports nothing.
///
/// It is the page of a session of a test: the panel of a browser host is driven
/// through a real child of this binary and the session of a test beside it, so
/// the rules of a link are tested on a machine that has no Firefox.
pub(super) struct IdlePage;

impl HostPage for IdlePage {
    fn send(&self, _command: BrowserCommand) -> bool {
        true
    }

    fn next_event(&self) -> Option<BrowserEvent> {
        None
    }

    fn shutdown(&mut self) {}
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
        // The picture of this session is new to this link: a panel that attaches
        // to a session that nobody looked at has no frame of its own, so the
        // newest one travels again, as the key frame of that link.
        self.last_sent = None;
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
///
/// `new_links` is the socket of the session, so a link that arrives while this
/// one serves the page can take it over; a session of a test serves a link with
/// no socket and answers nothing to a new connection.
pub(super) fn serve_link(
    session: &mut HostSession,
    link: &mut Link,
    profile: &str,
    engine: HostDriverEngine,
    new_links: Option<&NewLinks<'_>>,
) -> LinkEnd {
    let mut gate = FrameGate::new();
    match greet(session, link, profile, engine, new_links) {
        LinkEnd {
            outcome: LinkOutcome::Continue,
            ..
        } => {}
        end => return finish(link, end),
    }
    let end = pass(session, link, &mut gate, new_links);
    finish(link, end)
}

/// Greet this computer: the version of the wire, the system of the host, and the
/// page that this link came back to.
fn greet(
    session: &mut HostSession,
    link: &mut Link,
    profile: &str,
    engine: HostDriverEngine,
    new_links: Option<&NewLinks<'_>>,
) -> LinkEnd {
    let greeted_at = Instant::now();
    let request = loop {
        // A link that arrives while this one has not greeted yet is the link of
        // a panel: it takes the session over, and this one ends with no words.
        if let Some(next) = new_links.and_then(NewLinks::waiting) {
            tracing::info!(
                profile,
                "a new link of a browser host takes the session over from a link that did not greet it"
            );
            return LinkEnd::taken_over(next);
        }
        match request_of(link.next(PASS_WAIT)) {
            Step::Request(request) => break request,
            // A link that says nothing for this long is not a panel of this
            // host.
            Step::Silent if greeted_at.elapsed() >= GREET_WAIT => return LinkEnd::of(LinkOutcome::Closed),
            Step::Silent => {}
            Step::Ended => return LinkEnd::of(LinkOutcome::Closed),
            Step::Refuse(outcome) => return LinkEnd::of(outcome),
        }
    };
    let HostDriverRequest::Hello {
        version,
        system,
        quality,
        ..
    } = request
    else {
        return LinkEnd::of(greeting_refusal(
            "the first message of a link of a browser host is its greeting",
        ));
    };
    if version != horizon_browser::host_driver::HOST_DRIVER_VERSION {
        return LinkEnd::of(LinkOutcome::Answer(horizon_browser::host_driver::version_refusal(
            version,
        )));
    }
    if let Some(words) = system.copy_refusal(&HostSystem::here()) {
        return LinkEnd::of(greeting_refusal(&words));
    }
    session.attach(horizon_browser::host_driver::clamp_quality(quality));
    if link
        .send(&HostDriverReply::Welcome(session.welcome(profile, engine)))
        .is_err()
    {
        return LinkEnd::of(LinkOutcome::Closed);
    }
    // The state of the page follows the welcome at once, so this computer has
    // an address for the picture that comes next, and a request that follows
    // the welcome is read in the pass below.
    if link.send(&HostDriverReply::Page(session.page_message())).is_err() {
        return LinkEnd::of(LinkOutcome::Closed);
    }
    let _ = link.flush();
    LinkEnd::of(LinkOutcome::Continue)
}

fn greeting_refusal(sentence: &str) -> LinkOutcome {
    LinkOutcome::Refuse {
        code: "protocol_hello".to_string(),
        message: format!("{sentence}; start the panel of this browser host again"),
    }
}

/// The name of one request, for the trace of a link.
fn request_name(request: &HostDriverRequest) -> &'static str {
    match request {
        HostDriverRequest::Hello { .. } => "hello",
        HostDriverRequest::Input { .. } => "input",
        HostDriverRequest::Navigate { .. } => "navigate",
        HostDriverRequest::Reload => "reload",
        HostDriverRequest::Back => "back",
        HostDriverRequest::Forward => "forward",
        HostDriverRequest::Viewport { .. } => "viewport",
        HostDriverRequest::FrameAck { .. } => "frame_ack",
        HostDriverRequest::Semantic { .. } => "semantic",
        HostDriverRequest::Signals { .. } => "signals",
        HostDriverRequest::Close => "close",
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
fn pass(session: &mut HostSession, link: &mut Link, gate: &mut FrameGate, new_links: Option<&NewLinks<'_>>) -> LinkEnd {
    let mut sent_page = session.page_message();
    // When this link last said something. Two rules of the session read it: a
    // link that is silent belongs to nobody, and a new link may take the page
    // over from it.
    let mut last_line = Instant::now();
    loop {
        // A connection that waits on the socket of the session: the page comes
        // back to a link whose link before it is gone, and a second panel of a
        // profile that is in use reads why it waits.
        if let Some(next) = new_links.and_then(NewLinks::waiting) {
            let silent = last_line.elapsed();
            if let Some(links) = new_links
                && silent >= links.take_over
            {
                tracing::info!(
                    profile = links.profile,
                    silent = ?silent,
                    "a new link of a browser host takes the page over from a link that went silent"
                );
                return LinkEnd::taken_over(next);
            }
            if let Some(links) = new_links {
                refuse_busy_link(next, links.profile);
            }
        }
        session.apply_events();
        let page = session.page_message();
        if page != sent_page {
            sent_page = page.clone();
            if link.send(&HostDriverReply::Page(page)).is_err() {
                return LinkEnd::of(LinkOutcome::Closed);
            }
        }
        if let Some(reply) = session.next_frame(gate) {
            if link.send(&reply).is_err() {
                return LinkEnd::of(LinkOutcome::Closed);
            }
            if let HostDriverReply::Frame(frame) = &reply {
                gate.sent(frame.seq);
            }
            // A refusal of one picture carries no acknowledgement, so the next
            // pass may send the next picture of the page.
        }
        while let Some(reply) = session.take_reply() {
            if link.send(&reply).is_err() {
                return LinkEnd::of(LinkOutcome::Closed);
            }
        }
        if link.flush().is_err() {
            return LinkEnd::of(LinkOutcome::Closed);
        }
        // A link whose other end vanished keeps its socket open, because the
        // other side of `ssh` holds it, so the end of this link is its silence.
        if let Some(links) = new_links
            && last_line.elapsed() >= links.silent
        {
            tracing::info!(
                profile = links.profile,
                silent = ?last_line.elapsed(),
                "a link of a browser host said nothing for the bound of the session, so the session closes it"
            );
            return LinkEnd::of(LinkOutcome::Silent);
        }
        let next = link.next(PASS_WAIT);
        if matches!(next, Next::Line(_)) {
            last_line = Instant::now();
        }
        match request_of(next) {
            Step::Request(request) => {
                // Every request but the two that a link repeats (its signals
                // and the acknowledgement of a picture) reaches the trace, so a
                // person reads what a link of a panel asked for.
                let name = request_name(&request);
                let quiet = matches!(name, "signals" | "frame_ack");
                let outcome = session.apply(request, gate);
                if !quiet && outcome == LinkOutcome::Continue {
                    tracing::debug!(request = name, "a link of a browser host applied a request");
                }
                match outcome {
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
                            return LinkEnd::of(LinkOutcome::Closed);
                        }
                    }
                    outcome => return LinkEnd::of(outcome),
                }
            }
            // A silent pass is the normal case of an idle page.
            Step::Silent => {}
            Step::Ended => return LinkEnd::of(LinkOutcome::Closed),
            Step::Refuse(outcome) => return LinkEnd::of(outcome),
        }
    }
}

/// Write the answer of a link that ends, and flush it before the link closes.
fn finish(link: &mut Link, end: LinkEnd) -> LinkEnd {
    match &end.outcome {
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
        LinkOutcome::Continue
        | LinkOutcome::Closed
        | LinkOutcome::TakeOver
        | LinkOutcome::Silent
        | LinkOutcome::Note { .. } => {}
    }
    let _ = link.flush();
    end
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
