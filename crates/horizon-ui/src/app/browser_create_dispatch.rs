//! The dispatch, audit and publication of the browser panels an agent asks for.

use std::time::Instant;

use horizon_core::browser::manifest::{
    self, BrowserCreateAuditStatus, BrowserCreateRequest, BrowserCreateResult, ManifestWorkspace,
};
use horizon_core::browser::{BackendAvailability, BackendKind, NewBrowserPanel, new_browser_panel};
use horizon_core::{PanelKind, PanelOptions};

use super::HorizonApp;
use super::browser_create_failure::{
    CREATE_TIMEOUT, complete_result, record_and_complete_failure, record_create_failure,
};
use super::browser_create_lifecycle::{CreateStage, CreatedPanel, PendingBrowserCreate};
use super::browser_host_io::BoardFence;
use super::browser_manifest_stamp::{backend_session_limit_reached, host_identity};
use super::browser_request_claims::{AGENT_GONE, HOST_RETIRING, fail_create, refuse_expired_create};
use super::browser_requests::{ActorPanel, actor_panel};

impl HorizonApp {
    /// Starts a create that the worker claimed on the board as it is now. A
    /// create that duplicates an existing panel reads that panel's manifest on
    /// the coordination worker first: the read retries for up to two seconds on
    /// Windows, so the frame that starts the create never does it. Only a
    /// duplicate create takes that extra ordered job; every other create starts
    /// on this frame, as before.
    pub(super) fn start_claimed_browser_create(&mut self, request: BrowserCreateRequest) {
        let Some(source) = request.duplicate_from.clone() else {
            self.start_claimed_browser_create_on_board(request, None);
            return;
        };
        // The app's home is the root the drivers use for their default paths:
        // production builds the session store from `HorizonHome::resolve()`.
        let path = manifest::manifest_path_for_root(self.host_manifest_root(), &source);
        self.browser_create_host.io.then(
            move || manifest::read_at(&path),
            move |app, source| app.start_claimed_browser_create_on_board(request, source.flatten().as_ref()),
        );
    }

    /// Starts or refuses the create on the board as it is once its source
    /// manifest was read, with the reasons `apply_claimed_requests` uses: a
    /// board that retires meanwhile, or an agent that left it. A manifest that
    /// could not be read reaches [`Self::duplicate_browser_options`] as an
    /// absent one, which refuses the duplicate as it does today.
    fn start_claimed_browser_create_on_board(
        &mut self,
        request: BrowserCreateRequest,
        source_manifest: Option<&manifest::BrowserManifest>,
    ) {
        if self.browser_host_retiring() {
            fail_create(
                &mut self.browser_create_host.io,
                &request,
                HOST_RETIRING.0,
                HOST_RETIRING.1,
            );
            return;
        }
        match actor_panel(&self.board, &request.actor) {
            Some(actor_panel) => self.start_requested_browser(request, actor_panel, source_manifest),
            None => fail_create(
                &mut self.browser_create_host.io,
                &request,
                "workspace_unavailable",
                AGENT_GONE,
            ),
        }
    }

    pub(super) fn start_requested_browser(
        &mut self,
        mut request: BrowserCreateRequest,
        actor_panel: ActorPanel,
        source_manifest: Option<&manifest::BrowserManifest>,
    ) {
        // Startup latency counts everything the host does from accepting the
        // request, including panel creation and the audit writes.
        let started_at = Instant::now();
        if refuse_expired_create(&mut self.browser_create_host.io, &request) {
            return;
        }
        let duplicate = match self.prepare_browser_duplicate(&mut request, actor_panel, source_manifest) {
            Ok(options) => options,
            Err((code, message)) => {
                fail_create(&mut self.browser_create_host.io, &request, code, message);
                return;
            }
        };
        // A remote target takes the value and slot work of its plan off this
        // thread; its panel opens on a later frame, once they are at hand.
        if request.target.is_some() {
            self.begin_remote_create(request, actor_panel, started_at);
            return;
        }
        let backend = request.backend.unwrap_or(self.template_config.browser.backend);
        // One rule decides the machine, the engine and the profile of every new
        // browser panel, and it reads the entry that the settings hold now. A
        // create that names an engine that the host does not run is a panel of
        // this computer in that engine, and its answer says so.
        let panel = match new_browser_panel(
            request.profile.as_deref(),
            request.backend,
            backend,
            &self.template_config.browser_host,
        ) {
            Ok(panel) => panel,
            Err(message) => {
                fail_create(&mut self.browser_create_host.io, &request, "invalid_profile", &message);
                return;
            }
        };
        if let BackendAvailability::UnsupportedPlatform(reason) = backend.availability() {
            fail_create(
                &mut self.browser_create_host.io,
                &request,
                "unsupported_platform",
                reason,
            );
            return;
        }
        if backend_session_limit_reached(&self.board, backend) {
            fail_create(
                &mut self.browser_create_host.io,
                &request,
                "session_limit_reached",
                "the selected browser backend has reached its live-session limit",
            );
            return;
        }

        let mut browser_config = self.template_config.browser.clone();
        browser_config.backend = backend;
        let note = duplicate.is_none().then(|| panel.note.clone()).flatten();
        let options = duplicate.unwrap_or_else(|| browser_create_options(&request, browser_config, &panel));
        self.open_requested_browser_panel(request, actor_panel, options, note, started_at);
    }

