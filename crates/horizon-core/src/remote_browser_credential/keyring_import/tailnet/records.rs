//! The networks that an older Horizon saved, so the keyring account of a client has
//! a name that the person recognizes.
//!
//! That version kept the catalog of its networks in `tailnets.json` of the `cloud`
//! folder of its home: one `{"id","name"}` row for each saved network, and the slot
//! of its enrollment key in `slots`. The desktop API credential of a network had a
//! second file, `tailnet-api-bindings.json`, which names the slot of the client when
//! a managed binding replaced a pasted entry.
//!
//! Both files are untrusted text: they are the person's own files, which any program
//! of the same user can write. This module reads a bounded document with the exact
//! fields of that version, and it refuses a document that it cannot read instead of
//! showing a name that the file did not hold. A missing file is not a refusal: a
//! person who never used that version has no such folder, and then the import offers
//! no network.

use std::{collections::BTreeMap, fs::File, io::Read, path::Path};

use serde::Deserialize;

use super::super::KeyringImportError;

/// The catalog of saved networks, as that version wrote it.
const CATALOG: &str = "tailnets.json";

/// The slots of the desktop API clients, as that version wrote it.
const BINDINGS: &str = "tailnet-api-bindings.json";

/// The most bytes of one document that this import reads.
const MAX_DOCUMENT: u64 = 64 * 1024;

/// The most networks that one offer shows, so a written file cannot grow a frame.
const MAX_NETWORKS: usize = 64;

/// The saved networks of the older Horizon, and the accounts of their enrollment
/// keys, which the Keychain of macOS reads one at a time.
#[derive(Debug, Default, Eq, PartialEq)]
pub(crate) struct SavedNetworks {
    pub(super) networks: Vec<SavedNetwork>,
}

impl SavedNetworks {
    /// The networks in the order of the older catalog.
    #[must_use]
    pub(crate) fn networks(&self) -> &[SavedNetwork] {
        &self.networks
    }

    /// The accounts that can hold the OAuth client of a network, in the order of the
    /// older catalog: the slot of its API binding, or the saved id of the network.
    #[must_use]
    pub(crate) fn api_accounts(&self) -> Vec<&str> {
        self.networks.iter().map(|network| network.account.as_str()).collect()
    }

    /// The accounts of the enrollment keys, which the Keychain of macOS reads one at
    /// a time because a search of the whole service costs one question per item.
    #[must_use]
    pub(crate) fn enrollment_accounts(&self) -> Vec<&str> {
        self.networks
            .iter()
            .filter_map(|network| network.enrollment.as_deref())
            .collect()
    }
}

/// One saved network whose keyring account can hold the OAuth client of the network.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SavedNetwork {
    /// The account of the older Horizon's item that holds the client: the slot of a
    /// managed binding, or the saved local id of the network.
    pub(crate) account: String,
    /// The account of the enrollment key of the same network, which the older
    /// version's other service can hold as well.
    pub(crate) enrollment: Option<String>,
    /// The name that the person saved for the network.
    pub(crate) name: String,
}

impl SavedNetwork {
    /// The accounts that can hold the OAuth client of this network, most specific
    /// first.
    pub(crate) fn accounts(&self) -> impl Iterator<Item = &str> {
        std::iter::once(self.account.as_str())
            .chain(self.enrollment.as_deref())
            .filter(|account| !account.is_empty())
    }
}

/// The catalog document of that version.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Catalog {
    #[serde(default)]
    tailnets: Vec<SavedRow>,
    #[serde(default)]
    slots: BTreeMap<String, String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedRow {
    id: String,
    name: String,
}

/// The binding document of that version.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Bindings {
    #[serde(default)]
    bindings: BTreeMap<String, Binding>,
}

#[derive(Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
enum Binding {
    Active { slot: String, kind: Kind },
    Disabled,
}

#[derive(Clone, Copy, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
enum Kind {
    PersonalToken,
    #[serde(rename = "oauth")]
    OAuth,
}

/// The saved networks of the older Horizon under `cloud`, in the order of its
/// catalog, with the account that holds the OAuth client of each.
///
/// A network whose API credential is disabled, or whose saved credential is a
/// personal token, has no OAuth client to offer and is not in the answer.
pub(crate) fn saved(cloud: &Path) -> Result<SavedNetworks, KeyringImportError> {
    let Some(catalog): Option<Catalog> = read(&cloud.join(CATALOG))? else {
        return Ok(SavedNetworks::default());
    };
    if catalog.tailnets.len() > MAX_NETWORKS {
        return Err(KeyringImportError::OldNetworks);
    }
    let bindings: Bindings = read(&cloud.join(BINDINGS))?.unwrap_or(Bindings {
        bindings: BTreeMap::new(),
    });
    let mut networks = Vec::with_capacity(catalog.tailnets.len());
    let mut ids = std::collections::BTreeSet::new();
    for network in catalog.tailnets {
        if !valid_id(&network.id) || !valid_name(&network.name) || !ids.insert(network.id.clone()) {
            return Err(KeyringImportError::OldNetworks);
        }
        let enrollment = match catalog.slots.get(&network.id) {
            Some(slot) if !valid_id(slot) => return Err(KeyringImportError::OldNetworks),
            Some(slot) => Some(slot.clone()),
            None => None,
        };
        let account = match bindings.bindings.get(&network.id) {
            Some(Binding::Active { slot, kind }) => (*kind == Kind::OAuth).then(|| slot.clone()),
            Some(Binding::Disabled) => None,
            None => Some(network.id.clone()),
        };
        if let Some(account) = account {
            if !valid_id(&account) {
                return Err(KeyringImportError::OldNetworks);
            }
            networks.push(SavedNetwork {
                account,
                enrollment,
                name: network.name.trim().to_string(),
            });
        }
    }
    Ok(SavedNetworks { networks })
}

