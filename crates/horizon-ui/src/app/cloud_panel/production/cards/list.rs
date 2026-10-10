//! What the cloud list in the sidebar says of a cloud's card: the condition and
//! line of the status that the card's header shows, and the rate it bills now.
use super::super::Runtime;
use super::status::{self, Primary, Tone};
use super::{cost, view};
use horizon_core::{Board, cloud_list::Condition, cloud_panel::CloudGroup, cloud_runtime};
use std::time::SystemTime;

/// The condition of the cloud `group` and the line of its card's status.
pub(in crate::app::cloud_panel::production) fn condition(
    group: &CloudGroup,
    runtime: &Runtime,
    board: &Board,
    now: SystemTime,
) -> (Condition, String) {
    let status = status::of(runtime, view::occupancy(group, board), now);
    let condition = match status.tone {
        // A failed resume offers Resume too, but the resume that the user asked for
        // did not happen, so it needs the user.
        Tone::Failed => Condition::Failed,
        // A stopped worker offers Resume; it waits for nobody.
        Tone::Attention if status.primary == Some(Primary::Resume) => Condition::Stopped,
        Tone::Attention => Condition::Attention,
        Tone::Live => Condition::Busy,
        Tone::Ready => Condition::Ready,
        Tone::Idle => Condition::Idle,
    };
    let line = [status.verb.as_str(), status.numbers.as_str()]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" · ");
    (condition, line)
}

/// What the worker of `runtime` bills each hour now: nothing while it is stopped or deleted.
pub(in crate::app::cloud_panel::production) fn hourly_rate(runtime: &Runtime) -> Option<f64> {
    if cost::compute_idle(runtime) || cost::worker_deleted(runtime).is_some() {
        return None;
    }
    runtime
        .state
        .as_ref()
        .and_then(|state| state.worker.as_ref())
        .and_then(cloud_runtime::cost::hourly_rate)
}
