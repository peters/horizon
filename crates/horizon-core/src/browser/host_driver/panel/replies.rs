//! The answers of the driver of a browser host, and the commands that a link
//! carries to the page of that machine.
//!
//! A link of a panel writes the commands of the person and of an agent as
//! requests of the wire, and it applies each answer of the driver: a picture into
//! the frame slot, the state of the page into the manifest of this computer, and
//! the result of an agent tool back to that agent. A refusal that ends the link
//! is a lost link with words, which the panel shows.

use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use horizon_browser::BrowserCoordination as _;
use horizon_browser::host_driver::{HostDriverReply, HostDriverRequest};
use horizon_browser::{BrowserCommand, BrowserEventWaker, CoordinationState, FrameSlot};
use horizon_browser_control::manifest::ManifestCoordination;

use super::super::client::DriverLink;
use super::super::latency::LinkProbe;
use super::{HostDriverEvent, LostLink, wake};
use crate::browser::AttachedHostFailure;

/// How long a write of the manifest of this computer is tried again while
/// another program holds the lock of that manifest.
///
/// An agent that lists panels and takes a screenshot holds the lock of the
/// manifest for short turns, and a loaded disk holds it longer than the one
/// budget of a single turn. One refused turn must not cost the link of the
/// driver, because the person would see a recovered page instead of their own.
const MANIFEST_WRITE_BUDGET: Duration = Duration::from_secs(6);
const MANIFEST_WRITE_STEP: Duration = Duration::from_millis(50);

/// Write the manifest of this computer, and try again while another writer holds
/// its lock, for a bounded time.
///
/// Only a lock that timed out is tried again: a manifest that this computer
/// cannot read or write for another reason is a reason to stop the link.
fn write_manifest(mut write: impl FnMut() -> std::io::Result<()>) -> std::io::Result<()> {
    let deadline = Instant::now() + MANIFEST_WRITE_BUDGET;
    loop {
        match write() {
            Err(error) if error.kind() == std::io::ErrorKind::TimedOut && Instant::now() < deadline => {
                std::thread::sleep(MANIFEST_WRITE_STEP);
            }
            outcome => return outcome,
        }
    }
}

/// The name of the wait that one command of the panel writes, for a stall line.
pub(super) fn request_method(command: &BrowserCommand) -> &'static str {
    match command {
        BrowserCommand::Input(_) => "input",
        BrowserCommand::Navigate(_) => "navigate",
        BrowserCommand::Reload => "reload",
        BrowserCommand::Back => "back",
        BrowserCommand::Forward => "forward",
        BrowserCommand::SetViewport { .. } => "viewport",
        BrowserCommand::DropFiles { .. }
        | BrowserCommand::Video { .. }
        | BrowserCommand::Orientation { .. }
        | BrowserCommand::NativeSelectChoose { .. }
        | BrowserCommand::NativeSelectDismiss
        | BrowserCommand::HandoffDone
        | BrowserCommand::Stop => "other",
    }
}

/// What an answer of the driver did to its link.
pub(super) enum ReplyOutcome {
    Continue,
    Lost(LostLink),
}

/// Apply one answer of the driver. Answers a lost link when this answer ends it.
#[allow(clippy::too_many_arguments)]
pub(super) fn apply_reply(
    link: &mut DriverLink,
    reply: HostDriverReply,
    panel_local_id: &str,
    frame_slot: &FrameSlot,
    events: &mpsc::Sender<HostDriverEvent>,
    waker: &Arc<Mutex<Option<BrowserEventWaker>>>,
    manifest: &ManifestCoordination,
    probe: &mut LinkProbe,
    received_at: Instant,
) -> ReplyOutcome {
    match reply {
        HostDriverReply::Frame(frame) => {
            let decoded_at = Instant::now();
            let ack = link.accept_frame(frame_slot, &frame);
            probe.frame_handed(
                frame.seq,
                frame.data.len(),
                (frame.width, frame.height),
                received_at,
                decoded_at,
                true,
            );
            if let Some(ack) = ack
                && let Err(words) = link.request(&ack)
            {
                return ReplyOutcome::Lost(LostLink::Unwritable(words));
            }
            wake(waker);
        }
        HostDriverReply::Page(page) => {
            let _ = events.send(HostDriverEvent::Page(page));
            wake(waker);
        }
        HostDriverReply::Refusal { code, message, fatal } => {
            if fatal {
                // The driver ended the link with its own words, so the panel
                // shows them and no attempt repeats a refusal that needs work on
                // the host.
                return ReplyOutcome::Lost(LostLink::Refused(AttachedHostFailure::refused(
                    &link.attached(),
                    &code,
                    &message,
                )));
            }
            let _ = events.send(HostDriverEvent::Note(message));
            wake(waker);
        }
        HostDriverReply::Coordination(state) => {
            let state = CoordinationState {
                backend: horizon_browser::BackendKind::FirefoxBidi,
                automation_disclosure: state.automation_disclosure,
                browser_ws: String::new(),
                target_id: state.target_id,
                page_resumed: state.page_resumed,
                url: state.url,
                title: state.title,
                remote_target: None,
                remote_device: None,
                remote_file_upload: false,
                remote_orientation: None,
                file_chooser: state.file_chooser,
                passkey_pending: state.passkey_pending,
                // A browser host of another machine keeps the profile of that
                // machine, which this computer does not read.
                profile: None,
            };
            // A manifest that this computer cannot write is a reason to stop a
            // link: an agent would drive a page whose owner nobody records. A
            // turn that another writer held is tried again first.
            if let Err(error) = write_manifest(|| update_manifest(manifest, panel_local_id, &state)) {
                return ReplyOutcome::Lost(LostLink::Ended(format!(
                    "the manifest of this computer did not take the state of the page: {error}"
                )));
            }
        }
        HostDriverReply::Action(result) => {
            if let Err(error) = write_manifest(|| manifest.complete_action(panel_local_id, &result.result)) {
                return ReplyOutcome::Lost(LostLink::Ended(format!(
                    "the manifest of this computer did not take the result of an agent: {error}"
                )));
            }
        }
        HostDriverReply::Audit(entry) => {
            if let Err(error) = write_manifest(|| manifest.record_action(panel_local_id, &entry)) {
                return ReplyOutcome::Lost(LostLink::Ended(format!(
                    "the manifest of this computer did not take the record of an agent: {error}"
                )));
            }
        }
        HostDriverReply::HandoffAck { request_id } => {
            let _ = manifest.acknowledge_handoff(panel_local_id, &request_id);
        }
        HostDriverReply::Welcome(_) => {}
        HostDriverReply::Closed => {
            return ReplyOutcome::Lost(LostLink::Ended(
                "the driver of the browser host ended the session of this profile".to_string(),
            ));
        }
    }
    ReplyOutcome::Continue
}

