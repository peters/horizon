//! The sign of life of a live link, as the server keeps it.

use std::sync::{Arc, atomic::Ordering};
use std::time::{Duration, Instant};

use super::super::alive::{Alive, Limits};
use super::super::protocol::LiveEvent;
use super::super::relay::sign_of_life;
use super::{Computer, computer};
use crate::protocol::{Frame, LinkErrorCode};
use crate::server::LinkRegistry;

/// One computer of the hub whose live link has a sign of life, with that sign.
pub(super) fn signed(
    registry: &LinkRegistry,
    device: &str,
    limits: Limits,
) -> (Computer, Arc<Alive>, std::thread::JoinHandle<()>) {
    let computer = computer(registry, device);
    let alive = Arc::new(Alive::new(limits));
    let sign = {
        let (alive, outbox, ticket) = (
            Arc::clone(&alive),
            Arc::clone(&computer.outbox),
            Arc::clone(&computer.ticket),
        );
        std::thread::spawn(move || sign_of_life(&alive, &outbox, &ticket))
    };
    (computer, alive, sign)
}

/// The sign of life of one link: a heartbeat while it has nothing else to say,
/// and the end of a link whose computer sent nothing for the bound.
#[test]
fn a_live_link_that_reads_nothing_ends_and_an_idle_one_sends_a_heartbeat() {
    let registry = LinkRegistry::default();
    let quick = |silence: u64| Limits {
        tick: Duration::from_millis(5),
        heartbeat: Duration::from_millis(20),
        silence: Duration::from_millis(silence),
    };
    let (quiet, _, sign) = signed(&registry, "laptop-a", quick(120));
    // The link says nothing, so the server says that it is there.
    let deadline = Instant::now() + Duration::from_secs(30);
    while !quiet.take(&registry).contains(&LiveEvent::Heartbeat) {
        assert!(Instant::now() < deadline, "the server sends no heartbeat");
        std::thread::sleep(Duration::from_millis(2));
    }
    // Then the computer stays quiet, and the link ends with its own reason.
    let ending = quiet
        .frames
        .recv_timeout(Duration::from_secs(30))
        .expect("the error frame of a quiet link");
    match ending.frame {
        Frame::Error(error) => assert_eq!(error.code, LinkErrorCode::LiveSilent),
        other => panic!("not an error frame: {other:?}"),
    }
    if let Some(written) = ending.written {
        let _ = written.send(());
    }
    sign.join().expect("the thread of the sign of life");
    assert!(
        quiet.closed.load(Ordering::SeqCst),
        "the transport of a quiet link is closed"
    );

    // A computer that keeps answering keeps its link.
    let (busy, alive, sign) = signed(&registry, "laptop-b", quick(60));
    let deadline = Instant::now() + Duration::from_millis(300);
    while Instant::now() < deadline {
        alive.read();
        assert!(!busy.closed.load(Ordering::SeqCst), "a link that answers stays");
        std::thread::sleep(Duration::from_millis(10));
    }
    busy.outbox.close();
    sign.join().expect("the thread of the sign of life");
}
