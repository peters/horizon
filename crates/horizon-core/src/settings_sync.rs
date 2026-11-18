//! The settings that are the same on every computer of one server, and the
//! sections that stay with one computer.
//!
//! The settings store keeps one row for each configuration section. This module
//! is the one place that says whether a section follows the server: [`SECTIONS`]
//! names every section of the store with its scope, and a section that is not in
//! that table is not a section of this build at all. A test reads the table
//! against the rows that the store writes and reads, so a new section without a
//! decision fails it.
//!
//! A shared section travels as one object of the server, its id is the name of
//! the section, and its body is the section inside [`SectionBody`] together with
//! the configuration version that wrote it. The envelope is what makes an older
//! section migrate before it is applied and a newer one stay on the server, and
//! the server never reads either: it stores the JSON and decides the version of
//! the object, not the version of the configuration.
//!
//! Two rules hold here and nowhere else:
//!
//! - A section from the server replaces a local row through the store's own
//!   write path, after the whole merged configuration passes [`Config::validate`].
//!   A section that fails is not applied and not written: a server can never make
//!   a computer run a setting that its own settings could not hold.
//! - A computer never writes back a section in a way that loses a field that it
//!   does not know. [`merged_for_push`] keeps the fields of the server's body that
//!   this build does not have, so a newer Horizon's setting survives an older
//!   Horizon's save.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::config::Config;
use crate::config_migration::CURRENT_CONFIG_VERSION;
use crate::config_store::{self, ConfigStore};
use crate::error::{Error, Result};

/// The largest section that one computer sends and one server stores. A section
/// above it is refused before it reaches a frame, so an oversize setting costs a
/// notice and never the link.
pub const MAX_SECTION_BYTES: usize = 64 * 1024;

/// The longest id of one setting object, and the only characters of it: it is the
/// name of one section of the settings store. A name that this build does not know
/// is shared as well, so the rule is the rule of a section name and not the list of
/// the sections of this build.
const MAX_SETTING_ID_CHARS: usize = 40;

/// Checks the id of one setting object.
///
/// # Errors
/// Returns one sentence for a refusal when the id is not one section name.
pub fn check_setting_id(id: &str) -> std::result::Result<(), String> {
    let valid = !id.is_empty()
        && id.len() <= MAX_SETTING_ID_CHARS
        && id
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_');
    if valid {
        return Ok(());
    }
    Err(format!(
        "The setting id {id:?} is not 1 to {MAX_SETTING_ID_CHARS} characters of a-z, 0-9 and '_'"
    ))
}

/// The format of one [`SectionBody`]. A body of another format is refused
/// instead of guessed.
pub const BODY_FORMAT: u32 = 1;

/// Whether one section of the settings store follows the server.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    /// The section is the same on every computer of one server.
    Shared,
    /// The section stays with this computer, for the reason that it gives.
    Local(&'static str),
}

/// One section of the settings store and its scope.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Section {
    /// The name of the row in the store, and the id of its server object.
    pub name: &'static str,
    pub scope: Scope,
}

/// Every section of the settings store, with the decision about each one.
///
/// The three local sections are local because their value says something about
/// this computer alone: where its window stands, how it reaches the server, and
/// which configuration version its store holds. The machine key bindings and the
/// credential values are not sections of the store; they follow the server by
/// their own rules.
pub const SECTIONS: &[Section] = &[
    Section {
        name: "version",
        scope: Scope::Local("the configuration version of the settings store of this computer"),
    },
    Section {
        name: "window",
        scope: Scope::Local("the size and the position of the window on the screen of this computer"),
    },
    Section {
        name: "appearance",
        scope: Scope::Shared,
    },
    Section {
        name: "shortcuts",
        scope: Scope::Shared,
    },
    Section {
        name: "overlays",
        scope: Scope::Shared,
    },
    Section {
        name: "canvas",
        scope: Scope::Shared,
    },
    Section {
        name: "features",
        scope: Scope::Shared,
    },
    Section {
        name: "browser",
        scope: Scope::Shared,
    },
    Section {
        name: "remote_hosts",
        scope: Scope::Shared,
    },
    Section {
        name: "browser_host",
        scope: Scope::Shared,
    },
    Section {
        name: "server",
        scope: Scope::Local("how this computer reaches the server"),
    },
    Section {
        name: "secrets",
        scope: Scope::Shared,
    },
    Section {
        name: "machines",
        scope: Scope::Local("whether this computer watches the agents of its machine list"),
    },
    Section {
        name: "speech",
        scope: Scope::Shared,
    },
    Section {
        name: "presets",
        scope: Scope::Shared,
    },
    Section {
        name: "workspaces",
        scope: Scope::Shared,
    },
];

