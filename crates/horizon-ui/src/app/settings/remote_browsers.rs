//! Remote browsers tab: provider readiness, credential entry and the portable
//! profile. Values typed here go straight into the credential workbench and
//! are never written to the settings, the store, the process environment or
//! an exported profile.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use egui::Ui;
use horizon_core::Config;
use horizon_core::browser::remote::{
    CredentialBinding, CredentialReference, CredentialStoreKind, RemoteAuthentication, RemoteProviderProfile,
};
use horizon_core::browser::remote_profile::{self, PORTABLE_PROFILE_FILE_NAME};
use horizon_core::named_secrets::{NamedSecretVault, NamedSecretWorkbench};
use horizon_core::remote_browser_credential::{
    CredentialReadiness, CredentialState, CredentialWorkbench, ImportLine, KeyringImport, KeyringImportPlan,
    NoticeKind, Platform, RemoteCredentialError, SecretsState, TailnetLine, WorkbenchNotice, credential_destination,
};
use zeroize::{Zeroize as _, Zeroizing};

use crate::theme;

const MAX_NOTICES: usize = 8;

/// Identity of one text buffer: the row plus the exact destination a value
/// typed there would be sent to. The settings can change a provider's
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
    /// configuration, so a provider removed or rebound by a field (or by
    /// Reset) never keeps a secret that could reappear later.
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

/// Render the tab. Returns whether it changed the draft, which it does only
/// for a binding or an import.
/// The portable-profile row: the path the user edits and the outcome of the
/// last export or import. Outcomes name counts and the path, never a value.
pub(super) struct PortableProfilePanel {
    config_path: PathBuf,
    path: String,
    notice: Option<(String, bool)>,
}

impl PortableProfilePanel {
    pub(super) fn new(config_path: &Path) -> Self {
        Self {
            config_path: config_path.to_path_buf(),
            path: default_portable_profile_path(config_path),
            notice: None,
        }
    }

    /// The row whose last export or import said `notice`, for a test of the width
    /// of the tab.
    #[cfg(test)]
    pub(super) fn with_notice(path: impl Into<String>, notice: (String, bool)) -> Self {
        Self {
            config_path: PathBuf::new(),
            path: path.into(),
            notice: Some(notice),
        }
    }

    /// Write the shareable definition (bindings stripped) to the path. The
    /// configuration file itself is never a destination.
    fn export(&mut self, config: &Config) {
        let path = Path::new(self.path.trim());
        let outcome = remote_profile::refuse_config_path(&self.config_path, path)
            .and_then(|()| remote_profile::write_portable_profile(path, &config.browser.remote));
        self.notice = Some(match outcome {
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
        });
    }

