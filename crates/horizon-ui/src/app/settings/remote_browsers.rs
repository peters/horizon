//! Remote browsers tab: provider readiness, credential entry and the portable
//! profile. Values typed here go straight into the credential workbench and
//! are never written to the YAML buffer, the config file, the process
//! environment or an exported profile.

use std::collections::BTreeMap;
use std::path::Path;

use egui::Ui;
use horizon_core::Config;
use horizon_core::browser::remote::{
    CredentialReference, CredentialStoreKind, RemoteAuthentication, RemoteProviderProfile,
};
use horizon_core::browser::remote_profile::{self, PORTABLE_PROFILE_FILE_NAME};
use horizon_core::remote_browser_credential::{
    CredentialReadiness, CredentialState, CredentialWorkbench, KeychainState, NoticeKind, RemoteCredentialError,
    WorkbenchNotice, credential_destination,
};
use zeroize::{Zeroize as _, Zeroizing};

use crate::theme;

const MAX_NOTICES: usize = 8;

/// Identity of one text buffer: the row plus the exact destination a value
/// typed there would be sent to. The YAML tab can change a provider's
/// endpoint or a binding's store and slot while a draft is pending; a draft
/// belongs to the destination it was typed for and is scrubbed, never
/// rebound, when that destination changes.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
struct InputKey {
    provider: String,
    reference: String,
    destination: String,
}

impl InputKey {
    fn new(provider: &str, profile: &RemoteProviderProfile, reference: &CredentialReference) -> Self {
        Self {
            provider: provider.to_string(),
            reference: reference.as_str().to_string(),
            destination: credential_destination(profile, reference),
        }
    }

    fn matches_notice(&self, notice: &WorkbenchNotice) -> bool {
        notice.provider == self.provider
            && notice.reference.as_str() == self.reference
            && notice.destination == self.destination
    }

    fn same_row(&self, other: &Self) -> bool {
        self.provider == other.provider && self.reference == other.reference
    }
}

/// Per-reference text buffers for the tab. Dropped with the settings editor,
/// and every buffer is overwritten before it goes.
#[derive(Default)]
pub(super) struct CredentialInputs {
    values: BTreeMap<InputKey, String>,
    notices: Vec<WorkbenchNotice>,
}

impl CredentialInputs {
    /// The buffer for `key`. A draft the same row typed for another
    /// destination is scrubbed first, so it can never be submitted to the
    /// new one.
    fn buffer(&mut self, key: &InputKey) -> &mut String {
        let stale: Vec<InputKey> = self
            .values
            .keys()
            .filter(|existing| existing.same_row(key) && *existing != key)
            .cloned()
            .collect();
        for old_key in stale {
            if let Some(mut text) = self.values.remove(&old_key) {
                text.zeroize();
            }
        }
        self.values.entry(key.clone()).or_default()
    }

    /// Move a typed value out of its buffer into a zeroizing copy; the
    /// buffer is scrubbed at once and the copy is wiped when it drops.
    fn take(&mut self, key: &InputKey) -> Zeroizing<Vec<u8>> {
        let mut text = self.values.remove(key).unwrap_or_default();
        let bytes = Zeroizing::new(text.as_bytes().to_vec());
        text.zeroize();
        bytes
    }

    fn absorb(&mut self, notices: Vec<WorkbenchNotice>) {
        self.notices.extend(notices);
        let overflow = self.notices.len().saturating_sub(MAX_NOTICES);
        self.notices.drain(..overflow);
    }

    /// Scrub every draft whose row or destination is no longer in the parsed
    /// configuration, so a provider removed or rebound in the YAML tab (or
    /// by Reset) never keeps a secret that could reappear later.
    fn retain_current(&mut self, config: &Config) {
        let current: std::collections::BTreeSet<InputKey> = config
            .browser
            .remote
            .providers
            .iter()
            .flat_map(|(name, profile)| {
                profile
                    .authentication
                    .references()
                    .into_iter()
                    .map(|reference| InputKey::new(name, profile, reference))
                    .collect::<Vec<_>>()
            })
            .collect();
        let stale: Vec<InputKey> = self
            .values
            .keys()
            .filter(|key| !current.contains(*key))
            .cloned()
            .collect();
        for key in stale {
            if let Some(mut text) = self.values.remove(&key) {
                text.zeroize();
            }
        }
    }