    /// Opens the panel of a create whose request is resolved, and audits its
    /// dispatch. The backend, the startup orientation and the remote allocation
    /// cancelled by a failed creation come from `options`, so a local create and
    /// a remote create that was planned off this thread take one path here.
    /// `note` is the one line that the answer of the create carries when the page
    /// does not run on the browser host of the settings.
    pub(super) fn open_requested_browser_panel(
        &mut self,
        request: BrowserCreateRequest,
        actor_panel: ActorPanel,
        options: PanelOptions,
        note: Option<String>,
        started_at: Instant,
    ) {
        let backend = options.browser_config.as_ref().map_or_else(
            || request.backend.unwrap_or(self.template_config.browser.backend),
            |config| config.backend,
        );
        let startup_orientation = options
            .remote_session
            .as_ref()
            .and_then(horizon_core::browser::RemoteSessionRequest::orientation);
        let recovery = options.remote_session.as_ref().map(|session| session.recovery.clone());
        let panel_id = match self.create_agent_child_panel(options, actor_panel.workspace_id, actor_panel.panel_id) {
            Ok(panel_id) => panel_id,
            Err(error) => {
                if let Some(recovery) = &recovery {
                    recovery.cancel_before_launch();
                }
                tracing::error!(request_id = %request.request_id, %error, "failed to create requested browser panel");
                fail_create(
                    &mut self.browser_create_host.io,
                    &request,
                    "panel_create_failed",
                    "Horizon could not create the requested browser panel; inspect local logs",
                );
                return;
            }
        };
        let Some(panel_local_id) = self.board.panel(panel_id).map(|panel| panel.local_id.clone()) else {
            tracing::error!(request_id = %request.request_id, "created browser panel disappeared before registration");
            fail_create(
                &mut self.browser_create_host.io,
                &request,
                "panel_create_failed",
                "Horizon could not register the requested browser panel",
            );
            return;
        };
        // A navigation that the panel took before this point belongs to the
        // lane that made the page, not to the person: the first address of a
        // panel of a browser host is one such navigation. The create reads the
        // count of the live panel, so only a navigation after it is a takeover.
        let user_navigations_at_start = self
            .board
            .panel(panel_id)
            .and_then(horizon_core::Panel::browser)
            .map_or(0, horizon_core::browser::BrowserPanelState::user_navigation_count);
        self.audit_requested_browser(PendingBrowserCreate {
            request,
            started_at,
            stage: CreateStage::Auditing,
            board: self.browser_create_host.io.board_fence(),
            panel: Some(CreatedPanel {
                local_id: panel_local_id,
                backend,
                startup_orientation,
                ready_since: None,
                user_navigations_at_start,
                manifest: None,
                manifest_read_at: None,
                manifest_reading: false,
                note,
            }),
            remote_credentials: None,
        });
    }

    /// Registers the create and audits its dispatch on the coordination
    /// worker. It waits for that audit before it may complete: a journal that
    /// cannot be written closes the panel instead.
    fn audit_requested_browser(&mut self, pending: PendingBrowserCreate) {
        let request_id = pending.request.request_id.clone();
        let Some(panel) = pending.panel.as_ref() else { return };
        let audited = (
            panel.local_id.clone(),
            pending.request.clone(),
            panel.backend,
            panel.startup_orientation,
        );
        self.browser_create_host.io.then(
            move || {
                let (panel_local_id, request, backend, startup_orientation) = audited;
                [BrowserCreateAuditStatus::Queued, BrowserCreateAuditStatus::Dispatched]
                    .into_iter()
                    .try_for_each(|status| {
                        manifest::record_create_status(&panel_local_id, &request, backend, startup_orientation, status)
                    })
            },
            move |app, audited| app.apply_create_dispatch_audit(&request_id, audited),
        );
        self.browser_create_host.pending.push(pending);
        self.mark_runtime_dirty();
    }

