//! Host-side handling for agent-requested browser panel visibility changes.
//! The board decides whether a request may apply. The coordination worker
//! then audits it and publishes the manifest's visibility and the result, and
//! a later frame shows or hides the panel once all of that succeeded, so a
//! slow disk never stalls a frame.

use horizon_core::PanelKind;
use horizon_core::browser::manifest::{
    self, BrowserVisibilityAuditStatus, BrowserVisibilityRequest, BrowserVisibilityResult, HostStampOutcome,
    ManifestWorkspace, OwnedStampOutcome,
};

use super::HorizonApp;
use super::browser_host_io::BoardFence;
use super::browser_manifest_stamp::browser_workspace;
use super::browser_request_claims::HOST_RETIRING;
use super::browser_requests::{ActorPanel, actor_panel};

type Refusal = (&'static str, &'static str);

/// A visibility request that runs on the coordination worker.
pub(super) struct VisibilityInFlight {
    pub(super) local_id: String,
    request_id: String,
    /// The visibility it sets.
    visible: bool,
    /// The visibility the panel has when it applies, unless the user changes it.
    before: bool,
}

impl HorizonApp {
    pub(super) fn apply_browser_visibility_request(
        &mut self,
        request: &BrowserVisibilityRequest,
        actor_panel: ActorPanel,
    ) {
        let (original_visible, workspace) = match self.visibility_target(request, actor_panel) {
            Ok(target) => target,
            Err((code, message)) => {
                self.fail_visibility(request, code, message);
                return;
            }
        };
        // Until the request ends, a stamp keeps the visibility the manifest has,
        // so a move still updates the workspace while the visibility changes
        // only through the audited transaction.
        self.browser_create_host.visibility_in_flight.push(VisibilityInFlight {
            local_id: request.panel_local_id.clone(),
            request_id: request.request_id.clone(),
            visible: request.visible,
            before: original_visible,
        });
        let (published, applied, live) = (
            request.clone(),
            request.clone(),
            self.browser_create_host.io.live_fence(),
        );
        self.browser_create_host.io.then(
            move || publish_visibility(&HomeFiles, &live, &published, &workspace),
            move |app, published| app.finish_visibility_request(&applied, published),
        );
    }

    /// The panel's visibility before the request and its workspace stamp,
    /// when the request may change it. A request that runs for the same
    /// panel already sets that visibility first.
    fn visibility_target(
        &self,
        request: &BrowserVisibilityRequest,
        actor_panel: ActorPanel,
    ) -> Result<(bool, ManifestWorkspace), Refusal> {
        if request.deadline_at_millis < manifest::now_millis() {
            return Err(("request_expired", "browser visibility request expired"));
        }
        let panel_id = self.board.panel_id_by_local_id(&request.panel_local_id).ok_or((
            "panel_not_in_host",
            "browser panel is not hosted by the requesting agent's Horizon instance",
        ))?;
        let panel = self
            .board
            .panel(panel_id)
            .ok_or(("panel_closed", "browser panel is not live"))?;
        if panel.kind != PanelKind::Browser {
            return Err(("not_browser_panel", "target panel is not a browser panel"));
        }
        if panel.workspace_id != actor_panel.workspace_id {
            return Err((
                "panel_outside_workspace",
                "browser panel is outside the requesting agent's Horizon workspace",
            ));
        }
        let workspace = browser_workspace(&self.board, panel.workspace_id)
            .ok_or(("workspace_unavailable", "browser panel workspace is not live"))?;
        let before = self
            .browser_create_host
            .visibility_in_flight
            .iter()
            .rev()
            .find(|running| running.local_id == request.panel_local_id)
            .map_or(panel.visible, |running| running.visible);
        Ok((before, workspace))
    }