    /// The latest notice for exactly this row and destination.
    fn last_notice(&self, key: &InputKey) -> Option<&WorkbenchNotice> {
        self.notices.iter().rev().find(|notice| key.matches_notice(notice))
    }
}

impl Drop for CredentialInputs {
    fn drop(&mut self) {
        for value in self.values.values_mut() {
            // Wipes the whole backing allocation, not only the current
            // length, so an edited-down secret leaves no suffix behind.
            value.zeroize();
        }
    }
}

/// Render the tab. Never returns a config change: this tab edits no YAML.
/// The portable-profile row: the path the user edits and the outcome of the
/// last export or import. Outcomes name counts and the path, never a value.
pub(super) struct PortableProfilePanel {
    path: String,
    notice: Option<(String, bool)>,
}

impl PortableProfilePanel {
    pub(super) fn new(config_path: &Path) -> Self {
        Self {
            path: default_portable_profile_path(config_path),
            notice: None,
        }
    }

    /// Write the shareable definition (bindings stripped) to the path.
    fn export(&mut self, config: &Config) {
        let path = Path::new(self.path.trim());
        self.notice = Some(
            match remote_profile::write_portable_profile(path, &config.browser.remote) {
                Ok(()) => {
                    let remote = &config.browser.remote;
                    (
                        format!(
                            "Exported {} provider(s) and {} target(s) to {} without credentials",
                            remote.providers.len(),
                            remote.targets.len(),
                            path.display()
                        ),
                        false,
                    )
                }
                Err(error) => (format!("Export failed: {error}"), true),
            },
        );
    }

    /// Merge the profile at the path into the editing configuration. Returns
    /// whether the configuration changed; the caller re-serialises it into
    /// the YAML buffer, and Save writes it to disk.
    fn import(&mut self, config: &mut Config) -> bool {
        let path = Path::new(self.path.trim());
        let outcome = remote_profile::read_portable_profile(path)
            .and_then(|document| remote_profile::import_portable(&mut config.browser.remote, &document));
        match outcome {
            Ok(summary) => {
                self.notice = Some((
                    format!(
                        "Imported {}: {}. Enter this computer's credentials below, then Save.",
                        path.display(),
                        remote_profile::summary_line(&summary)
                    ),
                    false,
                ));
                true
            }
            Err(error) => {
                self.notice = Some((format!("Import failed: {error}"), true));
                false
            }
        }
    }
}

/// Where the portable profile lives unless the user types another path:
/// next to the configuration file.
pub(super) fn default_portable_profile_path(config_path: &Path) -> String {
    config_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
        .join(PORTABLE_PROFILE_FILE_NAME)
        .display()
        .to_string()
}

pub(super) fn render(
    ui: &mut Ui,
    config: &mut Config,
    workbench: &mut CredentialWorkbench,
    inputs: &mut CredentialInputs,
    portable: &mut PortableProfilePanel,
) -> bool {
    workbench.poll();
    inputs.absorb(workbench.take_notices());
    inputs.retain_current(config);
    render_stores_section(ui, workbench);
    let changed = render_portable_section(ui, config, portable);
    let providers = &config.browser.remote.providers;
    if providers.is_empty() {
        super::section_heading(ui, "Providers");
        super::section_card(ui, |ui| {
            super::dim_label(
                ui,
                "No remote device services are configured. Import a portable profile above or add a \
                 browser.remote section in the YAML tab; see docs/architecture/remote-browser-sessions.md \
                 for the schema.",
            );
        });
        return changed;
    }
    for (name, profile) in providers {
        render_provider(ui, config, name, profile, workbench, inputs);
    }
    changed
}

fn render_portable_section(ui: &mut Ui, config: &mut Config, portable: &mut PortableProfilePanel) -> bool {
    super::section_heading(ui, "Portable profile");
    let mut changed = false;
    super::section_card(ui, |ui| {
        super::dim_label(
            ui,
            "A portable profile carries providers, targets, limits and credential references so another \
             computer can use the same targets. It never contains credential values, bindings or local \
             paths; each computer enters its own credentials.",
        );
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("File").color(theme::FG_SOFT()).size(12.0));
            ui.add(
                egui::TextEdit::singleline(&mut portable.path)
                    .desired_width(ui.available_width() - 160.0)
                    .font(egui::TextStyle::Monospace),
            );
            let has_path = !portable.path.trim().is_empty();
            let has_remote = !config.browser.remote.is_empty();
            if ui
                .add_enabled(has_path && has_remote, egui::Button::new("Export"))
                .clicked()
            {
                portable.export(config);
            }
            if ui.add_enabled(has_path, egui::Button::new("Import")).clicked() {
                changed = portable.import(config);
            }
        });
        if let Some((text, is_error)) = &portable.notice {
            let color = if *is_error {
                theme::PALETTE_RED()
            } else {
                theme::PALETTE_GREEN()
            };
            ui.label(egui::RichText::new(text).color(color).size(12.0));
        }
    });
    changed
}

