//! Dispatch only through the configured core controllers and retained SSH trust.

use super::{
    Error, Intent, input,
    storage::{self, Context, Receipt},
};
use horizon_core::{
    HorizonHome, RuntimeState, SessionStore,
    cloud_run::CloudProvider,
    remote_git_setup::{self as git, ConfiguredRemoteGitSetupRequest, RemoteGitCredentialMode},
    remote_github_credential::RepositoryPat,
    remote_panel_attachment::{self as attachment, ConfiguredRemotePanelAttachRequest, RemotePanelTerminalSize},
    remote_ssh_identity::RemoteSshIdentityStore,
    remote_worker_status::{self as panel, ConfiguredRemotePanelStatusRequest},
    remote_workspace_setup::{
        self as setup, RemoteWorkspaceSetupConsent as Consent, RemoteWorkspaceSetupDraft, RemoteWorkspaceSetupLocator,
    },
};
use serde_json::{Value, json};
use std::{path::Path, time::Duration};

pub(super) fn create(root: &Path, intent: Intent) -> Result<Value, Error> {
    intent.config.validate().map_err(|_| Error::Input)?;
    if !matches!(
        intent.target.provider,
        CloudProvider::LocalDocker | CloudProvider::Azure
    ) {
        return Err(Error::Input);
    }
    // Validate intent before creating any session or directory.
    let provisional = HorizonHome::from_root(root.join("home"));
    preview(&provisional, &uuid::Uuid::new_v4().to_string(), &intent)?;
    let root = storage::create_root(root)?;
    let lock = storage::lock(&root)?;
    storage::create_root(&root.join("home"))?;
    let home = HorizonHome::from_root(root.join("home"));
    let sessions = SessionStore::new(home.clone(), root.join("profile.json"));
    let session = sessions
        .create_session_from_runtime(RuntimeState::default())
        .map_err(|_| Error::Storage)?;
    let prepared = preview(&home, &session.session_id, &intent)?;
    let panel = prepared
        .spec()
        .panels
        .first()
        .ok_or(Error::Input)?
        .panel_local_id
        .clone();
    let receipt = Receipt {
        version: 1,
        root: root.clone(),
        session: session.session_id,
        workspace: prepared.locator().workspace_local_id.clone(),
        panel,
        intent,
    };
    storage::write_new(
        &root.join("receipt.json"),
        &serde_json::to_vec_pretty(&receipt).map_err(|_| Error::Storage)?,
    )?;
    let context = Context::new(&root, receipt, lock);
    let consent = match context.receipt.intent.target.provider {
        CloudProvider::LocalDocker => Consent::LocalDocker {
            image: context.receipt.intent.target.image.clone(),
        },
        CloudProvider::Azure => Consent::Azure {
            image: context.receipt.intent.target.image.clone(),
            profile: prepared.azure_profile().ok_or(Error::Input)?.clone(),
        },
        CloudProvider::RunPod => return Err(Error::Input),
    };
    context.claim("create")?;
    let attempt = setup::submit_configured_remote_workspace(
        &context.home,
        &context.receipt.intent.config,
        &context.receipt.session,
        prepared,
        consent,
    );
    attempt.result.map_err(|error| Error::Remote(error.to_string()))?;
    Ok(json!({"task": context.receipt.workspace, "status": "worker_observed", "directory": root}))
}

fn preview(home: &HorizonHome, owner: &str, intent: &Intent) -> Result<setup::PreparedRemoteWorkspaceSetup, Error> {
    setup::preview_configured_remote_workspace(
        home,
        &intent.config,
        owner,
        RemoteWorkspaceSetupDraft {
            target: intent.target.clone(),
            repository: intent.repository.clone(),
            working_directory: intent.working_directory.clone(),
            command: intent.command.clone(),
            panel_directory: Some(intent.working_directory.clone()),
            retain_until_millis: intent.setup_expires_at_millis,
            network_volume: None,
        },
    )
    .map_err(|_| Error::Input)
}