    /// Shows or hides the panel once its manifest, audit and result say so.
    /// The board as it is now decides where the panel is; the next stamp writes
    /// that placement, including a move made while the request ran. A panel
    /// that left the agent's workspace, or that the user showed or hid while
    /// the request ran, keeps what the board says: the user's change wins, and
    /// the next stamp writes it to the manifest. A request that leaves the
    /// board as it was lets the next one for the panel start from the board.
    fn finish_visibility_request(&mut self, request: &BrowserVisibilityRequest, published: Option<bool>) {
        let host = &mut self.browser_create_host;
        let before = host
            .visibility_in_flight
            .iter()
            .position(|running| running.request_id == request.request_id)
            .map(|index| host.visibility_in_flight.remove(index).before);
        host.forget_stamped_placement();
        let applied = published == Some(true) && before.is_some_and(|before| self.show_published(request, before));
        if !applied {
            self.rebase_next_visibility_request(&request.panel_local_id);
        }
        if published.is_none() {
            self.fail_visibility(
                request,
                "manifest_update_failed",
                "browser panel visibility could not be updated",
            );
        }
    }

    /// Applies a published request to the board when it still may. Says whether it did.
    /// A board that retires since keeps the change it published, and saves
    /// it before it goes; a board that replaced it never gets it.
    fn show_published(&mut self, request: &BrowserVisibilityRequest, before: bool) -> bool {
        if self.browser_create_host.io.applying_stale() {
            return false;
        }
        let Some(panel_id) = self.board.panel_id_by_local_id(&request.panel_local_id) else {
            return false;
        };
        // A user who hides and shows the panel again within the few
        // milliseconds the worker runs leaves it as it was, so the request
        // still applies; telling the two apart needs a visibility revision
        // on the panel, which the board does not keep.
        let still_applies = self.board.panel(panel_id).is_some_and(|panel| {
            panel.visible == before
                && actor_panel(&self.board, &request.actor)
                    .is_some_and(|actor_panel| actor_panel.workspace_id == panel.workspace_id)
        });
        if !still_applies {
            return false;
        }
        let changed = self.board.set_panel_visible(panel_id, request.visible);
        if !request.visible && self.fullscreen_panel == Some(panel_id) {
            self.fullscreen_panel = None;
        }
        if !request.visible
            && self.board.focused.is_none()
            && let Some(actor_panel) = actor_panel(&self.board, &request.actor)
        {
            self.board.focus(actor_panel.panel_id);
        }
        if changed {
            self.mark_runtime_dirty();
        }
        true
    }

    /// The next request for the panel expected this one's visibility; it did not
    /// reach the board, so that request starts from the board instead.
    fn rebase_next_visibility_request(&mut self, local_id: &str) {
        let Some(visible) = self
            .board
            .panel_id_by_local_id(local_id)
            .and_then(|panel_id| self.board.panel(panel_id))
            .map(|panel| panel.visible)
        else {
            return;
        };
        if let Some(next) = self
            .browser_create_host
            .visibility_in_flight
            .iter_mut()
            .find(|running| running.local_id == local_id)
        {
            next.before = visible;
        }
    }

    /// Publishes a refused visibility request on the coordination worker.
    pub(super) fn fail_visibility(&mut self, request: &BrowserVisibilityRequest, code: &str, message: &str) {
        let request = request.clone();
        let result = BrowserVisibilityResult::failed(&request, code, message);
        self.browser_create_host
            .io
            .write(move || complete_visibility_failure(&HomeFiles, &request, &result));
    }
}

/// The file work of one visibility request: its audits, the locked manifest
/// write and its result. The worker uses those of this Horizon home.
trait VisibilityFiles {
    /// The agent that holds the panel's live lease, read without the lock.
    fn owner(&self, request: &BrowserVisibilityRequest) -> Option<String>;
    fn audit(&self, request: &BrowserVisibilityRequest, status: BrowserVisibilityAuditStatus) -> std::io::Result<()>;
    /// Writes `visible` only while the requesting agent holds the panel's lease.
    fn publish(
        &self,
        request: &BrowserVisibilityRequest,
        visible: bool,
        workspace: &ManifestWorkspace,
    ) -> std::io::Result<OwnedStampOutcome>;
    fn complete(&self, result: &BrowserVisibilityResult) -> std::io::Result<()>;
}

