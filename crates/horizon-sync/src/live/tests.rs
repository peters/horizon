//! The frames, the outbox and the hub of the live links, without a socket: a
//! test takes the frames of an outbox as the thread of a link does, so every
//! order here is exact.

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

use super::hub::{MAX_LIVE_LINKS, REFRESH_EVERY, RESIZE_EVERY};
use super::outbox::{CEILING_BYTES, Next, Outbox, Push, RESUME_BYTES, Tag};
use super::protocol::{
    DEVICE_CHARS, ID_CHARS, LABEL_CHARS, LiveBytes, LiveComputer, LiveEndReason, LiveEvent, LivePanel, LiveRequest,
    LiveState, LiveWatcher, MAX_LIVE_FOLLOWS, MAX_LIVE_PANELS, MAX_LIVE_WATCHERS, clean, clean_panels,
};
use crate::frame::MAX_FRAME_BYTES;
use crate::protocol::{Frame, LinkErrorCode};
use crate::server::{LinkRegistry, LinkTicket, NoTransport, Outgoing};

mod sign;

const OWNER: &str = "laptop-a";

fn text(text: &str) -> String {
    text.to_owned()
}

fn bytes(count: usize) -> LiveBytes {
    LiveBytes(vec![b'x'; count].into())
}

fn panel(id: &str, typing: bool) -> LivePanel {
    LivePanel {
        id: text(id),
        title: text("build"),
        kind: text("shell"),
        state: String::new(),
        workspace: text("main"),
        cols: 80,
        rows: 24,
        typing,
    }
}

fn output(panel: &str, count: usize) -> (LiveEvent, Tag) {
    let event = LiveEvent::Output {
        owner: text(OWNER),
        panel: text(panel),
        data: bytes(count),
        replay: false,
    };
    (event, Tag::Stream(text(OWNER), text(panel)))
}

fn ended(panel: &str, reason: LiveEndReason) -> LiveEvent {
    LiveEvent::Ended {
        owner: text(OWNER),
        panel: text(panel),
        reason,
    }
}

fn typed(count: usize) -> LiveEvent {
    LiveEvent::Typed {
        panel: text("p1"),
        from: text("laptop-b"),
        label: text("laptop-b"),
        data: bytes(count),
    }
}

/// A screen of one panel of the owner with its grid and its two facts.
fn send_screen(panel: &str, fresh: bool, everyone: bool) -> LiveRequest {
    LiveRequest::Screen {
        panel: text(panel),
        cols: 80,
        rows: 24,
        fresh,
        everyone,
    }
}

/// Output of the owner: the replay that draws a screen, or the live output.
fn send_output(panel: &str, words: &str, replay: bool) -> LiveRequest {
    LiveRequest::Output {
        panel: text(panel),
        data: LiveBytes(words.as_bytes().into()),
        replay,
    }
}

/// The frames of one follower as one word each, so a test reads the order of a
/// screen, its replay and the live output.
fn stream_of(events: &[LiveEvent]) -> Vec<String> {
    let word = |event: &LiveEvent| match event {
        LiveEvent::Screen { fresh, everyone, .. } => format!("screen fresh={fresh} everyone={everyone}"),
        LiveEvent::Output { data, replay, .. } => {
            let kind = if *replay { "replay" } else { "output" };
            format!("{kind} {}", String::from_utf8_lossy(&data.0))
        }
        other => format!("{other:?}"),
    };
    events.iter().map(word).collect()
}

fn follow(panel: &str) -> LiveRequest {
    LiveRequest::Follow {
        owner: text(OWNER),
        panel: text(panel),
    }
}

/// The grid that a follower gives the program of a panel that it follows.
fn resize(cols: u16, rows: u16) -> LiveRequest {
    LiveRequest::Resize {
        owner: text(OWNER),
        panel: text("p1"),
        cols,
        rows,
    }
}

fn refresh() -> LiveRequest {
    LiveRequest::Refresh {
        owner: text(OWNER),
        panel: text("p1"),
    }
}

