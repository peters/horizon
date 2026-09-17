use horizon_core::PanelOptions;
use horizon_core::browser::manifest::{self, AgentIdentity, BrowserCreateRequest};

use super::HorizonApp;
use super::browser_requests::ActorPanel;

impl HorizonApp {
    pub(super) fn prepare_browser_duplicate(
        &self,
        request: &mut BrowserCreateRequest,
        actor: ActorPanel,
    ) -> Result<Option<PanelOptions>, (&'static str, &'static str)> {
        if request.duplicate_from.is_none() {
            return Ok(None);
        }
        let options = self.duplicate_browser_options(request, actor)?;
        request.url.clone_from(&options.command);
        request.backend = options.browser_config.as_ref().map(|config| config.backend);
        Ok(Some(options))
    }

    pub(super) fn duplicate_browser_options(
        &self,
        request: &BrowserCreateRequest,
        actor: ActorPanel,
    ) -> Result<PanelOptions, (&'static str, &'static str)> {
        let refuse = |code, message| Err((code, message));
        if request.target.is_some() || request.backend.is_some() || request.url.is_some() {
            return refuse(
                "invalid_request",
                "duplication uses the source panel's backend and current URL",
            );
        }
        let source = request.duplicate_from.as_deref().unwrap_or_default();
        let Some(id) = self.board.panel_id_by_local_id(source) else {
            return refuse("panel_not_in_host", "source browser panel is not available");
        };
        let Some(panel) = self.board.panel(id) else {
            return refuse("panel_closed", "source browser panel is not available");
        };
        if panel.workspace_id != actor.workspace_id {
            return refuse(
                "panel_outside_workspace",
                "source browser panel is outside the caller's workspace",
            );
        }
        let Some(browser) = panel.browser() else {
            return refuse("not_browser_panel", "source is not a browser panel");
        };
        if browser.is_remote()
            || !matches!(
                browser.backend(),
                horizon_core::browser::BackendKind::ChromiumCdp | horizon_core::browser::BackendKind::FirefoxBidi
            )
        {
            return refuse(
                "unsupported_backend",
                "duplication currently requires local Chromium or Firefox",
            );
        }
        if self.browser_create_is_pending(id) || !browser.can_duplicate() {
            return refuse("browser_unavailable", "source browser panel is not ready");
        }
        let Some(manifest) = manifest::read(source) else {
            return refuse("browser_unavailable", "source browser panel is not available");
        };
        let identity = AgentIdentity {
            actor: &request.actor,
            host_instance: request.host_instance.as_deref(),
        };
        authorize_duplicate(&manifest, identity, manifest::now_millis())?;
        let mut options = browser
            .duplicate_options()
            .map_err(|_| ("browser_unavailable", "source browser panel is not ready"))?;
        options.visible = request.visible;
        options.size = Some(panel.layout.size);
        Ok(options)
    }
}

fn authorize_duplicate(
    source: &manifest::BrowserManifest,
    identity: AgentIdentity<'_>,
    now: i64,
) -> Result<(), (&'static str, &'static str)> {
    if !source.permits(identity) || source.live_owner(now).is_none_or(|owner| owner.name != identity.actor) {
        return Err(("ownership_changed", "source browser ownership changed"));
    }
    if source.user_is_active(now) || source.handoff_pending().is_some() {
        return Err(("user_active", "wait until the user hands the source browser back"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use manifest::{BrowserManifest, ManifestHandoff, ManifestOwner, ManifestWorkspace};

    fn source() -> BrowserManifest {
        BrowserManifest {
            host: Some("host".into()),
            workspace: Some(ManifestWorkspace {
                host_instance: "host".into(),
                local_id: "workspace".into(),
                actors: vec!["horizon:agent".into()],
            }),
            owner: Some(ManifestOwner {
                name: "horizon:agent".into(),
                tty: None,
                updated_at: 100,
            }),
            ..BrowserManifest::default()
        }
    }

    #[test]
    fn duplicate_requires_current_workspace_host_and_ownership() {
        let mut source = source();
        let identity = AgentIdentity {
            actor: "horizon:agent",
            host_instance: Some("host"),
        };
        assert!(authorize_duplicate(&source, identity, 100).is_ok());
        assert!(
            authorize_duplicate(
                &source,
                AgentIdentity {
                    host_instance: Some("different-host"),
                    ..identity
                },
                100
            )
            .is_err()
        );
        assert!(
            authorize_duplicate(
                &source,
                AgentIdentity {
                    actor: "horizon:other",
                    ..identity
                },
                100
            )
            .is_err()
        );
        source.owner = None;
        assert!(authorize_duplicate(&source, identity, 100).is_err());
    }

    #[test]
    fn duplicate_waits_for_user_activity_and_handoff_to_end() {
        let mut source = source();
        let identity = AgentIdentity {
            actor: "horizon:agent",
            host_instance: Some("host"),
        };
        source.user_active = true;
        source.user_active_at = 100;
        assert_eq!(
            authorize_duplicate(&source, identity, 100).expect_err("user active").0,
            "user_active"
        );
        source.user_active = false;
        source.handoff = Some(ManifestHandoff {
            request_id: "handoff".into(),
            reason: "fixture".into(),
            requested_at: 100,
            done: false,
        });
        assert_eq!(
            authorize_duplicate(&source, identity, 100).expect_err("handoff").0,
            "user_active"
        );
        source.handoff.as_mut().expect("handoff").done = true;
        assert!(authorize_duplicate(&source, identity, 100).is_ok());
    }

    #[test]
    fn board_workspace_is_rechecked_before_reading_a_source_profile() {
        use horizon_core::{Panel, PanelContent, PanelId, PanelKind};
        let (_temp, mut app) = crate::app::test_support::test_app();
        let workspace = app.board.create_workspace("source");
        let other = app.board.create_workspace("other");
        let panel = Panel::from_content(
            PanelId(9001),
            workspace,
            PanelKind::Browser,
            PanelContent::Browser(Box::new(horizon_core::browser::BrowserPanelState::inert())),
        );
        let mut request = BrowserCreateRequest::for_tests("agent");
        request.duplicate_from = Some(panel.local_id.clone());
        app.board.panels.push(panel);
        let actor = ActorPanel {
            panel_id: PanelId(9002),
            workspace_id: other,
        };
        assert_eq!(
            app.duplicate_browser_options(&request, actor)
                .err()
                .expect("different workspace")
                .0,
            "panel_outside_workspace"
        );
        request.url = Some("https://example.test/override".into());
        assert_eq!(
            app.duplicate_browser_options(&request, actor)
                .err()
                .expect("URL override")
                .0,
            "invalid_request"
        );
    }
}