fn render_stores_section(ui: &mut Ui, workbench: &mut CredentialWorkbench) {
    super::section_heading(ui, "Credential stores");
    super::section_card(ui, |ui| {
        let (label, color) = match workbench.keychain_state() {
            KeychainState::Opening => ("OS credential store: checking".to_string(), theme::FG_DIM()),
            KeychainState::Available => ("OS credential store: available".to_string(), theme::PALETTE_GREEN()),
            KeychainState::Unavailable(error) => (
                format!("OS credential store: unavailable ({error})"),
                theme::PALETTE_YELLOW(),
            ),
        };
        ui.label(egui::RichText::new(label).color(color).size(12.0));
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let count = workbench.session_value_count();
            ui.label(
                egui::RichText::new(format!("Session-only values held in memory: {count}"))
                    .color(theme::FG_SOFT())
                    .size(12.0),
            );
            if count > 0 && ui.small_button("Clear all").clicked() {
                workbench.clear_session();
            }
        });
        super::dim_label(
            ui,
            "Session-only values are discarded when Horizon exits. OS-store values persist on this computer only \
             and are never exported.",
        );
    });
}

fn render_provider(
    ui: &mut Ui,
    config: &Config,
    name: &str,
    profile: &RemoteProviderProfile,
    workbench: &mut CredentialWorkbench,
    inputs: &mut CredentialInputs,
) {
    super::section_heading(ui, name);
    super::section_card(ui, |ui| {
        let targets: Vec<&String> = config
            .browser
            .remote
            .targets
            .iter()
            .filter(|(_, target)| target.provider == name)
            .map(|(target_name, _)| target_name)
            .collect();
        super::dim_label(
            ui,
            &format!(
                "{} · {} · limits {} session(s), {} s allocation, {} s idle, {} s max · targets: {}",
                profile.endpoint.as_str(),
                authentication_label(&profile.authentication),
                profile.limits.max_sessions,
                profile.limits.allocation_timeout_seconds,
                profile.limits.idle_release_seconds,
                profile.limits.max_session_seconds,
                if targets.is_empty() {
                    "none".to_string()
                } else {
                    targets
                        .iter()
                        .map(|target| target.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                }
            ),
        );
        if profile.endpoint.is_loopback_http() {
            ui.label(
                egui::RichText::new("Plain HTTP loopback endpoint: only for a local test grid.")
                    .color(theme::PALETTE_YELLOW())
                    .size(11.0),
            );
        }
        let readiness = workbench.readiness(profile);
        if readiness.is_empty() {
            super::dim_label(ui, "No authentication configured for this provider.");
            return;
        }
        ui.add_space(8.0);
        for entry in readiness {
            render_reference(ui, name, profile, &entry, workbench, inputs);
            ui.add_space(6.0);
        }
    });
}

fn render_reference(
    ui: &mut Ui,
    provider: &str,
    profile: &RemoteProviderProfile,
    entry: &CredentialReadiness,
    workbench: &mut CredentialWorkbench,
    inputs: &mut CredentialInputs,
) {
    let (reference, store, state) = (&entry.reference, entry.store, entry.state);
    let bound = profile.credential_bindings.contains_key(reference);
    let key = InputKey::new(provider, profile, reference);
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(reference.as_str())
                .color(theme::FG())
                .size(12.0)
                .strong(),
        );
        ui.label(
            egui::RichText::new(store_label(store, bound))
                .color(theme::FG_DIM())
                .size(11.0),
        );
        let (state_text, color) = state_badge(state);
        ui.label(egui::RichText::new(state_text).color(color).size(11.0));
    });
    if !bound {
        super::dim_label(
            ui,
            "Not bound on this computer. Add a credential_bindings entry for this reference in the YAML tab.",
        );
        return;
    }
    ui.horizontal(|ui| {
        password_field(ui, inputs.buffer(&key), provider, reference);
        let has_text = !inputs.buffer(&key).is_empty();
        match store {
            CredentialStoreKind::Session => {
                if ui
                    .add_enabled(has_text, egui::Button::new("Set for this session"))
                    .clicked()
                {
                    // Every outcome, including a synchronous refusal, arrives
                    // as a workbench notice for this row.
                    let value = inputs.take(&key);
                    let _ = workbench.set_session_value(provider, profile, reference, &value);
                }
            }
            CredentialStoreKind::OsKeychain => {
                let available = workbench.keychain_state() == &KeychainState::Available;
                if ui
                    .add_enabled(has_text && available, egui::Button::new("Save to OS store"))
                    .clicked()
                {
                    let value = inputs.take(&key);
                    let _ = workbench.store_in_keychain(provider, profile, reference, &value);
                }
            }
        }
        if state == CredentialState::Present && ui.button("Delete").clicked() {
            let _ = workbench.delete(provider, profile, reference);
        }
    });
    if let Some(notice) = inputs.last_notice(&key)
        && notice_agrees_with(notice, state)
    {
        let (text, color) = notice_line(notice.kind, notice.error.as_ref());
        ui.label(egui::RichText::new(text).color(color).size(11.0));
    }
}