/// The manifests, audits and results of this Horizon home.
struct HomeFiles;

impl VisibilityFiles for HomeFiles {
    fn owner(&self, request: &BrowserVisibilityRequest) -> Option<String> {
        let manifest = manifest::read(&request.panel_local_id)?;
        manifest
            .live_owner(manifest::now_millis())
            .map(|owner| owner.name.clone())
    }

    fn audit(&self, request: &BrowserVisibilityRequest, status: BrowserVisibilityAuditStatus) -> std::io::Result<()> {
        manifest::record_visibility_status(request, status)
    }

    fn publish(
        &self,
        request: &BrowserVisibilityRequest,
        visible: bool,
        workspace: &ManifestWorkspace,
    ) -> std::io::Result<OwnedStampOutcome> {
        manifest::sync_owned_host_state(&request.panel_local_id, visible, workspace, &request.actor)
    }

    fn complete(&self, result: &BrowserVisibilityResult) -> std::io::Result<()> {
        manifest::complete_visibility_request(result)
    }
}

/// Audits the request and publishes the manifest's new visibility and the
/// result, on the coordination worker. The owner is checked again in the same
/// locked transaction as each manifest write, so a lease that moves while
/// earlier jobs or the audits run is never crossed. A completion that cannot be
/// audited restores the visibility the manifest had, while the agent still
/// holds the lease, and a request that expired, lost its owner, or whose board
/// retired while earlier jobs ran is refused. Says whether the change was
/// published.
fn publish_visibility(
    files: &impl VisibilityFiles,
    live: &BoardFence,
    request: &BrowserVisibilityRequest,
    workspace: &ManifestWorkspace,
) -> bool {
    let refuse = |code, message| {
        complete_visibility_failure(files, request, &BrowserVisibilityResult::failed(request, code, message));
        false
    };
    if !live.holds() {
        return refuse(HOST_RETIRING.0, HOST_RETIRING.1);
    }
    if request.deadline_at_millis < manifest::now_millis() {
        return refuse("request_expired", "browser visibility request expired");
    }
    // A request that already lost its owner is refused before it is audited
    // as dispatched; the locked write checks the owner again.
    if files.owner(request).as_deref() != Some(request.actor.as_str()) {
        return refuse("ownership_changed", "browser panel ownership changed");
    }
    if let Err(error) = files.audit(request, BrowserVisibilityAuditStatus::Dispatched) {
        tracing::warn!(request_id = %request.request_id, %error, "could not audit browser visibility dispatch");
        return refuse("audit_failed", "Horizon refused an unaudited visibility change");
    }
    let restore = match files.publish(request, request.visible, workspace) {
        Ok(OwnedStampOutcome::Stamped {
            stamp: HostStampOutcome::Written | HostStampOutcome::Unchanged,
            was_visible,
        }) => was_visible,
        Ok(OwnedStampOutcome::OwnerChanged) => {
            return refuse("ownership_changed", "browser panel ownership changed");
        }
        Ok(OwnedStampOutcome::Stamped {
            stamp: HostStampOutcome::NotOwned,
            ..
        }) => {
            tracing::warn!(request_id = %request.request_id, "browser panel's driver runs in another Horizon host");
            return refuse(
                "manifest_update_failed",
                "browser panel visibility could not be updated",
            );
        }
        Err(error) => {
            tracing::warn!(request_id = %request.request_id, %error, "could not update browser manifest visibility");
            return refuse(
                "manifest_update_failed",
                "browser panel visibility could not be updated",
            );
        }
    };
    if let Err(error) = files.audit(request, BrowserVisibilityAuditStatus::Completed) {
        tracing::warn!(request_id = %request.request_id, %error, "could not audit browser visibility completion");
        let _ = files.publish(request, restore, workspace);
        return refuse("audit_failed", "visibility change could not be audited");
    }
    complete_visibility_result(files, &BrowserVisibilityResult::ready(request));
    true
}