#[test]
fn terminal_bytes_travel_as_base64_and_never_print() {
    let frame = Frame::Live(LiveRequest::Output {
        panel: text("p1"),
        data: LiveBytes(b"\x1b[1mhi\xff"[..].into()),
        replay: false,
    });
    let json = serde_json::to_string(&frame).expect("a frame is JSON");
    assert_eq!(
        json,
        r#"{"live":{"output":{"panel":"p1","data":"G1sxbWhp/w==","replay":false}}}"#
    );
    assert_eq!(serde_json::from_str::<Frame>(&json).expect("it reads back"), frame);
    let printed = format!("{frame:?}");
    assert!(printed.contains("7 bytes") && !printed.contains("hi"), "{printed}");
    assert!(
        serde_json::from_str::<Frame>(r#"{"live":{"output":{"panel":"p1","data":"not base64!"}}}"#).is_err(),
        "bytes that are not base64 are not one frame"
    );
}

#[test]
fn every_live_frame_round_trips_through_its_json() {
    let computers = vec![LiveComputer {
        device: text(OWNER),
        label: text("Laptop A"),
        panels: vec![panel("p1", true)],
        own: false,
    }];
    let (owner, name) = (text(OWNER), text("p1"));
    let frames = vec![
        Frame::Live(LiveRequest::Open {
            label: text("Laptop A"),
        }),
        Frame::Live(LiveRequest::Panels {
            panels: vec![panel("p1", true)],
        }),
        Frame::Live(follow("p1")),
        Frame::Live(LiveRequest::Unfollow {
            owner: owner.clone(),
            panel: name.clone(),
        }),
        Frame::Live(refresh()),
        Frame::Live(LiveRequest::Screen {
            panel: name.clone(),
            cols: 80,
            rows: 24,
            fresh: true,
            everyone: false,
        }),
        Frame::Live(LiveRequest::Ended { panel: name.clone() }),
        Frame::Live(LiveRequest::Typed {
            owner: owner.clone(),
            panel: name.clone(),
            data: bytes(3),
        }),
        Frame::LiveEvent(LiveEvent::Opened {
            device: owner.clone(),
            computers: computers.clone(),
        }),
        Frame::LiveEvent(LiveEvent::Computers { computers }),
        Frame::LiveEvent(LiveEvent::Watchers {
            panel: name.clone(),
            watchers: vec![LiveWatcher {
                device: text("laptop-b"),
                label: text("Laptop B"),
            }],
            refresh: true,
        }),
        Frame::LiveEvent(LiveEvent::Screen {
            owner,
            panel: name,
            cols: 80,
            rows: 24,
            fresh: true,
            everyone: false,
        }),
        Frame::LiveEvent(output("p1", 3).0),
        Frame::LiveEvent(typed(3)),
    ];
    for frame in frames {
        let json = serde_json::to_string(&frame).expect("a frame is JSON");
        assert_eq!(
            serde_json::from_str::<Frame>(&json).expect("it reads back"),
            frame,
            "{json}"
        );
    }
    assert_eq!(
        serde_json::to_string(&Frame::LiveEvent(ended("p1", LiveEndReason::NotFound))).expect("a frame is JSON"),
        r#"{"live_event":{"ended":{"owner":"laptop-a","panel":"p1","reason":"not_found"}}}"#
    );
}

#[test]
fn the_state_of_a_link_and_the_mark_of_this_computer_do_not_travel() {
    assert!(serde_json::to_string(&LiveEvent::State(LiveState::Open)).is_err());
    assert!(serde_json::from_str::<Frame>(r#"{"live_event":{"state":"open"}}"#).is_err());
    let marked = LiveComputer {
        device: text(OWNER),
        label: String::new(),
        panels: Vec::new(),
        own: true,
    };
    let json = serde_json::to_string(&marked).expect("a computer is JSON");
    assert_eq!(json, r#"{"device":"laptop-a","label":"","panels":[]}"#);
    assert!(
        serde_json::from_str::<LiveComputer>(r#"{"device":"laptop-a","label":"","panels":[],"own":true}"#).is_err(),
        "a server cannot mark a computer as the one that reads"
    );
}

#[test]
fn a_text_and_a_list_are_cut_at_their_bounds() {
    assert_eq!(clean("a\x1b[31mb\n\u{9b}c\u{7f}", 60), "a[31mbc");
    assert_eq!(clean(&"é".repeat(70), LABEL_CHARS), "é".repeat(60));
    assert_eq!(clean(&"x".repeat(50), DEVICE_CHARS).len(), 40);

    let long = LivePanel {
        id: "i".repeat(70),
        title: "t".repeat(130),
        kind: "k".repeat(30),
        state: "s".repeat(30),
        workspace: "w".repeat(90),
        cols: 0,
        rows: 5000,
        typing: false,
    };
    let cleaned = clean_panels(vec![
        long,
        panel("p1", true),
        panel("p1", false),
        panel("\n", true),
        panel("p2", true),
    ]);
    let first = &cleaned[0];
    let lengths = [&first.id, &first.title, &first.kind, &first.state, &first.workspace].map(String::len);
    assert_eq!(lengths, [ID_CHARS, 120, 24, 24, 80]);
    // One pair of bounds for the server and a follower: `bounded_screen_grid`.
    assert_eq!((first.cols, first.rows), (2, 500));
    let ids: Vec<&str> = cleaned.iter().skip(1).map(|panel| panel.id.as_str()).collect();
    assert_eq!(
        ids,
        ["p1", "p2"],
        "a second panel of one id and a panel without an id leave"
    );
    assert!(cleaned[1].typing, "the first panel of an id stays");

    let many: Vec<LivePanel> = (0..70).map(|index| panel(&format!("p{index}"), true)).collect();
    assert_eq!(clean_panels(many).len(), MAX_LIVE_PANELS);
}

#[test]
fn output_over_the_ceiling_drops_the_waiting_output_of_its_panel_alone() {
    let outbox = Outbox::default();
    let (event, tag) = output("p2", 1000);
    assert_eq!(outbox.push(event, tag), Push::Queued);
    assert_eq!(outbox.push(ended("p9", LiveEndReason::Closed), Tag::Kept), Push::Queued);
    let other = outbox.waiting().0;
    let mut queued = 0;
    loop {
        let (event, tag) = output("p1", 48 * 1024);
        if outbox.push(event, tag) == Push::Full {
            break;
        }
        queued += 1;
        assert!(outbox.waiting().0 <= CEILING_BYTES, "the ceiling holds");
        assert!(queued < 100, "the ceiling stops the output");
    }
    assert!(queued >= 14, "about one mebibyte of output waited: {queued}");
    assert_eq!(
        outbox.waiting().0,
        other,
        "the output of the panel that fell behind left"
    );

    // The link is under the resume bound, so its thread reports that first, and
    // then the frames that stayed, in their order.
    assert!(matches!(outbox.next(), Next::Resume));
    assert!(matches!(outbox.next(), Next::Event(LiveEvent::Output { panel, .. }) if panel == "p2"));
    outbox.written();
    assert!(matches!(outbox.next(), Next::Event(LiveEvent::Ended { .. })));
    outbox.written();
    assert_eq!(outbox.waiting(), (0, true));
}

#[test]
fn a_kept_frame_is_never_dropped_and_kept_frames_alone_end_the_link() {
    let outbox = Outbox::default();
    while outbox.waiting().0 + output("p1", 48 * 1024).0.cost() <= CEILING_BYTES {
        let (event, tag) = output("p1", 48 * 1024);
        assert_eq!(outbox.push(event, tag), Push::Queued);
    }
    let (mut answer, mut kept) = (Push::Queued, 0);
    while answer == Push::Queued {
        answer = outbox.push(typed(16 * 1024), Tag::Kept);
        kept += 1;
        assert!(outbox.waiting().0 <= CEILING_BYTES, "the ceiling holds");
    }
    assert_eq!(
        answer,
        Push::Evicted,
        "the output of every panel leaves for a kept frame"
    );
    assert_eq!(
        outbox.waiting().0,
        kept * typed(16 * 1024).cost(),
        "every kept frame waits"
    );

    while outbox.push(typed(16 * 1024), Tag::Kept) != Push::Full {
        kept += 1;
        assert!(outbox.waiting().0 <= CEILING_BYTES, "the ceiling holds");
        assert!(kept < 100, "the ceiling stops what is typed");
    }
    assert!(kept >= 40, "about one mebibyte of typed bytes waited: {kept}");
    assert!(
        matches!(outbox.next(), Next::Closed),
        "a computer that does not read ends"
    );
    assert_eq!(
        outbox.push(typed(1), Tag::Kept),
        Push::Queued,
        "a closed outbox takes nothing"
    );
}

#[test]
fn the_newest_list_replaces_the_list_that_waits() {
    let outbox = Outbox::default();
    let list = |device: &str| LiveEvent::Computers {
        computers: vec![LiveComputer {
            device: text(device),
            label: String::new(),
            panels: Vec::new(),
            own: false,
        }],
    };
    for (panel, count) in [("p1", 100), ("p2", 100), ("p1", 100)] {
        let (event, tag) = output(panel, count);
        assert_eq!(outbox.push(event, tag), Push::Queued);
    }
    assert_eq!(outbox.push(list("laptop-a"), Tag::List), Push::Queued);
    assert_eq!(outbox.push(ended("p1", LiveEndReason::Closed), Tag::Kept), Push::Queued);
    let before = outbox.waiting().0;
    assert_eq!(outbox.push(list("laptop-b"), Tag::List), Push::Queued);
    assert_eq!(outbox.waiting().0, before, "one list waits");
    outbox.drop_stream(OWNER, "p1");

    assert!(matches!(outbox.next(), Next::Event(LiveEvent::Output { panel, .. }) if panel == "p2"));
    outbox.written();
    assert!(matches!(outbox.next(), Next::Event(LiveEvent::Ended { .. })));
    outbox.written();
    let Next::Event(LiveEvent::Computers { computers }) = outbox.next() else {
        panic!("the list comes last");
    };
    assert_eq!(computers[0].device, "laptop-b", "the newest list waits");
    outbox.close();
    assert!(matches!(outbox.next(), Next::Closed));
}

/// One computer on the hub of a test: its link, and the frames that wait for it.
struct Computer {
    ticket: Arc<LinkTicket>,
    outbox: Arc<Outbox>,
    /// What the writing thread of the link would write besides its outbox.
    frames: mpsc::Receiver<Outgoing>,
    closed: Arc<AtomicBool>,
}

fn computer(registry: &LinkRegistry, device: &str) -> Computer {
    let (frames, outgoing) = mpsc::channel();
    let closed = Arc::new(AtomicBool::new(false));
    let ticket = registry.add(device, frames, Arc::clone(&closed), Arc::new(NoTransport));
    let outbox = Arc::new(Outbox::default());
    assert!(registry.live.open(&ticket, device, &outbox), "{device} opens");
    Computer {
        ticket,
        outbox,
        frames: outgoing,
        closed,
    }
}

impl Computer {
    fn ask(&self, registry: &LinkRegistry, request: LiveRequest) {
        registry.live.request(&self.ticket.device, self.ticket.id, request);
    }

    /// Takes the frames that wait, as the thread of the link does.
    fn take(&self, registry: &LinkRegistry) -> Vec<LiveEvent> {
        let mut events = Vec::new();
        while !self.outbox.waiting().1 {
            match self.outbox.next() {
                Next::Event(event) => {
                    events.push(event);
                    self.outbox.written();
                }
                Next::Resume => registry.live.resume(&self.ticket.device, self.ticket.id),
                Next::Closed => break,
            }
        }
        events
    }
}

fn watchers(devices: &[&str], refresh: bool) -> LiveEvent {
    let watcher = |device: &&str| LiveWatcher {
        device: text(device),
        label: text(device),
    };
    LiveEvent::Watchers {
        panel: text("p1"),
        watchers: devices.iter().map(watcher).collect(),
        refresh,
    }
}

#[test]
fn a_follower_that_does_not_read_falls_behind_and_its_owner_is_asked_again_when_it_reads() {
    let registry = LinkRegistry::default();
    let owner = computer(&registry, OWNER);
    let follower = computer(&registry, "laptop-b");
    let panels = vec![panel("p1", true)];
    owner.ask(&registry, LiveRequest::Panels { panels });
    follower.ask(&registry, follow("p1"));
    assert_eq!(owner.take(&registry).last(), Some(&watchers(&["laptop-b"], true)));
    follower.take(&registry);

    // The thread of the follower holds the screen in a write that does not end,
    // and the owner sends four mebibytes of output.
    let screen = |cols| LiveRequest::Screen {
        panel: text("p1"),
        cols,
        rows: 24,
        fresh: false,
        everyone: false,
    };
    let send = |count| {
        let (panel, data) = (text("p1"), bytes(count));
        owner.ask(
            &registry,
            LiveRequest::Output {
                panel,
                data,
                replay: false,
            },
        );
    };
    owner.ask(&registry, screen(80));
    assert!(matches!(follower.outbox.next(), Next::Event(LiveEvent::Screen { .. })));
    let mut most = 0;
    for _ in 0..80 {
        send(48 * 1024);
        most = most.max(follower.outbox.waiting().0);
    }
    assert!(
        (CEILING_BYTES - 70_000..=CEILING_BYTES).contains(&most),
        "the ceiling held: {most}"
    );
    assert!(
        follower.outbox.waiting().0 < RESUME_BYTES,
        "the output that waited left"
    );
    assert_eq!(
        owner.take(&registry),
        Vec::new(),
        "the owner is not asked while the follower does not read"
    );

    // The write ends: the follower reads again, so the owner is asked for a
    // new screen, and the relay starts again at that screen and not before.
    follower.outbox.written();
    assert_eq!(follower.take(&registry), Vec::new());
    assert_eq!(owner.take(&registry), vec![watchers(&["laptop-b"], true)]);
    send(4);
    assert_eq!(
        follower.take(&registry),
        Vec::new(),
        "no output comes before the screen"
    );
    owner.ask(&registry, screen(100));
    send(5);
    let taken = follower.take(&registry);
    assert!(
        matches!(&taken[..], [LiveEvent::Screen { cols: 100, .. }, LiveEvent::Output { data, .. }] if data.0.len() == 5),
        "{taken:?}"
    );
}

/// One owner, two followers of one panel, and the frames that each one takes.
struct Two {
    registry: LinkRegistry,
    owner: Computer,
    first: Computer,
    second: Computer,
}

impl Two {
    /// An owner that shares `p1`, and two followers that took their first screen
    /// and are relaying.
    fn new() -> Self {
        let registry = LinkRegistry::default();
        let owner = computer(&registry, OWNER);
        let (first, second) = (computer(&registry, "laptop-b"), computer(&registry, "laptop-c"));
        for link in [&owner, &first, &second] {
            link.take(&registry);
        }
        owner.ask(
            &registry,
            LiveRequest::Panels {
                panels: vec![panel("p1", true)],
            },
        );
        for link in [&owner, &first, &second] {
            link.take(&registry);
        }
        let two = Self {
            registry,
            owner,
            first,
            second,
        };
        two.join(&two.first);
        two.join(&two.second);
        // The second screen of a join reaches the first follower as output alone.
        assert_eq!(
            stream_of(&two.first.take(&two.registry)),
            ["output one"],
            "the first follower skips the replay of the screen of the second"
        );
        two
    }

    /// One follower joins and takes its screen, the replay that draws it and the
    /// output after it.
    fn join(&self, follower: &Computer) {
        follower.ask(&self.registry, follow("p1"));
        self.owner.take(&self.registry);
        self.owner.ask(&self.registry, send_screen("p1", true, false));
        self.owner.ask(&self.registry, send_output("p1", "grid", true));
        self.owner.ask(&self.registry, send_output("p1", "one", false));
        assert_eq!(
            stream_of(&follower.take(&self.registry)),
            [
                "screen fresh=true everyone=false".to_owned(),
                "replay grid".to_owned(),
                "output one".to_owned()
            ]
        );
    }
}

#[test]
fn a_screen_for_one_follower_reaches_that_one_and_leaves_the_other_stream_whole() {
    let two = Two::new();
    let (first, second) = (&two.first, &two.second);

    // A third follower joins. Its screen, its replay and the output after it
    // reach it alone, and the stream of the two that relay goes on with the live
    // output: no frame of the join reaches them, and none of theirs is lost.
    let late = computer(&two.registry, "laptop-d");
    // The new link is a change of the list for every link, and the join is no
    // frame for the two followers that read on.
    late.take(&two.registry);
    for follower in [first, second] {
        follower.take(&two.registry);
    }
    late.ask(&two.registry, follow("p1"));
    two.owner.take(&two.registry);
    two.owner.ask(&two.registry, send_screen("p1", true, false));
    two.owner.ask(&two.registry, send_output("p1", "join-grid", true));
    two.owner.ask(&two.registry, send_output("p1", "two", false));
    assert_eq!(
        stream_of(&late.take(&two.registry)),
        [
            "screen fresh=true everyone=false".to_owned(),
            "replay join-grid".to_owned(),
            "output two".to_owned()
        ]
    );
    for follower in [first, second] {
        assert_eq!(
            stream_of(&follower.take(&two.registry)),
            ["output two"],
            "a follower that relays skips the whole replay and takes the live output"
        );
    }
}

#[test]
fn a_screen_for_everyone_reaches_the_followers_that_relay_and_the_one_that_waits() {
    let two = Two::new();
    let (first, second) = (&two.first, &two.second);

    // The second follower asks for a new screen, so it waits for one.
    second.ask(&two.registry, refresh());
    two.owner.take(&two.registry);
    two.owner.ask(&two.registry, send_output("p1", "half", false));
    assert_eq!(
        stream_of(&first.take(&two.registry)),
        ["output half"],
        "the follower that relays keeps its output"
    );
    assert_eq!(
        stream_of(&second.take(&two.registry)),
        Vec::<String>::new(),
        "the follower that waits takes none of it"
    );

    // A screen for everyone reaches each of them, with its replay.
    two.owner.ask(&two.registry, send_screen("p1", false, true));
    two.owner.ask(&two.registry, send_output("p1", "all-grid", true));
    for follower in [first, second] {
        assert_eq!(
            stream_of(&follower.take(&two.registry)),
            [
                "screen fresh=false everyone=true".to_owned(),
                "replay all-grid".to_owned()
            ]
        );
    }
}

#[test]
fn a_screen_that_one_follower_asked_for_reaches_that_one_alone() {
    let two = Two::new();
    let (first, second) = (&two.first, &two.second);

    // The first follower asks for a new screen. It waits for one, and the screen
    // that answers it reaches it alone.
    first.ask(&two.registry, refresh());
    two.owner.take(&two.registry);
    two.owner.ask(&two.registry, send_screen("p1", false, false));
    two.owner.ask(&two.registry, send_output("p1", "asked-grid", true));
    assert_eq!(
        stream_of(&first.take(&two.registry)),
        [
            "screen fresh=false everyone=false".to_owned(),
            "replay asked-grid".to_owned()
        ]
    );
    assert_eq!(
        stream_of(&second.take(&two.registry)),
        Vec::<String>::new(),
        "a screen for one follower does not reach the other"
    );

    // Now every follower relays, so a screen that is not for everyone reaches
    // nobody, and the live output after it reaches both.
    two.owner.ask(&two.registry, send_screen("p1", false, false));
    two.owner.ask(&two.registry, send_output("p1", "shared", false));
    for follower in [first, second] {
        assert_eq!(
            stream_of(&follower.take(&two.registry)),
            ["output shared"],
            "the replay of a screen reaches the follower that took that screen alone"
        );
    }
}

#[test]
fn a_follower_that_fell_behind_waits_for_the_screen_that_its_owner_sends() {
    let two = Two::new();
    let (first, behind) = (&two.first, &two.second);

    // Every frame over the ceiling drops what waits, and the follower that lost
    // its output waits for a new screen while the other one reads on.
    for _ in 0..40 {
        let (panel, data) = (text("p1"), bytes(48 * 1024));
        two.owner.ask(
            &two.registry,
            LiveRequest::Output {
                panel,
                data,
                replay: false,
            },
        );
        first.take(&two.registry);
    }
    assert!(
        behind.outbox.waiting().0 < CEILING_BYTES,
        "the ceiling dropped the output of the follower that did not read"
    );
    two.owner.ask(&two.registry, send_output("p1", "lost", false));
    assert_eq!(
        stream_of(&behind.take(&two.registry)),
        Vec::<String>::new(),
        "a follower that fell behind takes no output, and its next read asks for a screen"
    );
    assert_eq!(
        two.owner.take(&two.registry),
        vec![watchers(&["laptop-b", "laptop-c"], true)],
        "the owner is asked for a screen when the follower reads again"
    );

    // The screen that answers that request and the output after it reach the
    // follower that waited, and not the one that read on.
    two.owner.ask(&two.registry, send_screen("p1", false, false));
    two.owner.ask(&two.registry, send_output("p1", "back", false));
    assert_eq!(
        stream_of(&behind.take(&two.registry)),
        ["screen fresh=false everyone=false".to_owned(), "output back".to_owned()]
    );
    assert_eq!(
        stream_of(&first.take(&two.registry)),
        ["output lost", "output back"],
        "the follower that read on takes the live output and no screen"
    );
}

#[test]
fn a_follower_asks_the_owner_for_a_new_screen_at_most_once_a_second() {
    let registry = LinkRegistry::default();
    let owner = computer(&registry, OWNER);
    let follower = computer(&registry, "laptop-b");
    let stranger = computer(&registry, "laptop-c");
    let panels = vec![panel("p1", true)];
    owner.ask(&registry, LiveRequest::Panels { panels });
    follower.ask(&registry, follow("p1"));
    owner.take(&registry);
    follower.take(&registry);

    // A computer that does not follow the panel asks nothing. A follower asks
    // once: the second request and the follow that it has already come inside
    // the same second, and they are dropped.
    stranger.ask(&registry, refresh());
    assert_eq!(owner.take(&registry), Vec::new(), "a stranger asks nothing");
    follower.ask(&registry, refresh());
    follower.ask(&registry, refresh());
    follower.ask(&registry, follow("p1"));
    assert_eq!(owner.take(&registry), vec![watchers(&["laptop-b"], true)]);

    // The follower that asked waits for its screen, and takes that screen and
    // the output after it. The next request is still inside the second.
    let (name, data) = (text("p1"), bytes(3));
    owner.ask(&registry, send_screen("p1", false, false));
    owner.ask(
        &registry,
        LiveRequest::Output {
            panel: name,
            data,
            replay: false,
        },
    );
    assert_eq!(
        stream_of(&follower.take(&registry)),
        ["screen fresh=false everyone=false", "output xxx"],
        "the screen that the follower asked for, and the output after it"
    );
    assert_eq!(owner.take(&registry), Vec::new(), "still inside the second");

    // Each follower has a second of its own, and after it a follower asks again.
    stranger.ask(&registry, follow("p1"));
    let both = watchers(&["laptop-b", "laptop-c"], true);
    assert_eq!(owner.take(&registry), vec![both.clone()], "the set grew");
    stranger.ask(&registry, refresh());
    assert_eq!(
        owner.take(&registry),
        vec![both.clone()],
        "the first request of a follower"
    );
    std::thread::sleep(REFRESH_EVERY + Duration::from_millis(50));
    follower.ask(&registry, refresh());
    follower.ask(&registry, refresh());
    assert_eq!(owner.take(&registry), vec![both], "one request after the second passed");
    stranger.ask(
        &registry,
        LiveRequest::Unfollow {
            owner: text(OWNER),
            panel: text("p1"),
        },
    );
    owner.take(&registry);
    stranger.ask(&registry, refresh());
    assert_eq!(owner.take(&registry), Vec::new(), "a computer that left asks nothing");
}

#[test]
fn a_computer_that_does_not_read_what_is_typed_loses_its_live_link() {
    let registry = LinkRegistry::default();
    let owner = computer(&registry, OWNER);
    let follower = computer(&registry, "laptop-b");
    let panels = vec![panel("p1", true)];
    owner.ask(&registry, LiveRequest::Panels { panels });
    follower.ask(&registry, follow("p1"));
    for _ in 0..80 {
        let (of, panel, data) = (text(OWNER), text("p1"), bytes(16 * 1024));
        follower.ask(&registry, LiveRequest::Typed { owner: of, panel, data });
        assert!(owner.outbox.waiting().0 <= CEILING_BYTES, "the ceiling holds");
    }
    assert!(matches!(owner.outbox.next(), Next::Closed), "nothing more waits for it");
    let ending = owner
        .frames
        .recv_timeout(Duration::from_secs(20))
        .expect("the link ends with a frame");
    let Frame::Error(error) = &ending.frame else {
        panic!("the link ends with a typed error");
    };
    assert_eq!(error.code, LinkErrorCode::LiveLimit);
    drop(ending);
    let deadline = Instant::now() + Duration::from_secs(20);
    while !owner.closed.load(Ordering::SeqCst) {
        assert!(Instant::now() < deadline, "the link of the computer closes");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn a_follow_has_its_limits_and_a_refused_one_reads_why() {
    let registry = LinkRegistry::default();
    let owner = computer(&registry, OWNER);
    let panels: Vec<LivePanel> = (1..=9).map(|index| panel(&format!("p{index}"), false)).collect();
    owner.ask(&registry, LiveRequest::Panels { panels });
    let names = ["laptop-b", "laptop-c", "laptop-d", "laptop-e", "laptop-f"];
    let others: Vec<Computer> = names.iter().map(|name| computer(&registry, name)).collect();
    let refused = |computer: &Computer, request: LiveRequest| {
        computer.take(&registry);
        computer.ask(&registry, request);
        computer.take(&registry)
    };

    // Four computers follow one panel, and the fifth reads the limit.
    for other in &others[..MAX_LIVE_WATCHERS] {
        assert_eq!(refused(other, follow("p1")), Vec::new());
    }
    assert_eq!(
        owner.take(&registry).last(),
        Some(&watchers(&names[..MAX_LIVE_WATCHERS], true))
    );
    assert_eq!(
        refused(&others[4], follow("p1")),
        vec![ended("p1", LiveEndReason::Limit)]
    );

    // One computer follows eight panels, and the ninth reads the limit. A
    // follow that it has already asks the owner for a new screen.
    for index in 2..=MAX_LIVE_FOLLOWS {
        assert_eq!(refused(&others[0], follow(&format!("p{index}"))), Vec::new());
    }
    assert_eq!(
        refused(&others[0], follow("p9")),
        vec![ended("p9", LiveEndReason::Limit)]
    );
    owner.take(&registry);
    assert_eq!(refused(&others[0], follow("p1")), Vec::new());
    assert_eq!(
        owner.take(&registry),
        vec![watchers(&names[..MAX_LIVE_WATCHERS], true)],
        "the same follow again is no second follow"
    );

    // A panel of its own, a computer without a live link and a panel that is
    // not shared are not found, and the answer holds the names cut at their bounds.
    assert_eq!(
        refused(&owner, follow("p1")),
        vec![ended("p1", LiveEndReason::NotFound)]
    );
    assert_eq!(
        refused(&others[4], follow("nope")),
        vec![ended("nope", LiveEndReason::NotFound)]
    );
    let (long_owner, long_panel) = ("o".repeat(50), "p\n".repeat(70));
    let answer = refused(
        &others[4],
        LiveRequest::Follow {
            owner: long_owner,
            panel: long_panel,
        },
    );
    let [LiveEvent::Ended { owner, panel, reason }] = &answer[..] else {
        panic!("one answer: {answer:?}");
    };
    assert_eq!(
        (owner.len(), panel.as_str(), *reason),
        (40, &*"p".repeat(64), LiveEndReason::NotFound)
    );
}

#[test]
fn the_list_of_the_computers_stays_one_frame_and_the_live_links_have_a_limit() {
    let registry = LinkRegistry::default();
    let wide = |index: usize| LivePanel {
        id: format!("{index:064}"),
        title: "\u{1f600}".repeat(120),
        kind: "\u{1f600}".repeat(24),
        state: "\u{1f600}".repeat(24),
        workspace: "\u{1f600}".repeat(80),
        ..panel("p", true)
    };
    let panels: Vec<LivePanel> = (0..MAX_LIVE_PANELS).map(wide).collect();
    let first = computer(&registry, "computer-0");
    first.ask(&registry, LiveRequest::Panels { panels: panels.clone() });
    let alone = first.take(&registry);
    assert!(
        matches!(alone.last(), Some(LiveEvent::Computers { computers }) if computers[0].panels.len() == MAX_LIVE_PANELS),
        "a computer that is alone lists every panel"
    );

    let mut computers = vec![first];
    for index in 1..MAX_LIVE_LINKS {
        let next = computer(&registry, &format!("computer-{index}"));
        next.ask(&registry, LiveRequest::Panels { panels: panels.clone() });
        computers.push(next);
    }
    for computer in &computers {
        for event in computer.take(&registry) {
            let frame = serde_json::to_vec(&Frame::LiveEvent(event)).expect("an event is JSON");
            assert!(frame.len() <= MAX_FRAME_BYTES, "a list is one frame: {}", frame.len());
        }
    }
    let last = &computers[MAX_LIVE_LINKS - 1];
    last.ask(&registry, LiveRequest::Panels { panels: Vec::new() });
    let Some(LiveEvent::Computers { computers: listed }) = last.take(&registry).pop() else {
        panic!("the list after a change");
    };
    assert_eq!(listed.len(), MAX_LIVE_LINKS);
    assert!(
        listed[0].panels.len() < MAX_LIVE_PANELS && !listed[0].panels.is_empty(),
        "each computer has a part of the list: {}",
        listed[0].panels.len()
    );

    let (frames, _outgoing) = mpsc::channel();
    let closed = Arc::new(AtomicBool::new(false));
    let ticket = registry.add("computer-32", frames, closed, Arc::new(NoTransport));
    assert!(
        !registry
            .live
            .open(&ticket, "one too many", &Arc::new(Outbox::default())),
        "the server holds {MAX_LIVE_LINKS} live links"
    );
}

/// The grid that a follower gives the program of a panel reaches the owner only
/// from a computer that follows the panel now, only while the owner lets another
/// computer type in it, and no sooner than twice a second. The grid is cut at the
/// one pair of bounds of a screen.
/// Mutation: drop the `follows` check (the stranger's grid arrives), drop the
/// `typing` check (the grid of a panel that only looks arrives), drop the hold of
/// `resized` (both requests arrive), or send `cols`/`rows` as they came.
#[test]
fn a_grid_reaches_the_owner_from_a_follower_of_a_panel_that_lets_it_type_twice_a_second() {
    let registry = LinkRegistry::default();
    let owner = computer(&registry, OWNER);
    let follower = computer(&registry, "laptop-b");
    let stranger = computer(&registry, "laptop-c");
    owner.ask(
        &registry,
        LiveRequest::Panels {
            panels: vec![panel("p1", true), panel("p2", false)],
        },
    );
    follower.ask(&registry, follow("p1"));
    owner.take(&registry);
    follower.take(&registry);
    let grid = |cols, rows| {
        vec![LiveEvent::Resized {
            panel: text("p1"),
            from: text("laptop-b"),
            label: text("laptop-b"),
            cols,
            rows,
        }]
    };

    // A computer that does not follow the panel sends nothing to its owner.
    stranger.ask(&registry, resize(100, 30));
    assert_eq!(owner.take(&registry), Vec::new(), "a stranger sends no grid");

    // A follower sends one grid, and the second request inside the half second
    // is dropped, also when it names another grid.
    follower.ask(&registry, resize(100, 30));
    follower.ask(&registry, resize(101, 31));
    assert_eq!(owner.take(&registry), grid(100, 30), "one grid a half second");
    std::thread::sleep(RESIZE_EVERY + Duration::from_millis(50));
    follower.ask(&registry, resize(120, 40));
    assert_eq!(owner.take(&registry), grid(120, 40), "the next grid after the wait");

    // The bounds of a screen hold for a grid from another computer.
    std::thread::sleep(RESIZE_EVERY + Duration::from_millis(50));
    follower.ask(&registry, resize(0, 5000));
    assert_eq!(owner.take(&registry), grid(2, 500), "a grid over its bounds is cut");

    // The panel of the grid is shared, and the owner lets nobody type in it: the
    // relay holds the grid back, because a grid changes what the program does.
    let follows = |link: &Computer| link.ask(&registry, follow("p2"));
    follows(&follower);
    owner.take(&registry);
    follower.take(&registry);
    std::thread::sleep(RESIZE_EVERY + Duration::from_millis(50));
    follower.ask(
        &registry,
        LiveRequest::Resize {
            owner: text(OWNER),
            panel: text("p2"),
            cols: 100,
            rows: 30,
        },
    );
    assert_eq!(owner.take(&registry), Vec::new(), "typing is off in that panel");

    // A computer that left asks for no grid any more.
    follower.ask(
        &registry,
        LiveRequest::Unfollow {
            owner: text(OWNER),
            panel: text("p1"),
        },
    );
    owner.take(&registry);
    std::thread::sleep(RESIZE_EVERY + Duration::from_millis(50));
    follower.ask(&registry, resize(140, 50));
    assert_eq!(owner.take(&registry), Vec::new(), "a computer that left sends no grid");
}