fn password_field(ui: &mut Ui, buffer: &mut String, provider: &str, reference: &CredentialReference) {
    let mut output = egui::TextEdit::singleline(buffer)
        .password(true)
        .char_limit(16_384)
        .hint_text("value")
        .desired_width(220.0)
        .id_salt(("remote-browser-credential", provider, reference.as_str()))
        .show(ui);
    // Password masking disables clipboard copy, but egui still records plaintext undo.
    output.state.clear_undoer();
    output.state.store(ui.ctx(), output.response.id);
}

fn authentication_label(authentication: &RemoteAuthentication) -> &'static str {
    match authentication {
        RemoteAuthentication::None {} => "no authentication",
        RemoteAuthentication::Basic { .. } => "basic authentication",
        RemoteAuthentication::Bearer { .. } => "bearer token",
    }
}

fn store_label(store: CredentialStoreKind, bound: bool) -> &'static str {
    match (store, bound) {
        (_, false) => "unbound",
        (CredentialStoreKind::Session, true) => "session-only",
        (CredentialStoreKind::OsKeychain, true) => "OS store",
    }
}

fn state_badge(state: CredentialState) -> (&'static str, egui::Color32) {
    match state {
        CredentialState::Present => ("present", theme::PALETTE_GREEN()),
        CredentialState::Missing => ("missing", theme::PALETTE_YELLOW()),
        CredentialState::Locked => ("locked", theme::PALETTE_RED()),
        CredentialState::StoreUnavailable => ("store unavailable", theme::PALETTE_RED()),
        CredentialState::Checking => ("checking", theme::FG_DIM()),
    }
}

/// A successful notice is shown only while the row's state still reflects
/// it: an item deleted through another row that shares the OS address turns
/// this row `missing`, and a stale "Saved" would contradict that. Failures
/// are always shown.
fn notice_agrees_with(notice: &WorkbenchNotice, state: CredentialState) -> bool {
    if notice.error.is_some() {
        return true;
    }
    match notice.kind {
        NoticeKind::SessionValueSet | NoticeKind::StoredInKeychain => state == CredentialState::Present,
        NoticeKind::SessionValueCleared | NoticeKind::DeletedFromKeychain => state == CredentialState::Missing,
    }
}