/// The rule of one section, or `None` when no section of this build has that
/// name. A section that this build does not know follows the server as well, so
/// [`Scope::Shared`] is the answer for it: it is kept and never deleted, and an
/// older computer does not drop it.
#[must_use]
pub fn scope(name: &str) -> Option<Scope> {
    SECTIONS
        .iter()
        .find(|section| section.name == name)
        .map(|section| section.scope)
}

/// Whether `name` is one section of this build.
#[must_use]
pub fn is_section(name: &str) -> bool {
    SECTIONS.iter().any(|section| section.name == name)
}

/// Whether one section follows the server. A name that this build does not know
/// follows it.
#[must_use]
pub fn is_shared(name: &str) -> bool {
    !matches!(scope(name), Some(Scope::Local(_)))
}

/// The names of the shared sections of this build, in the order of the table.
pub fn shared_sections() -> impl Iterator<Item = &'static str> {
    SECTIONS
        .iter()
        .filter(|section| section.scope == Scope::Shared)
        .map(|section| section.name)
}

/// The body of one server object: the section of one configuration, and the
/// version of that configuration.
///
/// The server stores this value and never reads it. The computer that receives it
/// migrates a section of an older configuration version and refuses one of a
/// newer version, which is what the version is for.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SectionBody {
    /// [`BODY_FORMAT`] of the computer that wrote the body.
    pub format: u32,
    /// The configuration version of the computer that wrote the body.
    pub config_version: u32,
    /// The JSON of the section.
    pub section: Value,
}

/// Why one section of the server is not applied on this computer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SectionFault {
    /// The body is not one section body of this build, or its section is not
    /// JSON that this build reads.
    Unreadable(String),
    /// The body comes from a configuration version that this Horizon does not
    /// know. It stays on the server and this computer keeps its own row.
    NewerHorizon {
        /// The configuration version of the body.
        found: u32,
        /// The newest version that this build reads.
        expected: u32,
    },
    /// The section is readable, and the configuration that it makes together with
    /// the other sections does not pass the rule of this build.
    Refused(String),
    /// The store of this computer could not be read or written.
    Store(String),
}

impl std::fmt::Display for SectionFault {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unreadable(reason) => write!(formatter, "a setting of the server cannot be read: {reason}"),
            Self::NewerHorizon { found, expected } => write!(
                formatter,
                "a setting of the server comes from a newer Horizon, with configuration version {found}, and this \
                 Horizon reads up to version {expected}: this computer keeps its own setting until Horizon is updated"
            ),
            Self::Refused(reason) => write!(
                formatter,
                "a setting of the server is not one that this Horizon accepts: {reason}"
            ),
            Self::Store(reason) => write!(formatter, "the settings of this computer: {reason}"),
        }
    }
}

/// The fields of a shared section that stay with this computer, and the section of
/// each one: they say how this computer runs its programs, so the server never
/// carries them.
const LOCAL_FIELDS: &[(&str, &str)] = &[
    ("features", "keep_agents_through_restart"),
    ("features", "when_window_closes"),
];