    /// Lets an audited create start, or closes the panel of one whose
    /// dispatch could not be audited.
    /// A create whose dispatch was audited while Horizon exits or switches
    /// sessions is settled as `host_shutdown`: its panel is being torn down.
    fn apply_create_dispatch_audit(&mut self, request_id: &str, audited: Option<std::io::Result<()>>) {
        let retiring = self.browser_host_retiring();
        let pending = &mut self.browser_create_host.pending;
        let Some(index) = pending
            .iter()
            .position(|pending| pending.request.request_id == request_id)
        else {
            return;
        };
        let (code, message) = match audited {
            Some(Ok(())) if retiring => HOST_RETIRING,
            Some(Ok(())) => {
                pending[index].stage = CreateStage::Starting;
                return;
            }
            Some(Err(error)) => {
                tracing::error!(%request_id, %error, "could not audit requested browser creation");
                ("audit_failed", "Horizon refused to create an unaudited browser panel")
            }
            None => ("audit_failed", "Horizon refused to create an unaudited browser panel"),
        };
        let pending = pending.remove(index);
        // A replaced board took the panel with it; a panel of the same id now is another one.
        if !self.browser_create_host.io.applying_stale()
            && let Some(panel) = pending.panel.as_ref()
            && let Some(panel_id) = self.board.panel_id_by_local_id(&panel.local_id)
        {
            self.close_panel(panel_id);
        }
        record_and_complete_failure(
            &mut self.browser_create_host.io,
            &pending.request,
            pending.panel.as_ref(),
            code,
            message,
        );
    }
}

/// Stamps the new panel and assigns it to the requesting agent in one locked
/// transaction, so no other same-workspace agent can claim it in between,
/// then audits the completion and publishes `ready`, on the coordination
/// worker. A step that fails publishes the failure instead, and so does a
/// create whose board retired since it was asked for. Says whether the create
/// was published as ready.
pub(super) fn publish_requested_create(
    live: &BoardFence,
    panel_local_id: &str,
    request: &BrowserCreateRequest,
    workspace: &ManifestWorkspace,
    backend: BackendKind,
    orientation: Option<horizon_core::browser::remote::RemoteOrientation>,
    title: &str,
    ready: &BrowserCreateResult,
) -> bool {
    let fail = |code, message| {
        record_create_failure(panel_local_id, request, backend, orientation);
        complete_result(&BrowserCreateResult::failed(request, code, message));
        false
    };
    if !live.holds() {
        return fail(HOST_RETIRING.0, HOST_RETIRING.1);
    }
    if request.deadline_at_millis < manifest::now_millis() {
        return fail(CREATE_TIMEOUT.0, CREATE_TIMEOUT.1);
    }
    if let Err(error) = manifest::publish_requested_panel(
        panel_local_id,
        request.visible,
        workspace,
        host_identity(&request.actor),
    ) {
        tracing::error!(request_id = %request.request_id, %error, "could not publish requested browser panel");
        return if error.kind() == std::io::ErrorKind::PermissionDenied {
            fail(
                "ownership_failed",
                "Horizon could not assign the new browser panel to the requesting agent",
            )
        } else {
            fail(
                "manifest_update_failed",
                "Horizon could not publish the new browser panel's visibility and workspace",
            )
        };
    }
    stamp_create_title(panel_local_id, request, title);
    if let Err(error) = manifest::record_create_status(
        panel_local_id,
        request,
        backend,
        orientation,
        BrowserCreateAuditStatus::Completed,
    ) {
        tracing::error!(request_id = %request.request_id, %error, "could not complete browser creation audit");
        return fail("audit_failed", "Horizon could not complete the browser creation audit");
    }
    complete_result(ready);
    true
}

