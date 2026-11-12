//! One computer follows the settings of its server, and keeps its own when the
//! server is away.
//!
//! The link carries one object of the kind `setting` for each shared section of
//! the settings store, and this module is the worker that turns one answer of the
//! server into rows of that store. It runs on the worker thread of the link and
//! never on the UI thread: a frame of the window reads the settings again through
//! the ordinary watch of the settings file.
//!
//! Three rules hold here:
//!
//! 1. **The first link compares once.** A section that the server does not have is
//!    pushed as the value of this computer; a section that both have keeps the
//!    value of the server, and the value of this computer is kept in one backup
//!    file beside the store, at the mode of a private file.
//! 2. **The rule of the settings of this computer decides.** A section of an older
//!    configuration version is migrated, a section of a newer version stays on the
//!    server with one notice, and a section whose merged configuration breaks a
//!    rule of this build is neither applied nor written.
//! 3. **A refusal keeps the winning value.** A change of this computer on a version
//!    that another computer decided first is refused with the version that decided
//!    it, the row of that version comes back, and the person reads one line about
//!    it.

use std::collections::BTreeMap;
use std::path::PathBuf;

use horizon_core::config_store::ConfigStore;
use horizon_core::settings_sync::{self, SectionFault};
use horizon_core::{Config, MachineError};
use serde_json::Value;

use super::SyncHandle;
use super::files::MirroredSetting;
use super::pull::now_millis;
use super::state::{SettingsSyncView, SyncGuard};
use crate::intent::Op;

/// The settings of one computer that follows a server, beside the machine store
/// of the same link.
///
/// A caller queues the section that a person saved, and reads the state of the
/// settings sync from the same handle: the apply of a section of the server runs
/// on the worker thread of the link, and the window reads the store again through
/// its own watch of the settings file.
#[derive(Clone, Debug)]
pub struct SyncedSettings {
    handle: SyncHandle,
}

impl SyncedSettings {
    /// The settings of the link of `handle`.
    #[must_use]
    pub fn new(handle: SyncHandle) -> Self {
        Self { handle }
    }

    /// The handle of the supervisor behind these settings.
    #[must_use]
    pub fn handle(&self) -> &SyncHandle {
        &self.handle
    }

    /// Queues one intent for each section of `sections` that changed, so the other
    /// computers of the person read the same value. A local section, and a section
    /// that did not change, writes nothing.
    ///
    /// # Errors
    /// Returns why the queue file cannot be written.
    pub fn enqueue(&self, config: &Config, sections: &[String]) -> Result<(), MachineError> {
        for section in sections {
            self.handle.enqueue_setting(config, section)?;
        }
        Ok(())
    }

    /// The state of the settings of this computer on its server.
    #[must_use]
    pub fn view(&self) -> SettingsSyncView {
        self.handle.settings_view()
    }
}

/// Applies the sections that the server gave to the settings store of this
/// computer, and compares the settings of this computer with the server at the
/// first link.
///
/// A computer whose settings do not follow a server holds no settings path, and
/// this call does nothing then.
pub(super) fn apply(handle: &SyncHandle) {
    let (path, seeded): (Option<PathBuf>, bool) = {
        let state = handle.lock();
        (state.settings_path.clone(), state.settings_seeded)
    };
    let Some(path) = path else {
        return;
    };
    let store = ConfigStore::resolve(&path);
    if !seeded && !reconcile(handle, &store) {
        return;
    }
    apply_rows(handle, &store);
}