/// `value` of the section `name` with its local-only fields taken from `row`, or with them taken out when `row` is none.
#[must_use]
pub fn local_fields(name: &str, value: &Value, row: Option<&Value>) -> Value {
    let Value::Object(fields) = value else {
        return value.clone();
    };
    let mut result = fields.clone();
    for (_, field) in LOCAL_FIELDS.iter().filter(|(section, _)| *section == name) {
        match row.and_then(|row| row.get(*field)) {
            Some(local) => result.insert((*field).to_owned(), local.clone()),
            None => result.remove(*field),
        };
    }
    Value::Object(result)
}

/// The JSON value of one section of `config` as the server carries it, or `None`
/// when no section of this build has that name.
///
/// # Errors
/// Returns why the section cannot be encoded.
pub fn body_of(config: &Config, name: &str) -> Result<Option<Value>> {
    let value = config_store::section_value(config, name)?;
    Ok(value.map(|value| local_fields(name, &value, None)))
}

/// The body of the server object of one section of `config`, or `None` when the
/// section stays with this computer.
///
/// # Errors
/// Returns why the section cannot be encoded.
pub fn encode_body(config: &Config, name: &str) -> Result<Option<String>> {
    if !is_shared(name) {
        return Ok(None);
    }
    let Some(section) = body_of(config, name)? else {
        return Ok(None);
    };
    let body = SectionBody {
        format: BODY_FORMAT,
        config_version: config.version,
        section,
    };
    serde_json::to_string(&body)
        .map(Some)
        .map_err(|error| Error::Config(format!("the section {name} of the settings: {error}")))
}

/// One [`SectionBody`] of the text that a server gave.
///
/// # Errors
/// Refuses a text that is not JSON, a body of another format, and a body whose
/// section this build cannot read.
pub fn decode_body(text: &str) -> std::result::Result<SectionBody, SectionFault> {
    let body: SectionBody = serde_json::from_str(text)
        .map_err(|error| SectionFault::Unreadable(format!("it is not one setting: {error}")))?;
    if body.format != BODY_FORMAT {
        return Err(SectionFault::Unreadable(format!(
            "it has format {} and this Horizon writes format {BODY_FORMAT}",
            body.format
        )));
    }
    Ok(body)
}

/// One [`SectionBody`] of the JSON value that a server stores.
///
/// The mirror of a link keeps the body of the server as a JSON value, so a caller
/// that merges it reads it back through this function and never treats the value
/// as a section.
///
/// # Errors
/// Refuses a value that is not one body of this build.
pub fn decode_body_value(value: &Value) -> std::result::Result<SectionBody, SectionFault> {
    let text = serde_json::to_string(value)
        .map_err(|error| SectionFault::Unreadable(format!("it is not one setting: {error}")))?;
    decode_body(&text)
}

/// The section of one body, at the configuration version of this build.
///
/// A section of an older configuration version is migrated with the migrations of
/// this build, and a section of a newer version keeps its body on the server:
/// this computer reads no configuration version that it does not know.
///
/// # Errors
/// Refuses a body of a newer Horizon and a section that the migration cannot read.
pub fn arrive(name: &str, body: &SectionBody) -> std::result::Result<Value, SectionFault> {
    if body.config_version > CURRENT_CONFIG_VERSION {
        return Err(SectionFault::NewerHorizon {
            found: body.config_version,
            expected: CURRENT_CONFIG_VERSION,
        });
    }
    if body.config_version == CURRENT_CONFIG_VERSION {
        return Ok(body.section.clone());
    }
    // An older section is migrated inside a configuration of the defaults: the
    // migration of this build reads a whole configuration, and the other sections
    // of that value are not taken from it.
    let mut document = serde_json::to_value(Config::default())
        .map_err(|error| SectionFault::Unreadable(format!("this Horizon cannot encode its defaults: {error}")))?;
    let Some(fields) = document.as_object_mut() else {
        return Err(SectionFault::Unreadable("the defaults are not one object".to_owned()));
    };
    fields.insert(name.to_owned(), body.section.clone());
    fields.insert("version".to_owned(), Value::from(body.config_version));
    let mut config: Config = serde_json::from_value(document)
        .map_err(|error| SectionFault::Refused(format!("it does not read as a section of {name}: {error}")))?;
    crate::config_migration::migrate_in_memory(&mut config)
        .map_err(|error| SectionFault::Refused(error.to_string()))?;
    let migrated = serde_json::to_value(&config)
        .map_err(|error| SectionFault::Unreadable(format!("this Horizon cannot encode its settings: {error}")))?;
    Ok(migrated.get(name).cloned().unwrap_or(Value::Null))
}

