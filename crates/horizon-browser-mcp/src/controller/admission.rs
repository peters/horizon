//! Admission of new browser panels: one panel per engine for each agent,
//! counting the agent's creates that are still queued or in progress.

use horizon_browser::BackendKind;
use horizon_browser_control::manifest::{BrowserManifest, PendingCreate, RemoteCreateParameters};

use super::conflicting_panel;

/// Why a create needs `allow_additional`.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Conflict {
    /// An owned panel of the same engine, or any owned panel when the
    /// create's engine is unknown.
    Panel { panel_id: String, backend: BackendKind },
    /// A pending create of the agent that may open the same engine; `None`
    /// when that create's engine is unknown.
    Pending(Option<BackendKind>),
}

/// The engine a create will hold. `None` is a create without `backend` or
/// with a remote target: its engine is unknown here, so it conflicts with
/// every other create of the agent.
fn claimed_engine(backend: Option<BackendKind>, remote: bool) -> Option<BackendKind> {
    backend.filter(|_| !remote)
}

/// The reason a create of `parameters` needs `allow_additional`, given the
/// agent's owned panels and its pending creates, read under the agent's
/// create admission lock.
pub(super) fn conflict(
    owned: impl Iterator<Item = BrowserManifest>,
    pending: &[PendingCreate],
    parameters: &RemoteCreateParameters,
) -> Option<Conflict> {
    let engine = claimed_engine(parameters.backend, parameters.target.is_some());
    if let Some(pending) = pending
        .iter()
        .map(|pending| claimed_engine(pending.backend, pending.remote))
        .find(|pending| engine.is_none() || pending.is_none() || *pending == engine)
    {
        return Some(Conflict::Pending(pending));
    }
    conflicting_panel(owned, parameters).map(|panel| Conflict::Panel {
        panel_id: panel.panel_local_id,
        backend: panel.backend,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create(backend: Option<BackendKind>, target: Option<&str>) -> RemoteCreateParameters {
        RemoteCreateParameters {
            url: None,
            backend,
            target: target.map(str::to_string),
            orientation: None,
            visible: true,
        }
    }

    fn pending(backend: Option<BackendKind>, remote: bool) -> PendingCreate {
        PendingCreate { backend, remote }
    }

    fn check(pending: &[PendingCreate], parameters: &RemoteCreateParameters) -> Option<Conflict> {
        conflict(std::iter::empty(), pending, parameters)
    }

    #[test]
    fn a_pending_create_holds_its_engine() {
        let firefox = create(Some(BackendKind::FirefoxBidi), None);
        let queued = [pending(Some(BackendKind::FirefoxBidi), false)];
        assert_eq!(check(&[], &firefox), None);
        assert_eq!(
            check(&queued, &firefox),
            Some(Conflict::Pending(Some(BackendKind::FirefoxBidi)))
        );
        assert_eq!(
            check(&queued, &create(Some(BackendKind::ChromiumCdp), None)),
            None,
            "another engine is admitted beside the pending create"
        );
        assert!(
            matches!(check(&queued, &create(None, None)), Some(Conflict::Pending(_))),
            "a create of an unknown engine waits for every pending create"
        );
    }

    #[test]
    fn a_pending_create_of_an_unknown_engine_blocks_every_create() {
        for queued in [pending(None, false), pending(Some(BackendKind::ChromiumCdp), true)] {
            assert_eq!(
                check(&[queued], &create(Some(BackendKind::FirefoxBidi), None)),
                Some(Conflict::Pending(None))
            );
        }
    }
}