    /// Merge the profile at the path into the editing configuration. Returns
    /// whether the configuration changed; Save writes it to the store.
    fn import(&mut self, config: &mut Config) -> bool {
        let path = Path::new(self.path.trim());
        let outcome = remote_profile::refuse_config_path(&self.config_path, path)
            .and_then(|()| remote_profile::read_portable_profile(path))
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

#[allow(clippy::too_many_arguments)]
pub(super) fn render(
    ui: &mut Ui,
    config: &mut Config,
    workbench: &mut CredentialWorkbench,
    inputs: &mut CredentialInputs,
    portable: &mut PortableProfilePanel,
    usage: &mut super::remote_usage::UsagePanels,
    keyring: &mut KeyringImport,
    named: &mut NamedSecretWorkbench,
) -> bool {
    workbench.poll();
    // A store can relock while this tab is idle. The re-probe happens on a read,
    // so the frame that shows the bound must come, or a locked store would show
    // as present until the pointer moved.
    if let Some(due) = workbench.next_revalidation_in() {
        ui.ctx().request_repaint_after(due);
    }
    let notices = workbench.take_notices();
    if !notices.is_empty() {
        usage.clear();
    }
    inputs.absorb(notices);
    inputs.retain_current(config);
    render_stores_section(ui, workbench);
    render_keyring_section(ui, config, workbench, keyring, named);
    let mut changed = render_portable_section(ui, config, portable);
    let providers = &config.browser.remote.providers;
    usage.retain(|name, _| providers.contains_key(name));
    if providers.is_empty() {
        super::section_heading(ui, "Providers");
        super::section_card(ui, |ui| {
            super::dim_label(
                ui,
                "No remote device services are configured. Import a portable profile above; see \
                 docs/architecture/remote-browser-sessions.md for the form of the browser.remote \
                 section.",
            );
        });
        return changed;
    }
    let mut bind = None;
    for (name, profile) in providers {
        if let Some(request) = render_provider(ui, config, name, profile, workbench, inputs, usage) {
            bind = Some(request);
        }
    }
    if let Some((provider, reference, store)) = bind {
        changed |= bind_reference(config, &provider, &reference, store);
    }
    changed
}

/// The card that reads the values of an older Horizon out of the keyring of this
/// system: the button of the remote browser rows, one line per network of an older
/// Horizon with the three names of the recipe, and the answer of the last click.
///
/// Nothing here runs by itself: a click starts one import on a worker thread. The
/// line stays while a reference of these settings has no value on this computer, so
/// a person can try again after installing the program that reads the keyring. A
/// value of a network is never drawn: the card holds names, and the worker that
/// creates them keeps the values.
fn render_keyring_section(
    ui: &mut Ui,
    config: &Config,
    workbench: &mut CredentialWorkbench,
    keyring: &mut KeyringImport,
    named: &mut NamedSecretWorkbench,
) {
    let plan = KeyringImportPlan::of(&config.browser.remote);
    let offered = !plan.is_empty() && workbench.any_reference_without_value(&plan);
    let answer = keyring.line().cloned();
    let secrets_answer = keyring.secrets_line().cloned();
    let offer = keyring.offer().cloned();
    super::section_heading(ui, "Keyring of this system");
    super::section_card(ui, |ui| {
        if offered {
            super::dim_label(
                ui,
                "A value that an older version stored in the system keyring can be imported.",
            );
            ui.add_space(6.0);
        }
        let busy = keyring.is_busy();
        let button = egui::Button::new(if busy {
            "Importing…"
        } else {
            "Import from the keyring"
        });
        // A value of the keyring can be imported only into the secrets file, so the
        // button waits for that file like the rows of the providers do.
        let store = workbench.store_handle();
        let ready = store.is_some();
        let vault = named.vault();
        if ui.add_enabled(ready && !busy, button).clicked()
            && let Some(store) = store
        {
            keyring.start(plan.clone(), store, vault.clone());
            ui.ctx().request_repaint();
        }
        // The permission of macOS belongs to the Keychain, so the line follows the
        // keyring that the import reads and not the system that compiles this tab.
        if keyring.platform() == Platform::Keychain {
            super::dim_label(
                ui,
                "macOS can ask for your permission once for each value, because the Keychain belongs to you.",
            );
        }
        if let Some(answer) = answer {
            answer_line(ui, &answer);
        }
        if let Some(offer) = offer {
            ui.add_space(8.0);
            super::dim_label(
                ui,
                "An older version kept the OAuth client of a network in the keyring. These three names are \
                 the secrets of the machines recipe, and this computer draws no value.",
            );
            for line in offer.lines() {
                ui.add_space(8.0);
                render_network_line(ui, line, keyring, vault.as_ref(), named);
            }
        }
        if let Some(answer) = secrets_answer {
            answer_line(ui, &answer);
        }
    });
}

/// One line per network of an older Horizon: the three names of the recipe, the
/// names that this computer already holds a value for, and the button that creates
/// the missing ones. A name is drawn, a value never is.
///
/// The presence line and the button read the answer of the store that this window
/// holds, not only the read that offered the network: a click creates the three names
/// and the Secrets tab deletes one of them again, and both then show at once.
fn render_network_line(
    ui: &mut Ui,
    line: &TailnetLine,
    keyring: &mut KeyringImport,
    vault: Option<&std::sync::Arc<NamedSecretVault>>,
    secrets: &NamedSecretWorkbench,
) {
    let followed = line.followed(|name| secrets.held(name));
    let line = &followed;
    let names: Vec<&str> = line
        .names()
        .iter()
        .map(horizon_core::named_secrets::SecretName::as_str)
        .collect();
    ui.add(
        egui::Label::new(
            egui::RichText::new(names.join(" · "))
                .color(theme::FG())
                .size(12.0)
                .monospace(),
        )
        .wrap(),
    );
    super::dim_label(ui, &presence_line(line));
    let complete = line.complete();
    let label = if complete {
        "All three names are here"
    } else {
        "Create these secrets"
    };
    let ready = !keyring.is_busy() && !complete && vault.is_some();
    if ui.add_enabled(ready, egui::Button::new(label)).clicked()
        && let Some(vault) = vault
    {
        keyring.start_secrets(Some(line.net().to_string()), std::sync::Arc::clone(vault));
        ui.ctx().request_repaint();
    }
}

/// The line that says which of the three names of a network this computer holds.
fn presence_line(line: &TailnetLine) -> String {
    let present: Vec<&str> = line
        .names()
        .iter()
        .zip(line.present())
        .filter_map(|(name, present)| present.then_some(name.as_str()))
        .collect();
    match present.as_slice() {
        [] => "No value of the three names is on this computer yet.".to_string(),
        [one] => format!("{one} is already here."),
        [first, second] => format!("{first} and {second} are already here."),
        [first, second, third] => format!("{first}, {second} and {third} are already here."),
        _ => String::new(),
    }
}

/// One answer of a click, in the tone of its rule.
fn answer_line(ui: &mut Ui, answer: &ImportLine) {
    let color = if answer.error {
        theme::PALETTE_RED()
    } else {
        theme::PALETTE_GREEN()
    };
    ui.add(egui::Label::new(egui::RichText::new(&answer.text).color(color).size(11.0)).wrap());
}

/// A machine-local binding for a reference the imported profile left
/// unbound: the session store, or the secrets file under the reference of the
/// provider and reference. Returns whether the editing configuration changed;
/// Save writes it to the store.
fn bind_reference(
    config: &mut Config,
    provider: &str,
    reference: &CredentialReference,
    store: CredentialStoreKind,
) -> bool {
    let Some(profile) = config.browser.remote.providers.get_mut(provider) else {
        return false;
    };
    if profile.credential_bindings.contains_key(reference) {
        return false;
    }
    let slot = match store {
        CredentialStoreKind::Session | CredentialStoreKind::Environment => None,
        CredentialStoreKind::File => Some(format!("remote-browser/{provider}/{}", reference.as_str())),
    };
    profile
        .credential_bindings
        .insert(reference.clone(), CredentialBinding { store, slot });
    true
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
            // An outcome names a path, and a path is as long as the person made
            // it: it wraps inside the panel instead of widening it.
            ui.add(egui::Label::new(egui::RichText::new(text).color(color).size(12.0)).wrap());
        }
    });
    changed
}

