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

use super::hub::{MAX_LIVE_LINKS, REFRESH_EVERY};
use super::outbox::{CEILING_BYTES, Next, Outbox, Push, RESUME_BYTES, Tag};
use super::protocol::{
    DEVICE_CHARS, ID_CHARS, LABEL_CHARS, LiveBytes, LiveComputer, LiveEndReason, LiveEvent, LivePanel, LiveRequest,
    LiveState, LiveWatcher, MAX_LIVE_FOLLOWS, MAX_LIVE_PANELS, MAX_LIVE_WATCHERS, clean, clean_panels,
};
use crate::frame::MAX_FRAME_BYTES;
use crate::protocol::{Frame, LinkErrorCode};
use crate::server::{LinkRegistry, LinkTicket, NoTransport, Outgoing};

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

fn follow(panel: &str) -> LiveRequest {
    LiveRequest::Follow {
        owner: text(OWNER),
        panel: text(panel),
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
    });
    let json = serde_json::to_string(&frame).expect("a frame is JSON");
    assert_eq!(json, r#"{"live":{"output":{"panel":"p1","data":"G1sxbWhp/w=="}}}"#);
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
    assert_eq!((first.cols, first.rows), (1, 1000));
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
    };
    let send = |count| {
        let (panel, data) = (text("p1"), bytes(count));
        owner.ask(&registry, LiveRequest::Output { panel, data });
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

    // The request does not stop the relay: the follower still takes the output
    // that comes before the new screen.
    let (name, data) = (text("p1"), bytes(3));
    let screen = LiveRequest::Screen {
        panel: name.clone(),
        cols: 80,
        rows: 24,
    };
    owner.ask(&registry, screen);
    owner.ask(&registry, LiveRequest::Output { panel: name, data });
    follower.ask(&registry, refresh());
    assert_eq!(follower.take(&registry).len(), 2, "a screen and its output");
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