/// Put the title of the live page into the manifest of a finished create, when
/// the manifest holds none.
///
/// A page commits its first document before its title arrives, and the title of a
/// page of a browser host crosses a tunnel. The answer of the create carries the
/// title of the page that the person sees, so an agent reads the same line as the
/// person, and a manifest that already names a title keeps it.
fn stamp_create_title(panel_local_id: &str, request: &BrowserCreateRequest, title: &str) {
    if title.is_empty() {
        return;
    }
    if let Err(error) = manifest::update(panel_local_id, |manifest| {
        if manifest.title.is_empty() {
            title.clone_into(&mut manifest.title);
        }
    }) {
        tracing::warn!(request_id = %request.request_id, %error, "could not stamp the title of the new browser panel");
    }
}

/// The options of a create that names no device target: the ordinary browser
/// panel of this Horizon.
///
/// It follows the browser host of the settings, as a panel that a person creates
/// does, because its Chromium then runs on that machine through a tunnel and the
/// widget, the input and the frames stay the ones of a local panel. The rule
/// `new_browser_panel` named the profile and the machine of the page. A duplicate
/// instead keeps the backend and the profile of its source.
pub(in crate::app) fn browser_create_options(
    request: &BrowserCreateRequest,
    mut browser_config: horizon_core::browser::BrowserConfig,
    panel: &NewBrowserPanel,
) -> PanelOptions {
    // The driver stamps the profile of its panel into the manifest, so an agent
    // reads the profile of the panel that it controls. The spawn names it again
    // for a page of the browser host, which keeps the profile of that machine.
    browser_config.profile.clone_from(&panel.profile);
    PanelOptions {
        command: request.url.clone(),
        kind: PanelKind::Browser,
        visible: request.visible,
        browser_config: Some(browser_config),
        // The name of a shared profile is the session identity of the panel, so
        // a later page joins the browser that already holds its sign-ins. A
        // private panel names no profile and gets a folder of its own. A page of
        // the browser host keeps the profile of that machine, so it names none.
        browser_session_id: panel.profile.clone(),
        browser_private: panel.private,
        // The app runs the rule again where the entry of the settings is at hand,
        // and it reads these two fields to do it.
        browser_request: Some(horizon_core::browser::BrowserPanelRequest {
            profile: request.profile.clone(),
            backend: request.backend,
        }),
        remote_session: None,
        follows_browser_host: true,
        ..PanelOptions::default()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::channel;
    use std::time::{Duration, Instant};

    use horizon_core::browser::BrowserPanelState;
    use horizon_core::{Panel, PanelContent, PanelId, PanelOptions, WorkspaceId, browser_actor};

    use super::*;
    use crate::app::test_support::test_app;

    fn agent_options() -> PanelOptions {
        let (command, args) = if cfg!(windows) {
            ("cmd.exe", vec!["/C".to_string(), "exit 0".to_string()])
        } else {
            ("/bin/sh", vec!["-c".to_string(), "exit 0".to_string()])
        };
        PanelOptions {
            command: Some(command.to_string()),
            args,
            kind: PanelKind::Codex,
            ..PanelOptions::default()
        }
    }

    /// A browser panel without a started driver, placed in `workspace`. Its
    /// duplicate decision is refused as unavailable, so a duplicate of it never
    /// opens a second panel.
    fn inert_browser(app: &mut HorizonApp, id: u64, workspace: WorkspaceId) -> String {
        let panel = Panel::from_content(
            PanelId(id),
            workspace,
            PanelKind::Browser,
            PanelContent::Browser(Box::new(BrowserPanelState::inert())),
        );
        let local_id = panel.local_id.clone();
        app.board.panels.push(panel);
        app.board.assign_panel_to_workspace(PanelId(id), workspace);
        local_id
    }

    /// The rule of the host reads the `profile` field of a create, and its words
    /// are the words of the profile-name rule, so an agent learns what to change.
    #[test]
    fn a_create_on_the_wire_with_a_refused_profile_names_the_field_and_the_rule() {
        let host = horizon_core::BrowserHostConfig::default();
        for refused in ["Private", "my profile", "my_profile"] {
            let message = new_browser_panel(Some(refused), None, BackendKind::ChromiumCdp, &host)
                .expect_err("the profile rule refuses this name");
            assert!(message.contains("profile"), "the answer names the field: {message}");
            assert!(message.contains("a-z, 0-9"), "the answer names the rule: {message}");
            assert!(
                message.contains(horizon_browser::PRIVATE_PROFILE),
                "the answer names the reserved word: {message}"
            );
        }
        assert!(
            new_browser_panel(Some("work"), None, BackendKind::ChromiumCdp, &host).is_ok(),
            "a name that the rule accepts passes"
        );
    }

    /// The title of the live page reaches the manifest that the answer of a create
    /// reads, and a manifest that names a title keeps it.
    #[test]
    fn a_finished_create_carries_the_title_of_the_page_that_it_loaded() {
        let request = BrowserCreateRequest::for_tests("agent");
        let panel_local_id = format!("title-stamp-{}", std::process::id());
        manifest::write(&manifest::BrowserManifest {
            panel_local_id: panel_local_id.clone(),
            ..manifest::BrowserManifest::default()
        })
        .expect("a manifest of the create");

        stamp_create_title(&panel_local_id, &request, "Example Domain");
        assert_eq!(
            manifest::read(&panel_local_id).map(|manifest| manifest.title),
            Some("Example Domain".to_string()),
            "the answer of the create names the page that the person sees"
        );

        stamp_create_title(&panel_local_id, &request, "Another title");
        assert_eq!(
            manifest::read(&panel_local_id).map(|manifest| manifest.title),
            Some("Example Domain".to_string()),
            "a manifest that names a title keeps it"
        );

        stamp_create_title(&panel_local_id, &request, "");
        assert_eq!(
            manifest::read(&panel_local_id).map(|manifest| manifest.title),
            Some("Example Domain".to_string()),
            "a page without a title writes nothing"
        );
        manifest::remove_in(&manifest::default_manifest_dir(), &panel_local_id).expect("the manifest goes");
    }

    #[test]
    fn a_create_without_a_device_target_follows_the_browser_host() {
        let request = BrowserCreateRequest::for_tests("agent");
        let host = horizon_core::BrowserHostConfig::default();
        let panel = new_browser_panel(None, None, BackendKind::ChromiumCdp, &host).expect("a create");
        let options = browser_create_options(&request, horizon_core::browser::BrowserConfig::default(), &panel);
        assert_eq!(options.kind, PanelKind::Browser);
        assert!(
            options.follows_browser_host,
            "a create without a device target is the ordinary browser panel, which follows the settings"
        );
        assert!(
            options.remote_session.is_none(),
            "a create without a target is no provider session"
        );
        assert_eq!(
            options.browser_session_id.as_deref(),
            Some(horizon_browser::DEFAULT_PROFILE),
            "a create that names no profile joins the shared default without a host"
        );
        assert_eq!(
            options.browser_request,
            Some(horizon_core::browser::BrowserPanelRequest::default()),
            "the app runs the rule again where the entry of the settings is at hand"
        );
        assert_eq!(options.command, request.url);
    }

    /// A private create names the reserved word, so its panel gets a throwaway
    /// folder, and a named profile reaches the panel as its session id.
    #[test]
    fn a_create_reads_the_profile_that_it_names() {
        let mut request = BrowserCreateRequest::for_tests("agent");
        let host = horizon_core::BrowserHostConfig::default();
        request.profile = Some(horizon_browser::PRIVATE_PROFILE.to_string());
        let private = new_browser_panel(request.profile.as_deref(), None, BackendKind::ChromiumCdp, &host)
            .expect("a private create");
        let options = browser_create_options(&request, horizon_core::browser::BrowserConfig::default(), &private);
        assert!(
            options.browser_session_id.is_none(),
            "a private panel joins no shared profile"
        );
        assert!(options.browser_private, "a private panel keeps nothing");

        request.profile = Some("work".to_string());
        let work = new_browser_panel(request.profile.as_deref(), None, BackendKind::ChromiumCdp, &host)
            .expect("a create of one profile");
        let options = browser_create_options(&request, horizon_core::browser::BrowserConfig::default(), &work);
        assert_eq!(options.browser_session_id.as_deref(), Some("work"));
        assert!(!options.browser_private, "a shared profile keeps its sign-ins");
    }

    #[test]
    #[cfg_attr(windows, ignore = "agent panels launch through a POSIX login shell (#688)")]
    fn a_duplicate_whose_source_manifest_read_waits_starts_only_once_after_it() {
        let (_temp, mut app) = test_app();
        let alpha = app.board.create_workspace("alpha");
        let agent = app.board.create_panel(agent_options(), alpha).expect("agent panel");
        let actor = browser_actor(&app.board.panel(agent).expect("agent").local_id);
        let source = inert_browser(&mut app, 9_300, alpha);
        let path = manifest::manifest_path_for_root(app.host_manifest_root(), &source);
        manifest::write_at(
            &path,
            &manifest::BrowserManifest {
                url: "https://example.test/".into(),
                host: Some(manifest::host_instance().to_string()),
                ..manifest::BrowserManifest::default()
            },
        )
        .expect("live manifest");
        let mut request = BrowserCreateRequest::for_tests("agent");
        request.actor = actor;
        request.host_instance = Some(manifest::host_instance().to_string());
        request.duplicate_from = Some(source);
        let panels_before = app.board.panels.len();

        // The worker waits, as it does for a manifest read whose file another
        // process holds for the read's two-second bound.
        let (release, released) = channel::<()>();
        app.browser_create_host.io.write(move || {
            let _ = released.recv();
        });

        let started = Instant::now();
        app.start_claimed_browser_create(request);
        let elapsed = started.elapsed();
        release.send(()).expect("release the worker");
        assert!(
            elapsed < Duration::from_millis(1500),
            "the frame waited {elapsed:?} for the source manifest read"
        );
        assert!(
            app.browser_create_host.io.busy(),
            "the create waits for its source manifest"
        );
        assert_eq!(
            app.board.panels.len(),
            panels_before,
            "nothing starts before the read lands"
        );
        assert!(app.browser_create_host.pending.is_empty());

        app.settle_browser_host_io();
        assert!(
            !app.browser_create_host.io.busy(),
            "the request is answered once its source manifest was read"
        );
        assert_eq!(
            app.board.panels.len(),
            panels_before,
            "a source without a started driver is refused, so no duplicate panel opens"
        );
        assert!(
            app.browser_create_host.pending.is_empty(),
            "the refused duplicate leaves no create behind"
        );
        // A second round of applies answers nothing more for the request.
        app.settle_browser_host_io();
        assert_eq!(app.board.panels.len(), panels_before);
    }

    #[test]
    #[cfg_attr(windows, ignore = "agent panels launch through a POSIX login shell (#688)")]
    fn a_claim_is_answered_while_another_claim_waits_for_its_source_manifest() {
        let (_temp, mut app) = test_app();
        let alpha = app.board.create_workspace("alpha");
        let agent = app.board.create_panel(agent_options(), alpha).expect("agent panel");
        let actor = browser_actor(&app.board.panel(agent).expect("agent").local_id);
        let source = inert_browser(&mut app, 9_320, alpha);
        let path = manifest::manifest_path_for_root(app.host_manifest_root(), &source);
        manifest::write_at(
            &path,
            &manifest::BrowserManifest {
                url: "https://example.test/".into(),
                host: Some(manifest::host_instance().to_string()),
                ..manifest::BrowserManifest::default()
            },
        )
        .expect("live manifest");
        let mut duplicate = BrowserCreateRequest::for_tests("agent");
        duplicate.actor = actor.clone();
        duplicate.host_instance = Some(manifest::host_instance().to_string());
        duplicate.duplicate_from = Some(source);
        // A create of the same claimed batch that names a target this host does
        // not configure is refused on the frame that applies the claim.
        let mut unrelated = BrowserCreateRequest::for_tests("agent");
        unrelated.actor = actor;
        unrelated.host_instance = Some(manifest::host_instance().to_string());
        unrelated.target = Some("android".into());

        // The worker waits, as it does for the source manifest read.
        let (release, released) = channel::<()>();
        app.browser_create_host.io.write(move || {
            let _ = released.recv();
        });
        let panels = app.board.panels.len();
        app.start_claimed_browser_create(duplicate);
        let started = Instant::now();
        app.start_claimed_browser_create(unrelated);
        assert!(
            started.elapsed() < Duration::from_millis(500),
            "the unrelated claim waited for the source manifest read"
        );
        assert!(
            app.browser_create_host.io.busy(),
            "both claims are answered on the coordination worker"
        );
        assert!(
            app.browser_create_host.pending.is_empty(),
            "neither create waits for a panel of its own"
        );

        release.send(()).expect("release the worker");
        app.settle_browser_host_io();
        assert!(
            !app.browser_create_host.io.busy(),
            "every claim of the batch is answered"
        );
        assert!(
            app.browser_create_host.pending.is_empty(),
            "a second round of applies answers nothing more"
        );
        assert_eq!(app.board.panels.len(), panels, "neither claim opens a panel");
    }
}