fn render_stores_section(ui: &mut Ui, workbench: &mut CredentialWorkbench) {
    super::section_heading(ui, "Credential stores");
    super::section_card(ui, |ui| {
        let (label, color) = match workbench.secrets_state() {
            SecretsState::Opening => ("Secrets file: checking".to_string(), theme::FG_DIM()),
            SecretsState::Ready => ("Secrets file: available".to_string(), theme::PALETTE_GREEN()),
            SecretsState::Unavailable(error) => {
                (format!("Secrets file: unavailable ({error})"), theme::PALETTE_YELLOW())
            }
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
            "Session-only values are discarded when Horizon exits. Values in the secrets file persist on this computer only \
             and are never exported. Environment bindings are read from the named launch variables; \
             agent and terminal children inherit them normally.",
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
    usage: &mut super::remote_usage::UsagePanels,
) -> Option<(String, CredentialReference, CredentialStoreKind)> {
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
                "{} · {} · {}, {} s allocation, {} s idle, {} s max · targets: {}",
                profile.endpoint.as_str(),
                authentication_label(&profile.authentication),
                profile.local_session_limit().map_or_else(
                    || "Provider-managed capacity".to_string(),
                    |limit| format!("Local limit: {limit} session(s)"),
                ),
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
        super::remote_usage::render(ui, name, profile, workbench, usage);
        let readiness = workbench.readiness(profile);
        if readiness.is_empty() {
            super::dim_label(ui, "No authentication configured for this provider.");
            return None;
        }
        ui.add_space(8.0);
        let mut bind = None;
        for entry in readiness {
            if let Some(store) = render_reference(ui, name, profile, &entry, workbench, inputs) {
                bind = Some((name.to_string(), entry.reference.clone(), store));
            }
            ui.add_space(6.0);
        }
        bind
    })
}

fn render_reference(
    ui: &mut Ui,
    provider: &str,
    profile: &RemoteProviderProfile,
    entry: &CredentialReadiness,
    workbench: &mut CredentialWorkbench,
    inputs: &mut CredentialInputs,
) -> Option<CredentialStoreKind> {
    let (reference, store, state) = (&entry.reference, entry.store, entry.state);
    let bound = profile.credential_bindings.contains_key(reference);
    let key = InputKey::new(provider, profile, reference);
    readiness_row(ui, entry, bound);
    if !bound {
        // An imported profile arrives without bindings: choose where this
        // computer keeps the value, then the row takes it.
        return unbound_row(ui, workbench);
    }
    if store == CredentialStoreKind::Environment {
        if let Some(variable) = profile
            .credential_bindings
            .get(reference)
            .and_then(CredentialBinding::environment_variable)
        {
            super::dim_label(
                ui,
                &format!("Bound to environment variable `{variable}`. The value is read at launch and never shown."),
            );
        }
        return None;
    }
    value_row(
        ui,
        ValueRow {
            provider,
            profile,
            reference,
            store,
            state,
            key: &key,
        },
        workbench,
        inputs,
    );
    if let Some(notice) = inputs.last_notice(&key)
        && notice_agrees_with(notice, state)
    {
        let (text, color) = notice_line(notice.kind, notice.error.as_ref());
        ui.label(egui::RichText::new(text).color(color).size(11.0));
    }
    None
}

/// The name of the reference, where this computer keeps its value and the state
/// of that value. A long reference is cut inside its row, so the row never
/// widens the panel beside the canvas.
fn readiness_row(ui: &mut Ui, entry: &CredentialReadiness, bound: bool) {
    ui.horizontal_wrapped(|ui| {
        ui.add(
            egui::Label::new(
                egui::RichText::new(entry.reference.as_str())
                    .color(theme::FG())
                    .size(12.0)
                    .strong(),
            )
            .truncate(),
        );
        ui.add(
            egui::Label::new(
                egui::RichText::new(store_label(entry.store, bound))
                    .color(theme::FG_DIM())
                    .size(11.0),
            )
            .truncate(),
        );
        let (state_text, color) = state_badge(entry.state);
        ui.add(egui::Label::new(egui::RichText::new(state_text).color(color).size(11.0)).truncate());
    });
}

/// Asks where this computer keeps the value of a reference that no binding
/// names yet, and answers the place that the person chose.
fn unbound_row(ui: &mut Ui, workbench: &CredentialWorkbench) -> Option<CredentialStoreKind> {
    let mut chosen = None;
    ui.horizontal_wrapped(|ui| {
        super::dim_label(ui, "Not bound on this computer. Keep the value in:");
        if ui.small_button("This session only").clicked() {
            chosen = Some(CredentialStoreKind::Session);
        }
        let available = workbench.secrets_state() == &SecretsState::Ready;
        if ui
            .add_enabled(available, egui::Button::new("Secrets file").small())
            .clicked()
        {
            chosen = Some(CredentialStoreKind::File);
        }
    });
    chosen
}

/// The place of one value, the state of that value and the buffers of the row
/// that edits it.
#[derive(Clone, Copy)]
struct ValueRow<'a> {
    provider: &'a str,
    profile: &'a RemoteProviderProfile,
    reference: &'a CredentialReference,
    store: CredentialStoreKind,
    state: CredentialState,
    key: &'a InputKey,
}

