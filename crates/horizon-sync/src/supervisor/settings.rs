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
//! 3. **A refusal keeps the winning value, and only a different value is a loss.**
//!    A change of this computer on a version that another computer decided first is
//!    refused with the version that decided it, the row of that version comes back,
//!    and this computer takes it. That row decides whether the value of this
//!    computer differs from it at all: a push that lost against the same content is
//!    no loss, so it writes no row, keeps no backup and reads no line. A line names
//!    the settings whose content differed.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use horizon_core::config_store::ConfigStore;
use horizon_core::settings_sync::{self, SectionFault};
use horizon_core::{Config, MachineError};
use serde_json::Value;

use super::SyncHandle;
use super::pull::now_millis;
use super::state::{SettingsSyncView, SyncGuard, SyncState};
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
        match server_section(section, &row.body) {
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
    pin_refused(&mut state, &faults);
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
        Some(text) => match serde_json::from_str::<Value>(text) {
            Ok(value) => Some(settings_sync::local_fields(section, &value, None)),
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
fn server_section(section: &str, body: &Value) -> Result<Value, SectionFault> {
    settings_sync::arrive(section, &settings_sync::decode_body_value(body)?)
}

/// The value of one section in the rows of a store, or the default of this build.
fn store_section(rows: &BTreeMap<String, String>, section: &str, defaults: &Config) -> Option<Value> {
    rows.get(section)
        .and_then(|text| serde_json::from_str::<Value>(text).ok())
        // A field that stays with this computer is in no body of the server, so the
        // two values are compared without it.
        .map(|value| settings_sync::local_fields(section, &value, None))
        .or_else(|| settings_sync::body_of(defaults, section).ok().flatten())
}

/// Applies every section of the mirror that the store of this computer does not
/// hold yet.
///
/// The row that decided a refused push is applied here like any other row, and it
/// answers the one question that a refusal cannot answer: whether the value of this
/// computer differs from the value of the server. A row with the same content is no
/// change of this computer, so this computer takes its version, writes no row and
/// keeps its own value; a row with another content replaces the row and names its
/// section in one line for the person.
fn apply_rows(handle: &SyncHandle, store: &ConfigStore) {
    let arriving = {
        let state = handle.lock();
        mirror_rows(&state)
    };
    let (mut applied, write_faults) = write_rows(store, &arriving);
    let mut faults = arriving.faults;
    faults.extend(write_faults);
    if applied.is_empty() && faults.is_empty() && arriving.decided.is_empty() {
        return;
    }
    let mut lines = Vec::new();
    for (section, (_, fault)) in &faults {
        lines.push(format!("The setting {section}: {fault}"));
    }
    if !arriving.lost.is_empty() {
        // The line is written before the state that carries the version of these rows is
        // published, so a frame or a test that reads the settings as in step reads the
        // line of the value that lost too. A value that this computer kept until now is
        // not in the store any more, and the line names it: the person finds it in the
        // value of the server.
        for section in &arriving.lost {
            tracing::info!(
                section = %section,
                "the server kept the value of another computer for a setting, so this computer takes that value"
            );
        }
        handle.set_settings_lost(lost_line(&arriving.lost));
    }
    let mut state = handle.lock();
    // A row of the server can arrive between the read of the mirror that this write was
    // built from and the write itself, so the store can hold a row older than the mirror
    // after the write. That version is no version of this store, and the worker writes
    // the newer row with one more pass: a store that stayed behind a row of the mirror
    // would send the value of that older row with the version of the newer one.
    let behind: Vec<String> = applied
        .keys()
        .filter(|section| {
            state
                .settings
                .get(*section)
                .is_some_and(|row| row.version > applied[*section])
        })
        .cloned()
        .collect();
    for section in &behind {
        applied.remove(section);
        state.settings_applied.remove(section);
    }
    let behind = !behind.is_empty();
    state.settings_applied.extend(applied);
    pin_refused(&mut state, &faults);
    // Every row that arrived decided its refusal, so no section waits for one.
    for section in &arriving.decided {
        state.settings_lost_pending.remove(section);
    }
    state.settings_refusal = lines.iter().next_back().cloned();
    state.settings_generation = state.settings_generation.wrapping_add(1);
    state.settings_dirty = true;
    drop(state);
    for line in lines {
        handle.push_notice(line);
    }
    if behind {
        handle.apply_store();
    }
    (handle.wake)();
}

/// The rows of the mirror that the store may not hold, the refusals that they decide,
/// and the sections whose value of the person lost.
struct Arriving {
    values: BTreeMap<String, Value>,
    versions: BTreeMap<String, u64>,
    faults: BTreeMap<String, (u64, SectionFault)>,
    decided: BTreeSet<String>,
    lost: Vec<String>,
}

fn mirror_rows(state: &SyncState) -> Arriving {
    let mut arriving = Arriving {
        values: BTreeMap::new(),
        versions: BTreeMap::new(),
        faults: BTreeMap::new(),
        decided: BTreeSet::new(),
        lost: Vec::new(),
    };
    for (section, row) in &state.settings {
        if !settings_sync::is_shared(section) {
            continue;
        }
        // This row is the one that a refusal named, so its content decides whether this
        // computer lost a value at all. A row that this computer applied before the
        // refusal came is decided here too: the answer that carries the row of a refused
        // version can arrive before the refusal itself, and a section that waits for a
        // row that no answer brings again would leave the person without the line that
        // says that a value lost.
        if let Some((refused_at, pushed)) = state.settings_lost_pending.get(section)
            && row.version >= *refused_at
        {
            let held = server_section(section, &row.body);
            let pushed = server_section(section, pushed);
            arriving.decided.insert(section.clone());
            if !matches!((held, pushed), (Ok(held), Ok(pushed)) if settings_sync::covers(&held, &pushed)) {
                arriving.lost.push(section.clone());
            }
        }
        if state.settings_applied.get(section) == Some(&row.version)
            || state.settings_refused.get(section) == Some(&row.version)
        {
            continue;
        }
        match server_section(section, &row.body) {
            Ok(value) => {
                arriving.values.insert(section.clone(), value);
                arriving.versions.insert(section.clone(), row.version);
            }
            Err(fault) => {
                arriving.faults.insert(section.clone(), (row.version, fault));
            }
        }
    }
    arriving
}

/// Writes the rows of `arriving` that the store does not hold, and answers the version
/// of each row that the store holds now and the sections that this build refused. A
/// section whose content is the content of this computer takes the version of the
/// server and writes no row, so no editor offers a value that it already holds.
fn write_rows(
    store: &ConfigStore,
    arriving: &Arriving,
) -> (BTreeMap<String, u64>, BTreeMap<String, (u64, SectionFault)>) {
    let local = store.section_rows().unwrap_or_default();
    let defaults = Config::default();
    let mut applied: BTreeMap<String, u64> = BTreeMap::new();
    let mut faults: BTreeMap<String, (u64, SectionFault)> = BTreeMap::new();
    let mut writing: BTreeMap<String, Value> = BTreeMap::new();
    for (section, value) in &arriving.values {
        let own = store_section(&local, section, &defaults);
        if own.is_some_and(|own| settings_sync::covers(value, &own)) {
            applied.insert(section.clone(), arriving.versions[section]);
            continue;
        }
        writing.insert(section.clone(), value.clone());
    }
    if writing.is_empty() {
        return (applied, faults);
    }
    match settings_sync::apply_to_store(store, &writing) {
        Ok(_) => applied.extend(
            arriving
                .versions
                .iter()
                .map(|(section, version)| (section.clone(), *version)),
        ),
        // One section that this build refuses does not keep the others out.
        Err(_) => {
            for (section, value) in &writing {
                let one = BTreeMap::from([(section.clone(), value.clone())]);
                match settings_sync::apply_to_store(store, &one) {
                    Ok(_) => {
                        applied.insert(section.clone(), arriving.versions[section]);
                    }
                    Err(fault) => {
                        faults.insert(section.clone(), (arriving.versions[section], fault));
                    }
                }
            }
        }
    }
    (applied, faults)
}

/// Pins the sections whose content this build refused, so one refusal reads one
/// notice. A store that failed once is no refusal: the next answer writes it again.
fn pin_refused(state: &mut SyncState, faults: &BTreeMap<String, (u64, SectionFault)>) {
    for (section, (version, fault)) in faults {
        if !matches!(fault, SectionFault::Store(_)) {
            state.settings_refused.insert(section.clone(), *version);
        }
    }
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

/// One section whose push the server refused, with the version that decided the
/// refusal. A refusal without a version decided the section without a row.
pub(super) type RefusedSection = (String, Option<u64>, Value);

/// Writes the line about the settings whose value this computer lost to another
/// computer: one sentence, and the one setting or the several that it names.
fn lost_line(sections: &[String]) -> String {
    let named = if let [only] = sections {
        format!("the setting {only}")
    } else {
        format!("the settings {}", sections.join(", "))
    };
    format!("The server kept the value of another computer for {named}, so this computer takes that value.")
}

/// Remembers the sections whose push the server refused, and moves the mark of this
/// computer back before the version that a refusal names, so the next answer of
/// changes brings the row that decided it.
///
/// The refusal decides nothing by itself: the row of the server that follows says
/// whether the value of this computer differs from it at all. A push that lost
/// against the same content therefore keeps its value, writes no row, keeps no
/// backup and reads no line. A refusal that names no version decided the section
/// without a row, so the line follows at once.
///
/// The line stays in the window until the person closes it and not for the ten
/// seconds of a short notice: a person who looks away when the server returns
/// would never read which setting took the value of another computer. The log
/// names the section and no value.
pub(super) fn refuse_settings(handle: &SyncHandle, refused: &[RefusedSection], rewind: Option<u64>) {
    if refused.is_empty() {
        return;
    }
    let mut decided_now: Vec<String> = Vec::new();
    let mut state = handle.lock();
    if let Some(version) = rewind {
        state.client.replay_before(version);
    }
    for (section, version, pushed) in refused {
        match version {
            Some(version) => {
                state
                    .settings_lost_pending
                    .insert(section.clone(), (*version, pushed.clone()));
            }
            None => decided_now.push(section.clone()),
        }
    }
    drop(state);
    for (section, _, _) in refused {
        tracing::info!(section = %section, "the server refused a setting of this computer");
    }
    if !decided_now.is_empty() {
        handle.set_settings_lost(lost_line(&decided_now));
    }
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
