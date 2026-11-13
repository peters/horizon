//! One pass of the driver loop: the commands of the window, the protocol, the work
//! of the page, and the wait for the next command.
//!
//! Every blocking step of a pass carries a [`crate::latency::step`] guard, so the
//! line `latency stall` names the largest wait of the pass instead of reporting
//! that it does not know.

use std::sync::Arc;
use std::time::Instant;

use crate::cdp::CdpLink;
use crate::frames::FrameSlot;

use super::shared::DriverProcess;
use super::{BrowserEvent, BrowserEventSender, CommandReceiver, DriverState, IDLE_WAIT, MAX_CDP_BURST};

pub(super) fn run_loop(
    state: &mut DriverState,
    chrome: &mut DriverProcess,
    link: &mut CdpLink,
    command_rx: &CommandReceiver,
    frame_slot: &Arc<FrameSlot>,
    event_tx: &BrowserEventSender,
) {
    loop {
        let pass_started = Instant::now();
        // 1. Commands from the UI.
        let drain = crate::latency::step("drain-commands");
        let stop = state.drain_commands(link, command_rx, event_tx, frame_slot);
        drop(drain);
        if stop {
            state.flush_http_response_bodies(link, event_tx, frame_slot);
            state.capture_final_url(link, event_tx, frame_slot);
            state.settle_pending_wait_for_shutdown(Instant::now());
            if let Some(target) = state.target_id.as_deref() {
                chrome.close_page(link, target);
            } else {
                let _ = chrome.kill();
            }
            let _ = event_tx.send(BrowserEvent::Stopped { code: None });
            break;
        }

        // 2. CDP messages: responses and events. Drain a bounded burst of
        //    ready messages so sustained traffic keeps pace with what one
        //    pass can emit (input, viewport, clipboard, screencast acks).
        let mut connection_lost = None;
        let mut burst = 0;
        let naming = |name: &'static str| crate::latency::step(name);
        let cdp = naming("cdp-read");
        while burst < MAX_CDP_BURST {
            match link.read_one() {
                Ok(Some(message)) => {
                    state.handle_message(link, event_tx, frame_slot, message);
                    burst += 1;
                }
                Ok(None) => break,
                Err(error) => {
                    connection_lost = Some(error);
                    break;
                }
            }
        }
        drop(cdp);
        if let Some(error) = connection_lost {
            // The connection is gone. Reconnecting a single panel's
            // session is not worth the state surgery; surface the
            // failure and stop — the panel offers Retry.
            tracing::warn!(target: "browser", "cdp connection lost: {error}");
            state.audit_session_event(crate::BrowserAuditAction::LinkLost);
            let _ = event_tx.send(BrowserEvent::Warning(format!("CDP connection lost: {error}")));
            state.settle_pending_wait_for_shutdown(Instant::now());
            let _ = chrome.kill();
            let _ = event_tx.send(BrowserEvent::Stopped { code: None });
            break;
        }

        // Response bodies are opt-in and fetched only after the matching
        // loading-finished event. Keep each pass bounded so network traffic
        // cannot starve input, frames, or coordination.
        let http = naming("http-bodies");
        state.tick_http_response_bodies(link, event_tx, frame_slot);
        drop(http);
        if let Some(message) = state.challenge_loop.take_rejection() {
            let _ = event_tx.send(BrowserEvent::NavigationFailed(message.to_string()));
        }

        let chooser = naming("file-chooser");
        state.tick_file_chooser(link, event_tx, frame_slot);
        drop(chooser);
        let passkeys = naming("passkeys");
        state.tick_passkeys(link, event_tx, frame_slot);
        drop(passkeys);

        // 3. Flush a pending throttled manifest write (the loop always
        //    iterates, so a quiet page still gets its url/title flushed).
        let manifest = naming("manifest-write");
        state.write_manifest(false);
        drop(manifest);

        // 3b. Delayed post-load title fetch.
        let title = naming("title-fetch");
        state.tick_title_fetch(link, event_tx, frame_slot);
        drop(title);

        // 3c. Retry a transiently rejected viewport command, then capture one
        //     fresh frame after the resize burst settles.
        let viewport = naming("viewport-resize");
        state.tick_viewport_resize(link, event_tx, frame_slot);
        state.tick_viewport_capture(link, frame_slot);
        drop(viewport);

        // 3d. Keep the native-scrollbar hit-test geometry warm without ever
        //     blocking the input path on a CDP roundtrip.
        let scrollbar = naming("scrollbar-layout");
        state.tick_scrollbar_layout(link);
        drop(scrollbar);

        // 4. Backoff-retried screencast restart / re-attach.
        let screencast = naming("screencast-restart");
        state.pending_restart_tick(link, event_tx, frame_slot);
        drop(screencast);

        // 5. Ownership, handoff, the requests that wait, the liveness of the
        //    browser, and the wait for the next command.
        if !finish_pass(state, chrome, link, command_rx, frame_slot, event_tx, pass_started) {
            break;
        }
    }
    state.finish_pending_resize("browser_unavailable", "browser session stopped");
}

/// The end of one driver pass: the signals and the actions of an agent, the
/// requests that wait, the liveness of the browser, and the wait for the next
/// command of the window. Answers whether the loop continues.
fn finish_pass(
    state: &mut DriverState,
    chrome: &mut DriverProcess,
    link: &mut CdpLink,
    command_rx: &CommandReceiver,
    frame_slot: &Arc<FrameSlot>,
    event_tx: &BrowserEventSender,
    pass_started: Instant,
) -> bool {
    let naming = |name: &'static str| crate::latency::step(name);
    let liveness = naming("chrome-liveness");
    let exited = stop_for_chrome_exit(state, chrome, event_tx);
    drop(liveness);
    if exited {
        return false;
    }
    let signals = naming("agent-signals");
    let actions = state.tick_signals(event_tx);
    let _ = state.drain_agent_actions(link, event_tx, frame_slot, actions, chrome);
    drop(signals);
    let liveness = naming("chrome-liveness");
    let exited = stop_for_chrome_exit(state, chrome, event_tx);
    drop(liveness);
    if exited {
        return false;
    }
    let pending = naming("pending-requests");
    state.tick_pending_navigation();
    state.tick_pending_wait(link, event_tx, frame_slot, chrome);
    state.tick_pending_resize(link, event_tx, frame_slot);
    drop(pending);

    let liveness = naming("chrome-liveness");
    let exited = stop_for_chrome_exit(state, chrome, event_tx);
    drop(liveness);
    if exited {
        return false;
    }

    crate::latency::report_blocked_pass(link, pass_started);
    // The next command of the window wakes this wait at once, so an input that
    // arrives while the page is quiet is taken by the next pass and not after a
    // fixed sleep. The socket read of the pass waits for the page with its own,
    // shorter bound, so the two together are the idle period of the loop.
    let idle = naming("idle-wait");
    command_rx.wait_for(IDLE_WAIT);
    drop(idle);
    true
}

fn stop_for_chrome_exit(state: &mut DriverState, chrome: &mut DriverProcess, event_tx: &BrowserEventSender) -> bool {
    let Some(status) = chrome.child_status() else {
        return false;
    };
    state.settle_pending_wait_for_shutdown(Instant::now());
    let _ = event_tx.send(BrowserEvent::Stopped { code: status.code() });
    true
}