/// `config` with the section `name` replaced by `value`.
///
/// # Errors
/// Returns why the section cannot be read or encoded.
pub fn with_section(config: &Config, name: &str, value: &Value) -> Result<Config> {
    if !is_section(name) {
        return Err(Error::Config(format!(
            "the section {name} is not a section of this build"
        )));
    }
    let mut document = serde_json::to_value(config)
        .map_err(|error| Error::Config(format!("the settings of this computer: {error}")))?;
    let Some(fields) = document.as_object_mut() else {
        return Err(Error::Config(
            "the settings of this computer are not one object".to_owned(),
        ));
    };
    fields.insert(name.to_owned(), value.clone());
    let text = serde_json::to_string(&document)
        .map_err(|error| Error::Config(format!("the settings of this computer: {error}")))?;
    let text = crate::retired_panel_kinds::without_retired(&text);
    serde_json::from_str(text.as_ref()).map_err(|error| {
        Error::Config(format!(
            "the section {name} of the server does not read as a section of this build: {error}"
        ))
    })
}

/// The configuration of `rows` with `arriving` in place of their rows, checked
/// with the rule of this build.
///
/// A row of the store that is not a section of this build stays in the map: the
/// merged configuration reads it as an unknown section, which is what the store
/// itself does.
///
/// # Errors
/// Returns a fault when an arriving section does not read, or when the merged
/// configuration does not pass [`Config::validate`].
pub fn merged_config(
    rows: &BTreeMap<String, String>,
    arriving: &BTreeMap<String, Value>,
) -> std::result::Result<Config, SectionFault> {
    let mut merged = rows.clone();
    for (name, value) in arriving {
        let text = serde_json::to_string(value)
            .map_err(|error| SectionFault::Unreadable(format!("the section {name}: {error}")))?;
        merged.insert(name.clone(), text);
    }
    let config = config_store::decode_rows(&merged).map_err(|error| SectionFault::Refused(error.to_string()))?;
    config
        .validate()
        .map_err(|error| SectionFault::Refused(error.to_string()))?;
    Ok(config)
}

/// The value of `local` that this computer sends: the fields of the server's
/// section that this build does not know stay under the values of this build.
///
/// The argument is the body of the server and not its section, so a caller
/// cannot merge the envelope of a body as if it were a section: the envelope is
/// `format`, `config_version` and `section`, and a merge of it would put those
/// three fields inside the section of the next body.
///
/// Only the top level of a section is merged. A section is one value of the
/// settings, so a field of it that a newer Horizon added is at that level, and a
/// nested merge would decide which of two lists is the newer one.
#[must_use]
pub fn merged_for_push(server: Option<&SectionBody>, local: &Value) -> Value {
    let (Some(SectionBody { section: server, .. }), Value::Object(local)) = (server, local) else {
        return local.clone();
    };
    let Value::Object(server) = server else {
        return Value::Object(local.clone());
    };
    let mut merged = server.clone();
    for (field, value) in local {
        merged.insert(field.clone(), value.clone());
    }
    Value::Object(merged)
}

/// Whether the server's body says exactly what this computer would send of the
/// same section, so the first link has nothing to compare and nothing to write.
///
/// A field that one side has and the other does not is a difference: a newer
/// Horizon's field is what the rule of "no setting is lost" is about, and a field
/// that this computer has is a local value that the server does not hold.
#[must_use]
pub fn covers(server: &Value, local: &Value) -> bool {
    server == local
}

