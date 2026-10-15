//! The core call that carries out a worker operation the card started.
use super::{Action, Event, Settings, cloud_runtime, deployment};

/// The event that ends worker operation `action` on the cloud at `root`, or `None` for an
/// action that is not one: a Stop carries out `agent_stop`, an agent's request, when given.
pub(super) fn power_result(
    action: Action,
    agent_stop: Option<&cloud_runtime::lifecycle::AgentStop>,
    root: &std::path::Path,
    settings: &Settings,
    cancel: &cloud_runtime::Cancellation,
    emit: &dyn Fn(Event),
) -> Option<cloud_runtime::Result<Event>> {
    Some(match action {
        Action::Stop => match agent_stop {
            Some(stop) => cloud_runtime::lifecycle::stop_for_agent(root, settings, cancel, stop),
            None => cloud_runtime::lifecycle::stop(root, settings, cancel),
        }
        .map(|state| Event::Stopped(Box::new(state))),
        Action::Resume => cloud_runtime::lifecycle::resume(root, settings, cancel).map(|()| Event::Resumed),
        Action::RevokeBrowserstack => cloud_runtime::lifecycle::revoke_browserstack(root, settings, cancel)
            .map(|state| Event::Snapshot(Box::new(state))),
        Action::Delete => deployment::terminate(root, settings, cancel, emit).map(|()| Event::deleted()),
        Action::Resize(_)
        | Action::Deploy
        | Action::Desktop
        | Action::Remove
        | Action::Reconcile
        | Action::PreviewRebuild
        | Action::Rebuild
        | Action::ContinueRebuild
        | Action::CancelRebuild
        | Action::ShareLocalNetwork
        | Action::StopSharingLocalNetwork
        | Action::ContainerRegistry { .. } => return None,
    })
}