/// Compares the shared sections of this computer with the sections that the
/// server already has, once, and answers whether the comparison happened.
///
/// A store that cannot be read answers `false`: nothing is pushed and nothing is
/// applied, and the next answer of the server tries again.
fn reconcile(handle: &SyncHandle, store: &ConfigStore) -> bool {
    let local = match store.section_rows() {
        Ok(rows) => rows,
        Err(error) => {
            refuse(handle, format!("The settings of this computer: {error}"));
            return false;
        }
    };
    let rows = {
        let state = handle.lock();
        state.settings.clone()
    };
    let defaults = Config::default();
    let mut pushes: Vec<(String, Value)> = Vec::new();
    let mut replaced: BTreeMap<String, String> = BTreeMap::new();
    let mut arriving: BTreeMap<String, Value> = BTreeMap::new();
    let mut applied: BTreeMap<String, u64> = BTreeMap::new();
    let mut faults: BTreeMap<String, (u64, SectionFault)> = BTreeMap::new();
    for section in settings_sync::shared_sections() {
        let Some(local_value) = local_section(&local, &defaults, section, handle) else {
            continue;
        };
        let Some(row) = rows.get(section) else {
            pushes.push((section.to_owned(), local_value));
            continue;
        };
        match server_section(section, row) {
            Ok(server) if settings_sync::covers(&server, &local_value) => {
                applied.insert(section.to_owned(), row.version);
            }
            Ok(server) => {
                replaced.insert(section.to_owned(), local_value.to_string());
                arriving.insert(section.to_owned(), server);
            }
            Err(fault) => {
                faults.insert(section.to_owned(), (row.version, fault));
            }
        }
    }

    let mut lines = Vec::new();
    if !replaced.is_empty() {
        let backup = settings_sync::backup_path(store, now_millis());
        match settings_sync::write_backup(&backup, &replaced) {
            Ok(()) => lines.push(format!(
                "The server had settings of another computer, so this computer took them. The settings of this \
                 computer are kept in {}.",
                backup.display()
            )),
            Err(error) => lines.push(format!(
                "The server had settings of another computer, so this computer took them. The settings of this \
                 computer could not be kept in {}: {error}",
                backup.display()
            )),
        }
        match settings_sync::apply_to_store(store, &arriving) {
            Ok(_) => {
                for section in arriving.keys() {
                    if let Some(row) = rows.get(section) {
                        applied.insert(section.clone(), row.version);
                    }
                }
            }
            Err(fault) => {
                lines.push(fault.to_string());
                for section in arriving.keys() {
                    if let Some(row) = rows.get(section) {
                        faults.insert(section.clone(), (row.version, fault.clone()));
                    }
                }
            }
        }
    }
    for (section, (_, fault)) in &faults {
        lines.push(format!("The setting {section}: {fault}"));
    }

    let mut state = handle.lock();
    for (section, value) in pushes {
        if let Err(error) = enqueue_value(&mut state, &section, defaults.version, &value) {
            lines.push(format!("The setting {section} did not enter the queue: {error}"));
        }
    }
    state.settings_seeded = true;
    state.settings_applied.extend(applied);
    for (section, (version, _)) in &faults {
        state.settings_refused.insert(section.clone(), *version);
    }
    state.settings_refusal = lines.iter().next_back().cloned();
    state.settings_generation = state.settings_generation.wrapping_add(1);
    // The drop of the lock writes the mirror of the settings.
    state.settings_dirty = true;
    drop(state);
    for line in lines {
        handle.push_notice(line);
    }
    (handle.wake)();
    true
}

/// The value of one shared section of this computer's store, or of its defaults
/// when the store holds no row for it.
fn local_section(
    local: &BTreeMap<String, String>,
    defaults: &Config,
    section: &str,
    handle: &SyncHandle,
) -> Option<Value> {
    match local.get(section) {
        Some(text) => match serde_json::from_str(text) {
            Ok(value) => Some(value),
            Err(error) => {
                handle.push_notice(format!(
                    "The setting {section} of this computer cannot be read: {error}"
                ));
                None
            }
        },
        None => settings_sync::body_of(defaults, section).ok().flatten(),
    }
}

/// The value of one section that the server holds, at the configuration version
/// of this build.
fn server_section(section: &str, row: &MirroredSetting) -> Result<Value, SectionFault> {
    let text = serde_json::to_string(&row.body)
        .map_err(|error| SectionFault::Unreadable(format!("the section {section}: {error}")))?;
    let body = settings_sync::decode_body(&text)?;
    settings_sync::arrive(section, &body)
}