/// The shared sections whose value differs between `before` and `after`.
///
/// A local section is never part of the answer, so no local section can become an
/// intent.
#[must_use]
pub fn changed_sections(before: &Config, after: &Config) -> Vec<String> {
    shared_sections()
        .filter(|name| body_of(before, name).ok() != body_of(after, name).ok())
        .map(str::to_owned)
        .collect()
}

/// Writes the arriving sections into the store of `store` and answers the
/// configuration that the store holds afterwards.
///
/// The whole merged configuration passes the rule of this build before one byte is
/// written, and only the arriving rows are written: the settings of this computer
/// that the server did not change stay exactly as they are, unknown sections
/// included. The arriving row is written as the JSON that the server gave with the
/// fields of that section that stay with this computer, so a field of a newer
/// Horizon stays in the store and in the next push.
///
/// # Errors
/// Returns a fault when a section does not read, when the merged configuration
/// does not validate, or when the store cannot be read or written.
pub fn apply_to_store(
    store: &ConfigStore,
    arriving: &BTreeMap<String, Value>,
) -> std::result::Result<Config, SectionFault> {
    if arriving.is_empty() {
        return read_store(store);
    }
    let rows = store
        .section_rows()
        .map_err(|error| SectionFault::Store(error.to_string()))?;
    let mut kept = BTreeMap::new();
    for (name, value) in arriving {
        let row = rows.get(name).and_then(|text| serde_json::from_str::<Value>(text).ok());
        kept.insert(name.clone(), local_fields(name, value, row.as_ref()));
    }
    let config = merged_config(&rows, &kept)?;
    let mut writes = BTreeMap::new();
    for (name, value) in &kept {
        let text = serde_json::to_string(value)
            .map_err(|error| SectionFault::Unreadable(format!("the section {name}: {error}")))?;
        writes.insert(name.clone(), text);
    }
    store
        .write_sections(&writes)
        .map_err(|error| SectionFault::Store(error.to_string()))?;
    Ok(config)
}

/// The configuration that the store of `store` holds, or the defaults when it
/// holds none.
///
/// # Errors
/// Returns why the store cannot be read or its settings cannot be read.
pub fn read_store(store: &ConfigStore) -> std::result::Result<Config, SectionFault> {
    let rows = store
        .section_rows()
        .map_err(|error| SectionFault::Store(error.to_string()))?;
    if rows.is_empty() {
        return Ok(Config::default());
    }
    let config = config_store::decode_rows(&rows).map_err(|error| SectionFault::Refused(error.to_string()))?;
    config
        .validate()
        .map_err(|error| SectionFault::Refused(error.to_string()))?;
    Ok(config)
}

/// The file that keeps the settings of this computer from before its first link
/// with the server, beside the store and with the time of that link in its name.
#[must_use]
pub fn backup_path(store: &ConfigStore, millis: u64) -> PathBuf {
    let name = store
        .path()
        .file_stem()
        .and_then(std::ffi::OsStr::to_str)
        .unwrap_or("settings");
    store
        .path()
        .with_file_name(format!("{name}.before-server.{millis}.json"))
}

/// Writes the sections of this computer that the server replaced into `path`, at
/// the mode of a private file.
///
/// # Errors
/// Returns why the file cannot be written.
pub fn write_backup(path: &Path, sections: &BTreeMap<String, String>) -> std::io::Result<()> {
    let encoded = serde_json::to_vec_pretty(sections)?;
    crate::AtomicFile::new(path.to_path_buf()).write(&encoded)
}

/// The names of the sections that the server had and this computer did not
/// include, or `None` when the server's body says nothing that `local` misses.
#[must_use]
pub fn unknown_fields(server: &Value, local: &Value) -> Option<Vec<String>> {
    let (Value::Object(server), Value::Object(local)) = (server, local) else {
        return None;
    };
    let unknown: Vec<String> = server
        .keys()
        .filter(|field| !local.contains_key(*field))
        .cloned()
        .collect();
    (!unknown.is_empty()).then_some(unknown)
}

#[cfg(test)]
mod tests;