/// Write the state of the page of the host into the manifest of this computer.
fn update_manifest(
    manifest: &ManifestCoordination,
    panel_local_id: &str,
    state: &CoordinationState,
) -> std::io::Result<()> {
    // The first state of a panel of a host initializes the entry; a later one
    // updates it. Both are an update for a manifest that already holds the
    // entry, and `initialize` is what writes the fields that a driver owns.
    match manifest.update(panel_local_id, state) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => manifest.initialize(panel_local_id, state),
        Err(error) => Err(error),
    }
}

/// The words of a command that a page of another machine cannot take.
pub(super) fn unsupported_words(command: &BrowserCommand) -> Option<&'static str> {
    match command {
        BrowserCommand::DropFiles { .. } => Some(
            "this page runs on the browser host, so it cannot take files of this computer: open the file dialog of the page there",
        ),
        BrowserCommand::Video { .. } => {
            Some("a recording of the page of a browser host is not served yet: use a screen recording of that machine")
        }
        BrowserCommand::Orientation { .. }
        | BrowserCommand::NativeSelectChoose { .. }
        | BrowserCommand::NativeSelectDismiss => Some("this operation of the page of a browser host is not served yet"),
        BrowserCommand::Input(_)
        | BrowserCommand::Navigate(_)
        | BrowserCommand::Reload
        | BrowserCommand::Back
        | BrowserCommand::Forward
        | BrowserCommand::SetViewport { .. }
        | BrowserCommand::HandoffDone
        | BrowserCommand::Stop => None,
    }
}

/// The request of this computer that one command of the panel becomes, when the
/// link carries it.
pub(super) fn request_of(command: BrowserCommand) -> Option<HostDriverRequest> {
    match command {
        BrowserCommand::Navigate(url) => Some(HostDriverRequest::Navigate { url }),
        BrowserCommand::Reload => Some(HostDriverRequest::Reload),
        BrowserCommand::Back => Some(HostDriverRequest::Back),
        BrowserCommand::Forward => Some(HostDriverRequest::Forward),
        BrowserCommand::SetViewport { width, height } => Some(HostDriverRequest::Viewport { width, height }),
        BrowserCommand::Input(input) => Some(HostDriverRequest::Input { input }),
        // A page of another machine cannot take a file of this one, and the
        // panel says so where the command arrives; the operations that a host
        // driver does not serve are those of a browser that is not here.
        BrowserCommand::DropFiles { .. }
        | BrowserCommand::Video { .. }
        | BrowserCommand::Orientation { .. }
        | BrowserCommand::NativeSelectChoose { .. }
        | BrowserCommand::NativeSelectDismiss
        | BrowserCommand::HandoffDone
        | BrowserCommand::Stop => None,
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Error, ErrorKind};

    use super::{MANIFEST_WRITE_BUDGET, MANIFEST_WRITE_STEP, write_manifest};

    /// A lock that another writer held is tried again, so one busy turn of an
    /// agent does not cost the link of the driver.
    #[test]
    fn a_manifest_write_that_waited_for_its_lock_is_tried_again() {
        let mut attempts = 0;
        let written = write_manifest(|| {
            attempts += 1;
            if attempts < 3 {
                Err(Error::new(ErrorKind::TimedOut, "timed out waiting for manifest lock"))
            } else {
                Ok(())
            }
        });
        assert!(written.is_ok(), "the third turn of the write takes the lock");
        assert_eq!(attempts, 3);
    }

    /// A manifest that this computer cannot write for another reason is not a
    /// lock turn: the link stops at once, with the reason of the disk.
    #[test]
    fn a_manifest_write_that_cannot_succeed_stops_at_once() {
        let mut attempts = 0;
        let written = write_manifest(|| {
            attempts += 1;
            Err(Error::new(ErrorKind::PermissionDenied, "no room in the state folder"))
        });
        let error = written.expect_err("the write cannot succeed");
        assert_eq!(error.kind(), ErrorKind::PermissionDenied);
        assert_eq!(attempts, 1, "no turn waits for a reason that a wait cannot fix");
    }

    /// The wait is bounded: the panel stops the link after the budget and says so.
    #[test]
    fn the_wait_for_the_manifest_lock_is_bounded() {
        assert!(MANIFEST_WRITE_BUDGET <= std::time::Duration::from_secs(10));
        assert!(MANIFEST_WRITE_STEP >= std::time::Duration::from_millis(10));
        assert!(MANIFEST_WRITE_STEP * 2 <= MANIFEST_WRITE_BUDGET);
    }
}