/// The hidden field of a value with the actions of its row, and what a click of
/// those actions does. Every outcome, including a synchronous refusal, arrives as
/// a workbench notice for this row.
fn value_row(ui: &mut Ui, row: ValueRow<'_>, workbench: &mut CredentialWorkbench, inputs: &mut CredentialInputs) {
    let ValueRow {
        provider,
        profile,
        reference,
        store,
        state,
        key,
    } = row;
    let has_text = !inputs.buffer(key).is_empty();
    let file_available = workbench.secrets_state() == &SecretsState::Ready;
    let mut stored = false;
    let mut deleted = false;
    super::action_row(
        ui,
        |ui| password_field(ui, inputs.buffer(key), provider, reference, ui.available_width()),
        |ui| {
            // The actions are placed from the right, so `Delete` comes first and
            // stays at the end of the row.
            if state == CredentialState::Present {
                deleted = ui.button("Delete").clicked();
            }
            match store {
                CredentialStoreKind::Session => {
                    stored = ui
                        .add_enabled(has_text, egui::Button::new("Set for this session"))
                        .clicked();
                }
                CredentialStoreKind::File => {
                    stored = ui
                        .add_enabled(
                            has_text && file_available,
                            egui::Button::new("Save to the secrets file"),
                        )
                        .clicked();
                }
                CredentialStoreKind::Environment => {}
            }
        },
    );
    if stored {
        let value = inputs.take(key);
        match store {
            CredentialStoreKind::File => {
                let _ = workbench.store_value(provider, profile, reference, &value);
            }
            _ => {
                let _ = workbench.set_session_value(provider, profile, reference, &value);
            }
        }
    }
    if deleted {
        let _ = workbench.delete(provider, profile, reference);
    }
}

fn password_field(ui: &mut Ui, buffer: &mut String, provider: &str, reference: &CredentialReference, width: f32) {
    let mut output = egui::TextEdit::singleline(buffer)
        .password(true)
        .char_limit(16_384)
        .hint_text("value")
        .desired_width(width)
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
        (CredentialStoreKind::File, true) => "secrets file",
        (CredentialStoreKind::Environment, true) => "environment",
    }
}

fn state_badge(state: CredentialState) -> (&'static str, egui::Color32) {
    match state {
        CredentialState::Present => ("present", theme::PALETTE_GREEN()),
        CredentialState::Missing => ("missing", theme::PALETTE_YELLOW()),
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
        NoticeKind::SessionValueSet | NoticeKind::StoredInFile => state == CredentialState::Present,
        NoticeKind::SessionValueCleared | NoticeKind::DeletedFromFile => state == CredentialState::Missing,
    }
}