/// The one file of `path`, or `None` when this computer has no such file.
fn read<T: serde::de::DeserializeOwned>(path: &Path) -> Result<Option<T>, KeyringImportError> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(KeyringImportError::OldNetworks),
    };
    let mut bytes = Vec::new();
    file.take(MAX_DOCUMENT + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| KeyringImportError::OldNetworks)?;
    if bytes.len() as u64 > MAX_DOCUMENT {
        return Err(KeyringImportError::OldNetworks);
    }
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|_| KeyringImportError::OldNetworks)
}

/// Whether `value` is the identifier that the older version accepted for a network
/// and for the slot of its credential.
fn valid_id(value: &str) -> bool {
    !value.is_empty() && value.len() <= 64 && value.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

/// Whether `value` is a name that the older version accepted.
fn valid_name(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= 80 && !value.chars().any(char::is_control)
}

#[cfg(test)]
mod tests {
    use super::{SavedNetwork, SavedNetworks, saved};
    use crate::remote_browser_credential::KeyringImportError;
    use std::fs;

    fn write(cloud: &std::path::Path, name: &str, body: &str) {
        fs::create_dir_all(cloud).expect("the folder of the older Horizon");
        fs::write(cloud.join(name), body).expect("the file of the older Horizon");
    }

    #[test]
    fn a_staged_client_is_named_by_the_saved_network() {
        let temp = tempfile::tempdir().expect("a folder");
        write(
            temp.path(),
            "tailnets.json",
            r#"{"tailnets":[{"id":"saved-id","name":"Work"}],"slots":{"saved-id":"key-slot"}}"#,
        );
        assert_eq!(
            saved(temp.path()).expect("the saved network"),
            SavedNetworks {
                networks: vec![SavedNetwork {
                    account: "saved-id".to_string(),
                    enrollment: Some("key-slot".to_string()),
                    name: "Work".to_string(),
                }],
            }
        );
    }

    #[test]
    fn a_managed_client_is_named_by_its_slot_and_each_kind_is_read() {
        let temp = tempfile::tempdir().expect("a folder");
        write(
            temp.path(),
            "tailnets.json",
            r#"{"tailnets":[{"id":"oauth-id","name":"Home"},{"id":"token-id","name":"Build"},{"id":"gone-id","name":"Gone"}]}"#,
        );
        write(
            temp.path(),
            "tailnet-api-bindings.json",
            r#"{"bindings":{
                "oauth-id":{"state":"active","slot":"oauth-slot","kind":"oauth"},
                "token-id":{"state":"active","slot":"token-slot","kind":"personal_token"},
                "gone-id":{"state":"disabled"}}}"#,
        );
        assert_eq!(
            saved(temp.path()).expect("the saved network").networks,
            [SavedNetwork {
                account: "oauth-slot".to_string(),
                enrollment: None,
                name: "Home".to_string(),
            }],
            "a personal token and a disabled credential hold no OAuth client"
        );
    }

    #[test]
    fn no_folder_is_no_network_and_a_broken_document_is_a_refusal() {
        let temp = tempfile::tempdir().expect("a folder");
        assert!(
            saved(&temp.path().join("absent"))
                .expect("no network")
                .networks
                .is_empty()
        );
        write(temp.path(), "tailnets.json", r#"{"tailnets":[{"id":"a","name":"","}]}"#);
        assert_eq!(saved(temp.path()), Err(KeyringImportError::OldNetworks));
        write(
            temp.path(),
            "tailnets.json",
            r#"{"tailnets":[{"id":"a","name":"Work","auth_key":"tskey-auth-synthetic"}]}"#,
        );
        assert_eq!(saved(temp.path()), Err(KeyringImportError::OldNetworks));
        write(
            temp.path(),
            "tailnets.json",
            r#"{"tailnets":[{"id":"../a","name":"Work"}]}"#,
        );
        assert_eq!(saved(temp.path()), Err(KeyringImportError::OldNetworks));
        write(
            temp.path(),
            "tailnets.json",
            r#"{"tailnets":[{"id":"a","name":"Work"},{"id":"a","name":"Other"}]}"#,
        );
        assert_eq!(saved(temp.path()), Err(KeyringImportError::OldNetworks));
        write(
            temp.path(),
            "tailnets.json",
            r#"{"tailnets":[{"id":"a","name":"Work"}]}"#,
        );
        write(temp.path(), "tailnet-api-bindings.json", r#"{"bindings":[]}"#);
        assert_eq!(saved(temp.path()), Err(KeyringImportError::OldNetworks));
    }
}