pub(super) fn check(context: &Context) -> Result<Value, Error> {
    use setup::ConfiguredWorkspaceSetupObservation as Observation;
    let receipt = &context.receipt;
    let locator = RemoteWorkspaceSetupLocator::new(&context.home, &receipt.session, &receipt.workspace)
        .map_err(|_| Error::Storage)?;
    let observed = setup::check_configured_remote_workspace_setup(
        &context.home,
        &receipt.intent.config,
        &receipt.session,
        &locator,
    )
    .map_err(|error| Error::Remote(error.to_string()))?;
    let status = match observed {
        Observation::Missing => return Ok(json!({"setup": "missing", "panel": receipt.panel})),
        Observation::SavedOnly(_) => "saved_only",
        Observation::Interrupted(_) => "interrupted",
        Observation::Observed(_) => "observed",
    };
    let saved = context.saved()?;
    Ok(
        json!({"setup": status, "phase": format!("{:?}", saved.environment_summary().saved_phase),
        "panel": receipt.panel, "panels": saved.state().spec.panels.iter().map(|panel| panel.panel_local_id.clone()).collect::<Vec<_>>(), "repository": receipt.intent.repository, "target": receipt.intent.target}),
    )
}

pub(super) fn git(context: &Context, install: bool, observe: bool) -> Result<Value, Error> {
    let store = context.store()?;
    let expected = context.saved()?.environment_summary();
    let config = &context.receipt.intent.config;
    let identities = RemoteSshIdentityStore::new(&context.home);
    let request = ConfiguredRemoteGitSetupRequest {
        expected: &expected,
        client_session_id: &context.receipt.session,
    };
    if observe {
        let result = git::inspect_configured_remote_git_setup(&store, &identities, config, request)
            .map_err(|error| Error::Remote(error.to_string()))?;
        return Ok(
            json!({"state": format!("{:?}", result.state), "reason": result.reason.map(|reason| format!("{reason:?}"))}),
        );
    }
    let mode = if install {
        RemoteGitCredentialMode::InstallFirst
    } else {
        RemoteGitCredentialMode::UseInstalled
    };
    let prepared = git::prepare_configured_remote_git_setup(&store, config, request, mode)
        .map_err(|error| Error::Remote(error.to_string()))?;
    let secret = zeroize::Zeroizing::new(if install { input(16_384)? } else { String::new() });
    let token = if install {
        Some(RepositoryPat::new(secret.trim()).map_err(|_| Error::Input)?)
    } else {
        None
    };
    context.claim("git")?;
    let result =
        git::submit_configured_remote_git_setup(&store, &identities, config, request, prepared, token.as_ref())
            .map_err(|error| Error::Remote(error.to_string()))?;
    Ok(json!({"submission": format!("{:?}", result.submission)}))
}

pub(super) fn panel(context: &Context, start: bool) -> Result<Value, Error> {
    let store = context.store()?;
    let expected = context.saved()?.environment_summary();
    let config = &context.receipt.intent.config;
    let identities = RemoteSshIdentityStore::new(&context.home);
    let request = ConfiguredRemotePanelStatusRequest {
        expected: &expected,
        client_session_id: &context.receipt.session,
        panel_id: &context.receipt.panel,
    };
    let result = if start {
        let prepared = panel::prepare_configured_remote_git_start(&store, config, request)
            .map_err(|error| Error::Remote(error.to_string()))?;
        context.claim(&format!("start-{}", context.receipt.panel))?;
        panel::start_configured_remote_git_shell(&store, &identities, config, request, prepared)
            .map_err(|error| Error::Remote(error.to_string()))?
    } else {
        panel::inspect_configured_remote_panel(&store, &identities, config, request)
            .map_err(|error| Error::Remote(error.to_string()))?
            .status
    };
    Ok(json!({"status": format!("{result:?}")}))
}

pub(super) fn terminal(context: &Context) -> Result<Value, Error> {
    let store = context.store()?;
    let expected = context.saved()?.environment_summary();
    let attempt = attachment::attach_configured_remote_panel(
        &store,
        &RemoteSshIdentityStore::new(&context.home),
        &context.receipt.intent.config,
        ConfiguredRemotePanelAttachRequest {
            expected: &expected,
            client_session_id: &context.receipt.session,
            panel_id: &context.receipt.panel,
            terminal: RemotePanelTerminalSize {
                rows: 40,
                cols: 160,
                cell_width: 8,
                cell_height: 16,
                scrollback_limit: 1000,
                window_id: 0,
                kitty_keyboard: false,
            },
        },
    )
    .map_err(|error| Error::Remote(error.to_string()))?;
    let terminal = attempt
        .into_terminal(&store)
        .map_err(|error| Error::Remote(error.to_string()))?;
    std::thread::sleep(Duration::from_secs(2));
    Ok(
        json!({"terminal": terminal.last_lines_text(80), "observed_at": time::OffsetDateTime::now_utc().unix_timestamp()}),
    )
}