fn notice_line(kind: NoticeKind, error: Option<&RemoteCredentialError>) -> (String, egui::Color32) {
    match (kind, error) {
        (NoticeKind::SessionValueSet, None) => ("Held for this session.".to_string(), theme::PALETTE_GREEN()),
        (NoticeKind::SessionValueCleared, None) => ("Session value cleared.".to_string(), theme::FG_DIM()),
        (NoticeKind::StoredInFile, None) => ("Saved to the secrets file.".to_string(), theme::PALETTE_GREEN()),
        (NoticeKind::DeletedFromFile, None) => ("Deleted from the secrets file.".to_string(), theme::FG_DIM()),
        (NoticeKind::SessionValueSet | NoticeKind::StoredInFile, Some(error)) => {
            (format!("Not saved: {error}."), theme::PALETTE_RED())
        }
        (NoticeKind::SessionValueCleared | NoticeKind::DeletedFromFile, Some(error)) => {
            (format!("Not deleted: {error}."), theme::PALETTE_RED())
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Arc;

    use horizon_core::browser::remote::{ControlEndpoint, CredentialBinding, RemoteAdapterKind, RemoteSessionLimits};
    use horizon_core::named_secrets::{NamedSecretVault, NamedSecretWorkbench, SecretName};
    use horizon_core::remote_browser_credential::{
        Bound, KeyringImport, ProgramEnvironment, RemoteCredentialStore, SharedStore,
    };
    use horizon_core::secret_store::SecretStore;

    use super::*;

    fn profile(endpoint: &str, slot: &str) -> RemoteProviderProfile {
        let mut bindings = BTreeMap::new();
        bindings.insert(
            CredentialReference::from("key"),
            CredentialBinding {
                store: CredentialStoreKind::File,
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

    /// A stand-in for the program that reads the keyring of `platform`: one item on
    /// stdout, its account on stderr, in the format of `secret-tool` of libsecret, or
    /// the value of one account for the Keychain of macOS.
    fn stand_in_keyring(folder: &std::path::Path, platform: Platform, user: &str) {
        use std::os::unix::fs::PermissionsExt as _;

        let (name, script) = match platform {
            Platform::SecretService => (
                "secret-tool",
                format!(
                    "#!/bin/sh\n\
                     printf '%s\\n' '[/org/freedesktop/secrets/collection/login/7]'\n\
                     printf '%s\\n' 'label = keyring@horizon-remote-browser'\n\
                     printf '%s\\n' 'secret = synthetic-token'\n\
                     printf '%s\\n' 'created = '\n\
                     printf '%s\\n' 'modified = '\n\
                     printf '%s\\n' 'attribute.service = horizon-remote-browser' >&2\n\
                     printf '%s\\n' 'attribute.username = {user}' >&2\n"
                ),
            ),
            Platform::Keychain => (
                "security",
                format!(
                    "#!/bin/sh\n\
                     if [ \"$1\" = find-generic-password ] && [ \"$2\" = -s ] && \
                     [ \"$3\" = horizon-remote-browser ] && [ \"$4\" = -a ] && \
                     [ \"$5\" = '{user}' ] && [ \"$6\" = -w ]; then\n\
                     printf '%s\\n' 'synthetic-token'\n\
                     exit 0\n\
                     fi\n\
                     exit 44\n"
                ),
            ),
        };
        let path = folder.join(name);
        std::fs::write(&path, script).expect("the stand-in");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("an executable stand-in");
    }

    /// A stand-in for `secret-tool` that answers the OAuth client of the saved network
    /// `saved-id` on the tailnet service of an older Horizon, and no item of any other
    /// service.
    fn stand_in_tailnet_keyring(folder: &std::path::Path) {
        let script = r#"if [ "$1" = search ] && [ "$4" = service ] && [ "$5" = horizon-cloud-tailnet-api ]; then
    printf '%s\n' '[/org/freedesktop/secrets/collection/login/7]'
    printf '%s\n' 'label = horizon-cloud-tailnet-api'
    printf '%s\n' 'secret = {"kind":"oauth","client_id":"synthetic-id","client_secret":"synthetic-secret"}'
    printf '%s\n' 'created = '
    printf '%s\n' 'modified = '
    printf '%s\n' 'attribute.service = horizon-cloud-tailnet-api' >&2
    printf '%s\n' 'attribute.username = saved-id' >&2
fi
exit 0
"#;
        horizon_core::stand_in_program_in(folder, "secret-tool", script).expect("the stand-in program");
    }

    /// A workbench of the secrets file of a test, and the store behind it.
    fn file_workbench() -> (
        std::sync::Arc<horizon_core::secret_store::SecretStore>,
        CredentialWorkbench,
    ) {
        use std::sync::Arc;

        use horizon_core::remote_browser_credential::RemoteCredentialStore;
        use horizon_core::secret_store::SecretStore;

        let values = SecretStore::in_memory().expect("a secrets file in memory");
        let kept = Arc::clone(&values);
        let mut workbench = CredentialWorkbench::with_opener(Box::new(move || {
            Ok(Box::new(kept) as Box<dyn RemoteCredentialStore + Send>)
        }));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let profile = remote_with("grid", "https://grid.example.net/wd/hub").providers["grid"].clone();
        while std::time::Instant::now() < deadline {
            workbench.poll();
            let readiness = workbench.readiness(&profile);
            if workbench.secrets_state() == &SecretsState::Ready
                && readiness.iter().all(|entry| entry.state == CredentialState::Missing)
            {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        (values, workbench)
    }

    /// The line and the button appear while a reference of these settings has no
    /// value, the click reaches the one answer line, and both go when the value is
    /// there. The tab reads the keyring of either system on the computer that runs the
    /// tests.
    #[test]
    fn the_remote_tab_offers_the_import_of_the_keyring_while_a_value_is_missing() {
        for platform in [Platform::SecretService, Platform::Keychain] {
            the_remote_tab_offers_the_import(platform);
        }
    }

    fn the_remote_tab_offers_the_import(platform: Platform) {
        use horizon_core::remote_browser_credential::{Bound, KeyringImport, ProgramEnvironment};

        let mut config = Config::default();
        config.browser.remote = remote_with("grid", "https://grid.example.net/wd/hub");
        let user = "https://grid.example.net|remote-browser/grid/key";
        let folder = tempfile::tempdir().expect("a folder");
        stand_in_keyring(folder.path(), platform, user);
        let (values, mut workbench) = file_workbench();
        let mut named = horizon_core::named_secrets::NamedSecretWorkbench::new(None, &[]);
        let mut keyring = KeyringImport::with_programs(
            platform,
            ProgramEnvironment::with_path(folder.path()),
            Bound::WholeImport(std::time::Duration::from_secs(10)),
        );
        let mut draw = |workbench: &mut CredentialWorkbench, keyring: &mut KeyringImport| {
            let (_changed, texts) = super::super::tests::frame_of(|ui| {
                super::render_keyring_section(ui, &config, workbench, keyring, &mut named);
                false
            });
            texts
        };

        let texts = draw(&mut workbench, &mut keyring);
        assert!(
            super::super::tests::texts_hold(
                &texts,
                "A value that an older version stored in the system keyring can be imported."
            ),
            "the tab offers the import: {texts:?}"
        );
        assert!(
            super::super::tests::texts_hold(&texts, "Import from the keyring"),
            "the tab holds the button: {texts:?}"
        );
        assert_eq!(
            super::super::tests::texts_hold(
                &texts,
                "macOS can ask for your permission once for each value, because the Keychain belongs to you."
            ),
            platform == Platform::Keychain,
            "only the Keychain asks the person of macOS for permission: {texts:?}"
        );

        // The click is what starts the import, and its answer is the one line of
        // the card.
        let plan = KeyringImportPlan::of(&config.browser.remote);
        let store = workbench.store_handle().expect("the secrets file of the workbench");
        keyring.start(plan, store, None);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while keyring.is_busy() && std::time::Instant::now() < deadline {
            keyring.poll();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(!keyring.is_busy(), "the import answered");

        let texts = draw(&mut workbench, &mut keyring);
        assert!(
            super::super::tests::texts_hold(&texts, "Imported 1 value."),
            "the tab says what the import did: {texts:?}"
        );
        assert!(
            !texts.iter().any(|text| text.contains("synthetic-token")),
            "no value is drawn: {texts:?}"
        );

        // The value is on this computer now, so the presence read takes the line
        // and the button away. The row is routed to the change list of the link
        // exactly as the window does it after the worker of the import answered.
        workbench.adopt_imported_values(keyring.take_changes());
        let profile = config.browser.remote.providers["grid"].clone();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while std::time::Instant::now() < deadline {
            workbench.poll();
            if workbench.readiness(&profile)[0].state == CredentialState::Present {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(
            workbench.readiness(&profile)[0].state,
            CredentialState::Present,
            "the import wrote the row of the reference"
        );
        let texts = draw(&mut workbench, &mut keyring);
        assert!(
            !super::super::tests::texts_hold(
                &texts,
                "A value that an older version stored in the system keyring can be imported."
            ),
            "a value on this computer takes the offer away: {texts:?}"
        );
        assert!(
            super::super::tests::texts_hold(
                &texts,
                "macOS can ask for your permission once for each value, because the Keychain belongs to you."
            ) == (platform == Platform::Keychain),
            "the permission of macOS stands beside the button that reads the keyring: {texts:?}"
        );
        assert!(
            super::super::tests::texts_hold(&texts, "Imported 1 value."),
            "the answer of the click stays with the person: {texts:?}"
        );
        drop(values);
    }

    /// The networks of an older Horizon reach the card as three names each: the
    /// names that this computer holds, the names that a click creates, and no value.
    #[test]
    fn the_card_offers_the_three_names_of_a_network_of_an_older_horizon() {
        use horizon_core::remote_browser_credential::{Bound, KeyringImport, ProgramEnvironment, TailnetOffer};

        let config = Config::default();
        let (_values, mut workbench) = file_workbench();
        let mut named = horizon_core::named_secrets::NamedSecretWorkbench::new(None, &[]);
        let mut keyring = KeyringImport::with_programs(
            Platform::SecretService,
            ProgramEnvironment::with_path(std::env::temp_dir()),
            Bound::WholeImport(std::time::Duration::from_secs(1)),
        );
        let line = TailnetLine::of("HOME".to_string(), [true, false, false]).expect("the three names of the recipe");
        keyring.adopt_offer(TailnetOffer::of(vec![line]));

        let (_changed, texts) = super::super::tests::frame_of(|ui| {
            super::render_keyring_section(ui, &config, &mut workbench, &mut keyring, &mut named);
            false
        });
        let holds = |needle: &str| texts.iter().any(|text| text.contains(needle));
        assert!(
            holds("TS_HOME_CLIENT_ID · TS_HOME_CLIENT_SECRET · TS_HOME_TAG"),
            "the card names the three secrets of the recipe: {texts:?}"
        );
        assert!(
            holds("TS_HOME_CLIENT_ID is already here."),
            "the card says which name this computer holds: {texts:?}"
        );
        assert!(
            holds("Create these secrets"),
            "the card offers the button of the missing names: {texts:?}"
        );
        assert!(
            !texts.iter().any(|text| text.contains("synthetic")),
            "the card draws no value: {texts:?}"
        );
    }

    /// The card of the keyring over one saved network of an older Horizon, with the
    /// secrets file of this computer and the tab of its named secrets.
    struct NetworkCard {
        _programs: tempfile::TempDir,
        _home: tempfile::TempDir,
        config: Config,
        vault: Arc<NamedSecretVault>,
        store: SharedStore,
        named: NamedSecretWorkbench,
        workbench: CredentialWorkbench,
        keyring: KeyringImport,
        names: [SecretName; 3],
    }

    impl NetworkCard {
        /// The card of the network `Home`, whose OAuth client the stand-in keyring
        /// holds, and a secrets file that holds no named secret.
        fn new() -> Self {
            let programs = tempfile::tempdir().expect("a folder of stand-in programs");
            stand_in_tailnet_keyring(programs.path());
            let home = tempfile::tempdir().expect("the home of the older version");
            let cloud = home.path().join("cloud");
            std::fs::create_dir_all(&cloud).expect("the folder of the older version");
            std::fs::write(
                cloud.join("tailnets.json"),
                r#"{"tailnets":[{"id":"saved-id","name":"Home"}]}"#,
            )
            .expect("the saved network of the older version");

            let values = SecretStore::in_memory().expect("a secrets file in memory");
            let vault = Arc::new(NamedSecretVault::new(Box::new(Arc::clone(&values))));
            let store: SharedStore = Arc::new(std::sync::Mutex::new(
                Box::new(Arc::clone(&values)) as Box<dyn RemoteCredentialStore + Send>
            ));
            let names = ["TS_HOME_CLIENT_ID", "TS_HOME_CLIENT_SECRET", "TS_HOME_TAG"]
                .map(|text| SecretName::parse(text).expect("a name of the recipe"));
            let secrets = NamedSecretWorkbench::new(Some(Arc::clone(&vault)), &names);
            let keyring = KeyringImport::with_programs(
                Platform::SecretService,
                ProgramEnvironment::with_path(programs.path()),
                Bound::WholeImport(std::time::Duration::from_secs(10)),
            )
            .with_old(cloud);
            Self {
                _programs: programs,
                _home: home,
                config: Config::default(),
                vault,
                store,
                named: secrets,
                workbench: file_workbench().1,
                keyring,
                names,
            }
        }

        /// The texts of the card in this frame.
        fn texts(&mut self) -> Vec<String> {
            let (_changed, texts) = super::super::tests::frame_of(|ui| {
                super::render_keyring_section(
                    ui,
                    &self.config,
                    &mut self.workbench,
                    &mut self.keyring,
                    &mut self.named,
                );
                false
            });
            texts
        }

        /// The first click, on `Import from the keyring`.
        fn import(&mut self) {
            let plan = KeyringImportPlan::of(&self.config.browser.remote);
            self.keyring
                .start(plan, Arc::clone(&self.store), Some(Arc::clone(&self.vault)));
            self.answered();
        }

        /// The press of `Create these secrets` on the line of the network.
        fn create(&mut self) {
            self.keyring
                .start_secrets(Some("HOME".to_string()), Arc::clone(&self.vault));
            self.answered();
        }

        /// The delete of one name of the network in the `Secrets` tab.
        fn delete(&mut self, index: usize) {
            self.vault.remove(&self.names[index]).expect("the delete of one name");
        }

        /// The answer of the card's worker, until it is on the line of its button.
        fn answered(&mut self) {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            while self.keyring.is_busy() && std::time::Instant::now() < deadline {
                self.keyring.poll();
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            assert!(!self.keyring.is_busy(), "the import of the keyring answered");
        }

        /// The read of the secrets file that the window asks for after a change, until
        /// its answer names these three names.
        fn read(&mut self, expected: [Option<bool>; 3]) {
            self.named.refresh(&self.names);
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            let mut held = self.held();
            while held != expected && std::time::Instant::now() < deadline {
                self.named.poll();
                held = self.held();
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            assert_eq!(held, expected, "the tab read the secrets file");
        }

        /// Whether the tab holds a value for each of the three names.
        fn held(&self) -> [Option<bool>; 3] {
            self.names.each_ref().map(|name| self.named.held(name))
        }
    }

    /// Whether one of the drawn texts of the card holds `needle`.
    fn card_holds(texts: &[String], needle: &str) -> bool {
        texts.iter().any(|text| text.contains(needle))
    }

    /// The presence line and the button of a network follow the secrets file. The read
    /// that offered the network answered before the click, so the answer of the click
    /// names the three names as here, and a delete in the Secrets tab takes one of them
    /// away again.
    #[test]
    fn the_line_of_a_network_follows_the_secrets_that_the_click_created() {
        let mut card = NetworkCard::new();
        card.import();
        let texts = card.texts();
        assert!(
            card_holds(&texts, "No value of the three names is on this computer yet."),
            "the line of the network says what the store holds: {texts:?}"
        );
        assert!(card_holds(&texts, "Create these secrets"), "{texts:?}");

        card.create();
        // The frame that takes the created names reads the store again, exactly as the
        // window does: the names enter the settings list and the tab asks for them.
        card.read([Some(true); 3]);
        let texts = card.texts();
        assert!(card_holds(&texts, "Created 3 secrets of the network."), "{texts:?}");
        assert!(
            card_holds(
                &texts,
                "TS_HOME_CLIENT_ID, TS_HOME_CLIENT_SECRET and TS_HOME_TAG are already here."
            ),
            "the presence line follows the secrets that the click created: {texts:?}"
        );
        assert!(
            card_holds(&texts, "All three names are here"),
            "the button follows the secrets that the click created: {texts:?}"
        );
        assert!(!card_holds(&texts, "Create these secrets"), "{texts:?}");
        assert!(!card_holds(&texts, "synthetic"), "the card draws no value: {texts:?}");

        // The Secrets tab deletes the tag, so the line and the button of the network
        // offer it again, and the two names that stay are still here.
        card.delete(2);
        card.read([Some(true), Some(true), Some(false)]);
        let texts = card.texts();
        assert!(
            card_holds(&texts, "TS_HOME_CLIENT_ID and TS_HOME_CLIENT_SECRET are already here."),
            "the presence line follows the delete of the Secrets tab: {texts:?}"
        );
        assert!(
            card_holds(&texts, "Create these secrets"),
            "the button follows the delete of the Secrets tab: {texts:?}"
        );
        assert!(!card_holds(&texts, "All three names are here"), "{texts:?}");
    }

    /// The row that an import of the keyring writes is queued for the link to a
    /// server exactly like a value that a person typed: the frame that takes the
    /// answer of the worker routes its reference into the change list.
    #[test]
    fn the_row_that_an_import_writes_is_queued_for_the_link() {
        for platform in [Platform::SecretService, Platform::Keychain] {
            the_row_of_an_import_is_queued(platform);
        }
    }

    fn the_row_of_an_import_is_queued(platform: Platform) {
        use horizon_core::RuntimeState;
        use horizon_core::remote_browser_credential::{Bound, KeyringImport, ProgramEnvironment};

        use crate::app::test_support::{run_app_frame, test_app_with_config_and_startup};

        let mut config = Config::default();
        config.browser.remote = remote_with("grid", "https://grid.example.net/wd/hub");
        let user = "https://grid.example.net|remote-browser/grid/key";
        let folder = tempfile::tempdir().expect("a folder");
        stand_in_keyring(folder.path(), platform, user);
        let (_temp, ctx, mut app) = test_app_with_config_and_startup(
            &config,
            horizon_core::StartupDecision::Ephemeral {
                runtime_state: Box::new(RuntimeState::default()),
            },
        );
        app.keyring_import = KeyringImport::with_programs(
            platform,
            ProgramEnvironment::with_path(folder.path()),
            Bound::WholeImport(std::time::Duration::from_secs(10)),
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while std::time::Instant::now() < deadline && app.remote_browser_credentials.store_handle().is_none() {
            app.remote_browser_credentials.poll();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let store = app
            .remote_browser_credentials
            .store_handle()
            .expect("the workbench opened the secrets file of this home");
        assert!(
            app.remote_browser_credentials.take_value_changes().is_empty(),
            "nothing changed yet"
        );

        app.keyring_import
            .start(KeyringImportPlan::of(&config.browser.remote), store, None);
        // A frame takes the answer of the import worker, and the same frame routes
        // its reference into the change list that the link to a server reads. This
        // app follows no server, so that list is empty again when the frame ends:
        // the entry itself is the check of the core module.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while app.keyring_import.line().is_none() && std::time::Instant::now() < deadline {
            run_app_frame(&ctx, &mut app);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(
            app.keyring_import.line().map(|line| line.text.as_str()),
            Some("Imported 1 value."),
            "the tab holds the line of the import: {platform:?}"
        );

        // The frame cleared the presence that an import invalidates, so the read of
        // this reference answers `present` at once instead of at the next
        // revalidation. Without that routing the row would stay `missing` for the
        // ten seconds of the cache.
        let profile = config.browser.remote.providers["grid"].clone();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        let mut state = CredentialState::Missing;
        while state != CredentialState::Present && std::time::Instant::now() < deadline {
            run_app_frame(&ctx, &mut app);
            state = app.remote_browser_credentials.readiness(&profile)[0].state;
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(
            state,
            CredentialState::Present,
            "the row of the imported value is on this computer, as a typed value would be"
        );
        assert!(
            app.remote_browser_credentials.take_value_changes().is_empty(),
            "this app follows no server, so the frame sent the change to nobody"
        );
    }

    #[test]
    fn the_portable_panel_imports_into_the_editing_config_and_exports_without_bindings() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config_path = dir.path().join("config.json");
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
        panel.path = dir.path().join("absent.json").display().to_string();
        assert!(!panel.import(&mut second));
        let (notice, is_error) = panel.notice.clone().expect("notice");
        assert!(is_error && notice.contains("absent.json"), "{notice}");

        // The configuration file is never exported over or imported from.
        std::fs::write(&config_path, first.to_json().expect("json")).expect("config");
        let saved = std::fs::read_to_string(&config_path).expect("read");
        panel.path = config_path.display().to_string();
        panel.export(&first);
        let (notice, is_error) = panel.notice.clone().expect("notice");
        assert!(is_error && notice.contains("is the configuration file"), "{notice}");
        assert!(!panel.import(&mut second));
        assert_eq!(std::fs::read_to_string(&config_path).expect("read"), saved);
    }

    #[test]
    fn binding_an_unbound_reference_adds_a_machine_local_binding_once() {
        let mut config = Config::default();
        config.browser.remote = remote_with("grid", "https://grid.example.net/wd/hub");
        let grid = config.browser.remote.providers.get_mut("grid").expect("grid");
        grid.credential_bindings.clear();
        let reference = CredentialReference::from("key");

        assert!(bind_reference(
            &mut config,
            "grid",
            &reference,
            CredentialStoreKind::File
        ));
        let binding = &config.browser.remote.providers["grid"].credential_bindings[&reference];
        assert_eq!(binding.store, CredentialStoreKind::File);
        assert_eq!(binding.slot.as_deref(), Some("remote-browser/grid/key"));

        // Already bound: nothing changes, whatever store is asked for.
        assert!(!bind_reference(
            &mut config,
            "grid",
            &reference,
            CredentialStoreKind::Session
        ));
        assert_eq!(
            config.browser.remote.providers["grid"].credential_bindings[&reference].store,
            CredentialStoreKind::File
        );
        assert!(!bind_reference(
            &mut config,
            "absent",
            &reference,
            CredentialStoreKind::Session
        ));

        let token = CredentialReference::from("token");
        assert!(bind_reference(
            &mut config,
            "grid",
            &token,
            CredentialStoreKind::Session
        ));
        let session = &config.browser.remote.providers["grid"].credential_bindings[&token];
        assert_eq!(session.store, CredentialStoreKind::Session);
        assert!(session.slot.is_none());
    }

    #[test]
    fn the_portable_section_renders_for_an_empty_configuration() {
        use crate::test_egui::DiscardTextures;

        let dir = tempfile::tempdir().expect("tempdir");
        let mut panel = PortableProfilePanel::new(&dir.path().join("config.json"));
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
            kind: NoticeKind::StoredInFile,
            error: None,
        };
        assert!(notice_agrees_with(&saved, CredentialState::Present));
        assert!(
            !notice_agrees_with(&saved, CredentialState::Missing),
            "a delete through an aliased row hides the stale Saved"
        );
        let failed = WorkbenchNotice {
            error: Some(RemoteCredentialError::StoreUnavailable),
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
            kind: NoticeKind::StoredInFile,
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