/// Applies every section of the mirror that the store of this computer does not
/// hold yet.
fn apply_rows(handle: &SyncHandle, store: &ConfigStore) {
    let (arriving, versions, mut faults) = {
        let state = handle.lock();
        let mut arriving = BTreeMap::new();
        let mut versions = BTreeMap::new();
        let mut faults = BTreeMap::new();
        for (section, row) in &state.settings {
            if !settings_sync::is_shared(section) {
                continue;
            }
            if state.settings_applied.get(section) == Some(&row.version)
                || state.settings_refused.get(section) == Some(&row.version)
            {
                continue;
            }
            match server_section(section, row) {
                Ok(value) => {
                    arriving.insert(section.clone(), value);
                    versions.insert(section.clone(), row.version);
                }
                Err(fault) => {
                    faults.insert(section.clone(), (row.version, fault));
                }
            }
        }
        (arriving, versions, faults)
    };
    let mut applied: BTreeMap<String, u64> = BTreeMap::new();
    if !arriving.is_empty() {
        match settings_sync::apply_to_store(store, &arriving) {
            Ok(_) => applied.extend(versions.iter().map(|(section, version)| (section.clone(), *version))),
            // One section that this build refuses does not keep the others out.
            Err(_) => {
                for (section, value) in &arriving {
                    let one = BTreeMap::from([(section.clone(), value.clone())]);
                    match settings_sync::apply_to_store(store, &one) {
                        Ok(_) => {
                            applied.insert(section.clone(), versions[section]);
                        }
                        Err(fault) => {
                            faults.insert(section.clone(), (versions[section], fault));
                        }
                    }
                }
            }
        }
    }
    if applied.is_empty() && faults.is_empty() {
        return;
    }
    let mut lines = Vec::new();
    for (section, (_, fault)) in &faults {
        lines.push(format!("The setting {section}: {fault}"));
    }
    let mut state = handle.lock();
    state.settings_applied.extend(applied);
    for (section, (version, _)) in &faults {
        state.settings_refused.insert(section.clone(), *version);
    }
    state.settings_refusal = lines.iter().next_back().cloned();
    state.settings_generation = state.settings_generation.wrapping_add(1);
    state.settings_dirty = true;
    drop(state);
    for line in lines {
        handle.push_notice(line);
    }
    (handle.wake)();
}

/// Queues one section with no version, as the first link pushes the value of a
/// section that the server does not have.
fn enqueue_value(state: &mut SyncGuard<'_>, section: &str, config_version: u32, value: &Value) -> Result<(), String> {
    let body = settings_sync::SectionBody {
        format: settings_sync::BODY_FORMAT,
        config_version,
        section: value.clone(),
    };
    let encoded = serde_json::to_value(&body).map_err(|error| error.to_string())?;
    let text = serde_json::to_string(&encoded).map_err(|error| error.to_string())?;
    if text.len() > settings_sync::MAX_SECTION_BYTES {
        return Err(format!(
            "it is larger than {} KB",
            settings_sync::MAX_SECTION_BYTES / 1024
        ));
    }
    let seen = state.settings.get(section).map(|row| row.version);
    state
        .client
        .enqueue(
            seen,
            Op::SettingPut {
                id: section.to_owned(),
                body: encoded,
            },
        )
        .map_err(|error| error.to_string())?;
    state.settings_generation = state.settings_generation.wrapping_add(1);
    state.settings_dirty = true;
    Ok(())
}

/// Writes the one line of a refusal of a section that this computer sent, and
/// reads the row of the version that decided it again.
///
/// The line stays in the window until the person closes it and not for the ten
/// seconds of a short notice: a person who looks away when the server returns
/// would never read which setting took the value of another computer. The log
/// names the section and no value.
pub(super) fn refuse_settings(handle: &SyncHandle, refused: &[String], rewind: Option<u64>) {
    if refused.is_empty() {
        return;
    }
    let line = format!(
        "The server kept the value of another computer for the setting {}, so this computer takes that value.",
        refused.join(", ")
    );
    let mut state = handle.lock();
    if let Some(version) = rewind {
        state.client.replay_before(version);
    }
    drop(state);
    for section in refused {
        tracing::info!(
            section = %section,
            "the server kept the value of another computer for a setting, so this computer takes that value"
        );
    }
    handle.set_settings_lost(line);
    (handle.wake)();
}

/// Writes one sentence about the settings of this computer that this computer
/// could not read, and remembers it for a frame that reads the state.
fn refuse(handle: &SyncHandle, line: String) {
    let mut state = handle.lock();
    state.settings_refusal = Some(line.clone());
    state.settings_generation = state.settings_generation.wrapping_add(1);
    drop(state);
    handle.push_notice(line);
    (handle.wake)();
}