fn notice_line(kind: NoticeKind, error: Option<&RemoteCredentialError>) -> (String, egui::Color32) {
    match (kind, error) {
        (NoticeKind::SessionValueSet, None) => ("Held for this session.".to_string(), theme::PALETTE_GREEN()),
        (NoticeKind::SessionValueCleared, None) => ("Session value cleared.".to_string(), theme::FG_DIM()),
        (NoticeKind::StoredInKeychain, None) => {
            ("Saved to the OS credential store.".to_string(), theme::PALETTE_GREEN())
        }
        (NoticeKind::DeletedFromKeychain, None) => {
            ("Deleted from the OS credential store.".to_string(), theme::FG_DIM())
        }
        (NoticeKind::SessionValueSet | NoticeKind::StoredInKeychain, Some(error)) => {
            (format!("Not saved: {error}."), theme::PALETTE_RED())
        }
        (NoticeKind::SessionValueCleared | NoticeKind::DeletedFromKeychain, Some(error)) => {
            (format!("Not deleted: {error}."), theme::PALETTE_RED())
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use horizon_core::browser::remote::{ControlEndpoint, CredentialBinding, RemoteAdapterKind, RemoteSessionLimits};

    use super::*;

    fn profile(endpoint: &str, slot: &str) -> RemoteProviderProfile {
        let mut bindings = BTreeMap::new();
        bindings.insert(
            CredentialReference::from("key"),
            CredentialBinding {
                store: CredentialStoreKind::OsKeychain,
                slot: Some(slot.to_string()),
            },
        );
        RemoteProviderProfile {
            adapter: RemoteAdapterKind::Webdriver,
            endpoint: ControlEndpoint::parse(endpoint).expect("endpoint"),
            authentication: RemoteAuthentication::Bearer {
                token_ref: CredentialReference::from("key"),
            },
            credential_bindings: bindings,
            limits: RemoteSessionLimits::default(),
        }
    }

    fn remote_with(provider_name: &str, endpoint: &str) -> horizon_core::browser::remote::RemoteBrowserConfig {
        let mut providers = BTreeMap::new();
        providers.insert(provider_name.to_string(), profile(endpoint, "remote-browser/grid/key"));
        horizon_core::browser::remote::RemoteBrowserConfig {
            providers,
            targets: BTreeMap::new(),
        }
    }

    #[test]
    fn the_portable_panel_imports_into_the_editing_config_and_exports_without_bindings() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config_path = dir.path().join("config.yaml");
        let mut panel = PortableProfilePanel::new(&config_path);
        assert_eq!(
            panel.path,
            dir.path().join(PORTABLE_PROFILE_FILE_NAME).display().to_string()
        );

        // Export from the first computer's editing config.
        let mut first = Config::default();
        first.browser.remote = remote_with("grid", "https://grid.example.net/wd/hub");
        panel.export(&first);
        let (notice, is_error) = panel.notice.clone().expect("notice");
        assert!(!is_error, "{notice}");
        assert!(
            notice.starts_with("Exported 1 provider(s) and 0 target(s) to "),
            "{notice}"
        );
        let document = std::fs::read_to_string(&panel.path).expect("profile");
        assert!(!document.contains("credential_bindings"), "{document}");

        // Import on the second computer: the config changes, the notice
        // names counts, and no binding arrives.
        let mut second = Config::default();
        assert!(panel.import(&mut second));
        let (notice, is_error) = panel.notice.clone().expect("notice");
        assert!(!is_error, "{notice}");
        assert!(notice.contains("added 1 provider(s) and 0 target(s)"), "{notice}");
        assert!(second.browser.remote.providers["grid"].credential_bindings.is_empty());

        // A conflicting import changes nothing and reports the provider only.
        let mut trusted = Config::default();
        trusted.browser.remote = remote_with("grid", "https://elsewhere.example.net/wd/hub");
        let before = trusted.browser.remote.clone();
        assert!(!panel.import(&mut trusted));
        let (notice, is_error) = panel.notice.clone().expect("notice");
        assert!(is_error, "{notice}");
        assert!(notice.contains("different endpoint"), "{notice}");
        assert!(!notice.contains("elsewhere.example.net"), "{notice}");
        assert_eq!(trusted.browser.remote, before);

        // A missing file is an error that names the path, not a panic.
        panel.path = dir.path().join("absent.yaml").display().to_string();
        assert!(!panel.import(&mut second));
        let (notice, is_error) = panel.notice.clone().expect("notice");
        assert!(is_error && notice.contains("absent.yaml"), "{notice}");
    }

    #[test]
    fn the_portable_section_renders_for_an_empty_configuration() {
        use crate::test_egui::DiscardTextures;

        let dir = tempfile::tempdir().expect("tempdir");
        let mut panel = PortableProfilePanel::new(&dir.path().join("config.yaml"));
        let mut config = Config::default();
        let ctx = egui::Context::default();
        let mut changed = None;
        let _ = ctx
            .run_ui(egui::RawInput::default(), |ui| {
                egui::CentralPanel::default().show(ui, |ui| {
                    changed = Some(render_portable_section(ui, &mut config, &mut panel));
                });
            })
            .discard_textures();
        assert_eq!(changed, Some(false));
        assert!(panel.notice.is_none());
    }

    #[test]
    fn a_draft_is_scrubbed_when_its_row_points_at_another_destination() {
        let reference = CredentialReference::from("key");
        let mut inputs = CredentialInputs::default();
        let first = InputKey::new("grid", &profile("https://grid.example.net/wd/hub", "a"), &reference);
        inputs.buffer(&first).push_str("typed-for-grid");
        assert_eq!(
            inputs.buffer(&first),
            "typed-for-grid",
            "same destination keeps the draft"
        );

        let moved = InputKey::new("grid", &profile("https://other.example.net/wd/hub", "a"), &reference);
        assert!(inputs.buffer(&moved).is_empty(), "a new endpoint starts empty");
        assert!(
            inputs.values.keys().all(|key| key.destination != first.destination),
            "the old destination's draft is gone"
        );
        assert!(inputs.take(&first).is_empty(), "and can never be submitted");

        let rebound = InputKey::new("grid", &profile("https://other.example.net/wd/hub", "b"), &reference);
        inputs.buffer(&moved).push_str("typed-for-other");
        assert!(inputs.buffer(&rebound).is_empty(), "a new slot starts empty too");
        assert!(inputs.take(&moved).is_empty());

        // A provider removed from the configuration loses its draft too.
        inputs.buffer(&rebound).push_str("typed-then-removed");
        let mut config = Config::default();
        config.browser.remote.providers.insert(
            "elsewhere".to_string(),
            profile("https://third.example.net/wd/hub", "c"),
        );
        inputs.retain_current(&config);
        assert!(
            inputs.values.is_empty(),
            "drafts for rows absent from the config are scrubbed"
        );
        assert!(inputs.take(&rebound).is_empty());
    }

    #[test]
    fn a_successful_notice_is_shown_only_while_the_row_state_agrees() {
        let reference = CredentialReference::from("key");
        let profile = profile("https://grid.example.net/wd/hub", "a");
        let saved = WorkbenchNotice {
            provider: "grid".to_string(),
            reference: reference.clone(),
            destination: credential_destination(&profile, &reference),
            kind: NoticeKind::StoredInKeychain,
            error: None,
        };
        assert!(notice_agrees_with(&saved, CredentialState::Present));
        assert!(
            !notice_agrees_with(&saved, CredentialState::Missing),
            "a delete through an aliased row hides the stale Saved"
        );
        let failed = WorkbenchNotice {
            error: Some(RemoteCredentialError::Locked),
            ..saved.clone()
        };
        assert!(
            notice_agrees_with(&failed, CredentialState::Missing),
            "failures always show"
        );
        let cleared = WorkbenchNotice {
            kind: NoticeKind::SessionValueCleared,
            ..saved
        };
        assert!(notice_agrees_with(&cleared, CredentialState::Missing));
        assert!(!notice_agrees_with(&cleared, CredentialState::Present));
    }

    #[test]
    fn drafts_are_wiped_including_spare_capacity() {
        let reference = CredentialReference::from("key");
        let mut inputs = CredentialInputs::default();
        let key = InputKey::new("grid", &profile("https://grid.example.net/wd/hub", "a"), &reference);
        let buffer = inputs.buffer(&key);
        buffer.push_str("a-much-longer-secret-value");
        buffer.truncate(4);
        let taken = inputs.take(&key);
        assert_eq!(&*taken, b"a-mu");
        assert!(inputs.values.is_empty());
    }

    #[test]
    fn a_notice_shows_only_on_the_row_and_destination_it_was_for() {
        let reference = CredentialReference::from("key");
        let old_profile = profile("https://grid.example.net/wd/hub", "a");
        let old_key = InputKey::new("grid", &old_profile, &reference);
        let mut inputs = CredentialInputs::default();
        inputs.absorb(vec![WorkbenchNotice {
            provider: "grid".to_string(),
            reference: reference.clone(),
            destination: credential_destination(&old_profile, &reference),
            kind: NoticeKind::StoredInKeychain,
            error: None,
        }]);
        assert!(inputs.last_notice(&old_key).is_some());
        let new_key = InputKey::new("grid", &profile("https://grid.example.net/wd/hub", "b"), &reference);
        assert!(
            inputs.last_notice(&new_key).is_none(),
            "an outcome for the old slot is not shown on the rebound row"
        );
    }
}