fn complete_visibility_failure(
    files: &impl VisibilityFiles,
    request: &BrowserVisibilityRequest,
    result: &BrowserVisibilityResult,
) {
    if let Err(error) = files.audit(request, BrowserVisibilityAuditStatus::Failed) {
        tracing::warn!(request_id = %request.request_id, %error, "could not append failed browser visibility audit");
    }
    complete_visibility_result(files, result);
}

fn complete_visibility_result(files: &impl VisibilityFiles, result: &BrowserVisibilityResult) {
    if let Err(error) = files.complete(result) {
        tracing::error!(request_id = %result.request_id, %error, "could not publish browser visibility result");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::test_support::test_app;
    use horizon_core::browser::BrowserPanelState;
    use horizon_core::{Panel, PanelContent, PanelId, PanelOptions, WorkspaceId, browser_actor};

    /// An agent and a shown browser panel in one workspace, and another
    /// workspace. Returns the agent's actor name.
    fn fixture() -> (tempfile::TempDir, HorizonApp, String, WorkspaceId) {
        let (temp, mut app) = test_app();
        let workspace = app.board.create_workspace("agent");
        let other = app.board.create_workspace("other");
        let agent = app
            .board
            .create_panel(
                PanelOptions {
                    command: Some("/bin/sh".into()),
                    args: vec!["-c".into(), "exit 0".into()],
                    kind: PanelKind::Codex,
                    ..PanelOptions::default()
                },
                workspace,
            )
            .expect("agent panel");
        let actor = browser_actor(&app.board.panel(agent).expect("agent").local_id);
        let browser = Panel::from_content(
            PanelId(900),
            workspace,
            PanelKind::Browser,
            PanelContent::Browser(Box::new(BrowserPanelState::inert())),
        );
        app.board.panels.push(browser);
        app.board.assign_panel_to_workspace(PanelId(900), workspace);
        (temp, app, actor, other)
    }

    fn request(app: &HorizonApp, actor: &str, id: &str, visible: bool) -> BrowserVisibilityRequest {
        let panel = app.board.panel(PanelId(900)).expect("browser");
        serde_json::from_value(serde_json::json!({
            "request_id": id,
            "actor": actor,
            "panel_local_id": panel.local_id,
            "visible": visible,
            "requested_at_millis": 0,
            "deadline_at_millis": i64::MAX,
        }))
        .expect("request")
    }

    fn visible(app: &HorizonApp) -> bool {
        app.board.panel(PanelId(900)).expect("browser").visible
    }

    /// Registers `request` as running, as `apply_browser_visibility_request` does.
    fn running(app: &mut HorizonApp, request: &BrowserVisibilityRequest) {
        let actor_panel = actor_panel(&app.board, &request.actor).expect("actor");
        let (before, _) = app.visibility_target(request, actor_panel).expect("target");
        app.browser_create_host.visibility_in_flight.push(VisibilityInFlight {
            local_id: request.panel_local_id.clone(),
            request_id: request.request_id.clone(),
            visible: request.visible,
            before,
        });
    }

    #[test]
    #[cfg_attr(windows, ignore = "agent panels launch through a POSIX login shell (#688)")]
    fn a_published_change_leaves_a_panel_that_left_the_workspace_or_that_the_user_changed() {
        let (_temp, mut app, actor, other) = fixture();
        let hide = request(&app, &actor, "hide", false);
        running(&mut app, &hide);
        let workspace = app.board.panel(PanelId(900)).expect("browser").workspace_id;
        app.board.assign_panel_to_workspace(PanelId(900), other);
        app.finish_visibility_request(&hide, Some(true));
        assert!(visible(&app), "a panel outside the agent's workspace is not hidden");

        app.board.assign_panel_to_workspace(PanelId(900), workspace);
        let show = request(&app, &actor, "show", true);
        running(&mut app, &show);
        app.board.set_panel_visible(PanelId(900), false);
        app.finish_visibility_request(&show, Some(true));
        assert!(!visible(&app), "the user hid the panel while the request ran");

        running(&mut app, &show);
        app.finish_visibility_request(&show, Some(true));
        assert!(visible(&app), "a request that still applies shows the panel");
    }

    #[test]
    #[cfg_attr(windows, ignore = "agent panels launch through a POSIX login shell (#688)")]
    fn a_stamp_moves_a_panel_whose_request_runs_and_leaves_its_visibility_to_the_request() {
        let (_temp, mut app, actor, other) = fixture();
        let local_id = app.board.panel(PanelId(900)).expect("browser").local_id.clone();
        let path = manifest::manifest_path_for_root(app.session_store.home().root(), &local_id);
        let live = manifest::BrowserManifest {
            host: Some(manifest::host_instance().to_string()),
            ..manifest::BrowserManifest::default()
        };
        manifest::write_at(&path, &live).expect("live manifest");
        let hide = request(&app, &actor, "hide", false);
        running(&mut app, &hide);
        app.board.assign_panel_to_workspace(PanelId(900), other);
        assert!(app.restamp_browser_manifests_for_placement());
        app.settle_browser_host_io();
        let stamped = manifest::read_at(&path).expect("manifest");
        let other_id = app.board.workspace(other).expect("other").local_id.clone();
        assert_eq!(
            stamped.workspace.map(|workspace| workspace.local_id),
            Some(other_id),
            "the move revokes the old workspace at once"
        );
        assert!(
            !stamped.hidden,
            "only the request, or a stamp after it, changes the visibility"
        );
    }

    #[test]
    #[cfg_attr(windows, ignore = "agent panels launch through a POSIX login shell (#688)")]
    fn a_request_after_one_that_runs_starts_from_the_visibility_it_sets() {
        let (_temp, mut app, actor, _) = fixture();
        let first = request(&app, &actor, "first", false);
        running(&mut app, &first);
        let second = request(&app, &actor, "second", false);
        running(&mut app, &second);
        let actor_panel = actor_panel(&app.board, &actor).expect("actor");
        let show = request(&app, &actor, "show", true);
        let (before, _) = app.visibility_target(&show, actor_panel).expect("target");
        assert!(!before, "the hides that run first set what a rollback restores");

        // The first hide fails: the board stays as it was, and the second starts from it.
        app.finish_visibility_request(&first, Some(false));
        assert!(visible(&app));
        app.finish_visibility_request(&second, Some(true));
        assert!(!visible(&app), "the second hide applies");
    }

    /// The files of one visibility request under a test root. `on_audit` runs
    /// for each audit before it is recorded, and may fail it.
    struct TestFiles<'a> {
        root: &'a std::path::Path,
        on_audit: Box<dyn Fn(BrowserVisibilityAuditStatus) -> std::io::Result<()> + 'a>,
        results: std::cell::RefCell<Vec<BrowserVisibilityResult>>,
    }

    impl VisibilityFiles for TestFiles<'_> {
        fn owner(&self, request: &BrowserVisibilityRequest) -> Option<String> {
            let path = manifest::manifest_path_for_root(self.root, &request.panel_local_id);
            let manifest = manifest::read_at(&path)?;
            manifest
                .live_owner(manifest::now_millis())
                .map(|owner| owner.name.clone())
        }

        fn audit(&self, _: &BrowserVisibilityRequest, status: BrowserVisibilityAuditStatus) -> std::io::Result<()> {
            (self.on_audit)(status)
        }

        fn publish(
            &self,
            request: &BrowserVisibilityRequest,
            visible: bool,
            workspace: &ManifestWorkspace,
        ) -> std::io::Result<OwnedStampOutcome> {
            manifest::sync_owned_host_state_in(self.root, &request.panel_local_id, visible, workspace, &request.actor)
        }

        fn complete(&self, result: &BrowserVisibilityResult) -> std::io::Result<()> {
            self.results.borrow_mut().push(result.clone());
            Ok(())
        }
    }

    #[test]
    #[cfg_attr(windows, ignore = "agent panels launch through a POSIX login shell (#688)")]
    fn a_lease_that_moves_while_the_request_runs_keeps_the_new_owners_manifest() {
        let (_temp, app, actor, _) = fixture();
        let local_id = app.board.panel(PanelId(900)).expect("browser").local_id.clone();
        let root = app.session_store.home().root().to_path_buf();
        let path = manifest::manifest_path_for_root(&root, &local_id);
        let workspace = ManifestWorkspace::new(
            manifest::host_instance(),
            "agent",
            vec![actor.clone(), "horizon:other-agent".into()],
        );
        let lease = |name: &str| {
            let mut live = manifest::read_at(&path).unwrap_or_default();
            live.host = Some(manifest::host_instance().to_string());
            live.owner = Some(manifest::ManifestOwner {
                name: name.to_string(),
                tty: None,
                updated_at: manifest::now_millis(),
            });
            manifest::write_at(&path, &live).expect("lease");
        };
        let live = app.browser_create_host.io.live_fence();
        let hide = request(&app, &actor, "hide", false);
        // What the manifest holds once the other agent took it.
        let taken = std::cell::RefCell::new(Vec::new());
        let take = || {
            lease("horizon:other-agent");
            taken.replace(std::fs::read(&path).expect("manifest"));
        };

        // The agent owns the panel when the request starts; another agent takes
        // it while the dispatch audit waits, after the early check.
        lease(&actor);
        let files = TestFiles {
            root: &root,
            on_audit: Box::new(|status| {
                if status == BrowserVisibilityAuditStatus::Dispatched {
                    take();
                }
                Ok(())
            }),
            results: std::cell::RefCell::default(),
        };
        assert!(!publish_visibility(&files, &live, &hide, &workspace));
        assert_eq!(
            std::fs::read(&path).expect("manifest"),
            taken.take(),
            "nothing was written"
        );
        assert!(!manifest::read_at(&path).expect("manifest").hidden);
        assert_eq!(
            files.results.take(),
            [BrowserVisibilityResult::failed(
                &hide,
                "ownership_changed",
                "browser panel ownership changed"
            )],
            "the request does not report ready"
        );

        // The hide is written while the agent owns the panel; the lease moves
        // before its completion audit fails, so the rollback leaves it alone.
        lease(&actor);
        let files = TestFiles {
            root: &root,
            on_audit: Box::new(|status| {
                if status == BrowserVisibilityAuditStatus::Completed {
                    take();
                    return Err(std::io::Error::other("a fixture audit fails"));
                }
                Ok(())
            }),
            results: std::cell::RefCell::default(),
        };
        assert!(!publish_visibility(&files, &live, &hide, &workspace));
        assert_eq!(
            std::fs::read(&path).expect("manifest"),
            taken.take(),
            "nothing was rolled back"
        );
        assert!(
            manifest::read_at(&path).expect("manifest").hidden,
            "the hide was written before"
        );
        assert_eq!(
            files.results.take(),
            [BrowserVisibilityResult::failed(
                &hide,
                "audit_failed",
                "visibility change could not be audited"
            )]
        );

        // An agent that keeps the lease gets the visibility it replaced back.
        lease(&actor);
        let show = request(&app, &actor, "show", true);
        let files = TestFiles {
            root: &root,
            on_audit: Box::new(|status| match status {
                BrowserVisibilityAuditStatus::Completed => Err(std::io::Error::other("a fixture audit fails")),
                _ => Ok(()),
            }),
            results: std::cell::RefCell::default(),
        };
        assert!(!publish_visibility(&files, &live, &show, &workspace));
        assert!(
            manifest::read_at(&path).expect("manifest").hidden,
            "the show was rolled back"
        );
    }
}
