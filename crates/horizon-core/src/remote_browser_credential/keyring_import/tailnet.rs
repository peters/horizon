//! The OAuth client of a Tailscale network that an older Horizon kept in the
//! keyring, as the three named secrets of the recipe.
//!
//! An older Horizon kept the client of one network under the OS-store service
//! `horizon-cloud-tailnet-api` (`API_KEYCHAIN_SERVICE` of that version), and the
//! enrollment auth key of the same network under `horizon-cloud-tailnets`
//! (`KEYCHAIN_SERVICE`). The account of an item was the opaque slot of the binding,
//! or the saved local id of the network for an entry that a person pasted before
//! the binding existed. The value is a JSON document, and only the OAuth one names
//! the client of a network:
//!
//! ```text
//! {"kind":"oauth","client_id":"<client id>","client_secret":"<client secret>"}
//! ```
//!
//! A personal token is not a client and an enrollment key is not a client either, so
//! this module reads only the OAuth document and counts every other value as an
//! entry that it does not use. It never guesses: the fields, their length and their
//! bytes must be the ones of that version.
//!
//! The network name of the client is not in the keyring. That version kept it in
//! `<home>/cloud/tailnets.json` beside the keyring, in the field `name` of a saved
//! network, and the slot of the client in
//! `<home>/cloud/tailnet-api-bindings.json` when a managed binding replaced the
//! pasted entry. Both files are read here, so the person reads the network name that
//! they saved and not an opaque account.
//!
//! The tag of the client is not in the keyring either: that version asked for the
//! scope `devices:core` and the tag `tag:horizon-worker` in every token request
//! (`crates/horizon-cloud/src/tailnet/api/node/http.rs` of that version), so the
//! client of a saved network is the client of that tag. The recipe states the same
//! rule: the tag of the third secret must be the tag of the OAuth client.

pub(super) mod records;

use std::{collections::BTreeSet, path::Path};

use serde::Deserialize;
use zeroize::Zeroizing;

use super::readers::KeyringEntry;
use crate::named_secrets::SecretName;

pub(super) use records::{SavedNetwork, saved};

/// The tag that the older Horizon asked for in every token request, and therefore
/// the tag of the OAuth client that it stored.
const TAG: &str = "tag:horizon-worker";

/// The most characters of `<NET>`, which the recipe states as the same bound.
pub const MAX_NET_CHARS: usize = 16;

/// The `<NET>` of a network name that holds no letter and no digit.
const FALLBACK_NET: &str = "NET";

/// The three names of one network, in the order of the recipe: the client id, the
/// client secret and the tag.
const SUFFIXES: [&str; 3] = ["CLIENT_ID", "CLIENT_SECRET", "TAG"];

/// The most bytes of one stored client id, and of one stored client secret, as the
/// older version bounded them.
const MAX_CLIENT_ID_BYTES: usize = 256;
const MAX_CLIENT_SECRET_BYTES: usize = 4096;

/// The most bytes of one stored document, as the older version bounded it.
const MAX_STORED_BYTES: usize = 16 * 1024;

/// The OAuth client of one network of the older Horizon, in memory only.
pub(super) struct TailnetClient {
    client_id: Zeroizing<String>,
    client_secret: Zeroizing<String>,
}

impl TailnetClient {
    /// The value of the three names of this client: the client id, the client secret
    /// and the tag of the client.
    pub(super) fn values(&self) -> [Zeroizing<Vec<u8>>; 3] {
        [
            Zeroizing::new(self.client_id.as_bytes().to_vec()),
            Zeroizing::new(self.client_secret.as_bytes().to_vec()),
            Zeroizing::new(TAG.as_bytes().to_vec()),
        ]
    }
}

/// The stored document, exactly as that version wrote it. A field that it did not
/// write, or a field of another type, is not the shape of that version.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Stored {
    kind: String,
    #[serde(default)]
    token: Option<String>,
    #[serde(default)]
    client_id: Option<String>,
    #[serde(default)]
    client_secret: Option<String>,
}

/// Whether `value` is the opaque text that the older version stored in one field.
fn opaque(value: &str, limit: usize) -> bool {
    !value.is_empty() && value.len() <= limit && value.bytes().all(|byte| byte.is_ascii_graphic())
}

/// The OAuth client of one value of the keyring, or `None` when the value is not the
/// document that the older version wrote for an OAuth client. A personal token, an
/// enrollment key, a truncated document and a document with a field that the older
/// version did not write are all `None`.
pub(super) fn client_of(bytes: &[u8]) -> Option<TailnetClient> {
    if bytes.len() > MAX_STORED_BYTES {
        return None;
    }
    let stored: Stored = serde_json::from_slice(bytes).ok()?;
    if stored.kind != "oauth" || stored.token.is_some() {
        return None;
    }
    let client_id = stored.client_id?;
    let client_secret = stored.client_secret?;
    if !opaque(&client_id, MAX_CLIENT_ID_BYTES) || !opaque(&client_secret, MAX_CLIENT_SECRET_BYTES) {
        return None;
    }
    Some(TailnetClient {
        client_id: Zeroizing::new(client_id),
        client_secret: Zeroizing::new(client_secret),
    })
}

/// The `<NET>` of one saved network name: upper case, `A-Z`, `0-9` and `_` only, `_`
/// collapsed, no `_` at either end, and at most [`MAX_NET_CHARS`] characters.
///
/// A name without one letter or digit gives [`FALLBACK_NET`], so a network with a
/// name of marks still reaches three names.
#[must_use]
pub fn net_of(name: &str) -> String {
    let mut net = String::with_capacity(MAX_NET_CHARS);
    for character in name.chars() {
        let mapped = match character {
            'A'..='Z' | 'a'..='z' => character.to_ascii_uppercase(),
            '0'..='9' => character,
            _ => '_',
        };
        if mapped == '_' && (net.is_empty() || net.ends_with('_')) {
            continue;
        }
        net.push(mapped);
    }
    net.truncate(MAX_NET_CHARS);
    let net = net.trim_end_matches('_');
    if net.is_empty() {
        FALLBACK_NET.to_string()
    } else {
        net.to_string()
    }
}

/// The `<NET>` of `base` that no `taken` name holds: the base itself, then the base
/// with `_2`, `_3` and so on, each within [`MAX_NET_CHARS`] characters.
fn free_net(base: &str, taken: &BTreeSet<String>) -> String {
    if !taken.contains(base) {
        return base.to_string();
    }
    for number in 2_usize.. {
        let suffix = format!("_{number}");
        let keep = MAX_NET_CHARS.saturating_sub(suffix.len());
        let stem = base.trim_end_matches('_');
        let stem = stem[..stem.len().min(keep)].trim_end_matches('_');
        let candidate = format!("{stem}{suffix}");
        if !taken.contains(&candidate) {
            return candidate;
        }
    }
    base.to_string()
}

/// The three names of `net`, or `None` when one of them breaks the name rule.
fn three_names(net: &str) -> Option<[SecretName; 3]> {
    let mut names = Vec::with_capacity(SUFFIXES.len());
    for suffix in SUFFIXES {
        names.push(SecretName::parse(&format!("TS_{net}_{suffix}")).ok()?);
    }
    names.try_into().ok()
}

/// One network that the import offers as three named secrets.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TailnetLine {
    /// The `<NET>` of the three names.
    net: String,
    /// The three names, in the order of the recipe.
    names: [SecretName; 3],
    /// Which of the three names this computer already has a value for.
    present: [bool; 3],
}

impl TailnetLine {
    /// One line of the network `net`, with the names that this computer already
    /// holds. `None` when a name of the line breaks the name rule.
    #[must_use]
    pub fn of(net: String, present: [bool; 3]) -> Option<Self> {
        let names = three_names(&net)?;
        Some(Self { net, names, present })
    }

    /// The `<NET>` of this line.
    #[must_use]
    pub fn net(&self) -> &str {
        &self.net
    }

    /// The three names of this line, in the order of the recipe.
    #[must_use]
    pub fn names(&self) -> &[SecretName; 3] {
        &self.names
    }

    /// Which of the three names this computer already has a value for.
    #[must_use]
    pub fn present(&self) -> [bool; 3] {
        self.present
    }

    /// The names that this computer holds no value for, which are the names that a
    /// click on the button creates.
    #[must_use]
    pub fn missing(&self) -> Vec<&SecretName> {
        self.names
            .iter()
            .zip(self.present)
            .filter_map(|(name, present)| (!present).then_some(name))
            .collect()
    }

    /// Whether every name of this line has a value on this computer already, so its
    /// button has nothing to create.
    #[must_use]
    pub fn complete(&self) -> bool {
        self.present.iter().all(|present| *present)
    }
}

/// The networks of the older Horizon whose OAuth client the keyring holds, as the
/// Settings tab offers them. It holds names and never a value.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TailnetOffer {
    lines: Vec<TailnetLine>,
}

impl TailnetOffer {
    /// The offer of these lines, in the order of the saved networks.
    #[must_use]
    pub fn of(lines: Vec<TailnetLine>) -> Self {
        Self { lines }
    }

    /// The networks of the offer, in the order of the saved networks.
    #[must_use]
    pub fn lines(&self) -> &[TailnetLine] {
        &self.lines
    }

    /// Whether the offer names no network.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }
}

/// The client of every saved network that the keyring answers, with the count of
/// entries that name no client of a saved network.
///
/// `entries` is every item of the two tailnet services of the keyring. An entry whose
/// account is the account of a saved network and whose value is the OAuth document
/// of that version is one network. Every other entry is counted, and none of it is
/// guessed into a network.
pub(super) fn clients<'a>(
    networks: &'a [SavedNetwork],
    entries: &[KeyringEntry],
) -> (Vec<(&'a SavedNetwork, TailnetClient)>, usize) {
    let mut clients: Vec<(&SavedNetwork, TailnetClient)> = Vec::new();
    let mut used = vec![false; entries.len()];
    for network in networks {
        for account in network.accounts() {
            let Some((index, _)) = entries
                .iter()
                .enumerate()
                .find(|(index, entry)| entry.user == account && !used[*index])
            else {
                continue;
            };
            let Some(client) = client_of(&entries[index].value) else {
                continue;
            };
            used[index] = true;
            clients.push((network, client));
            break;
        }
    }
    let unused = used.iter().filter(|used| !**used).count();
    (clients, unused)
}

/// One offered network, with the values that a click creates. The values live in the
/// worker thread only, and they are dropped when the offer of the tab is built.
pub(super) struct Offered {
    pub(super) line: TailnetLine,
    values: [Zeroizing<Vec<u8>>; 3],
}

impl Offered {
    /// The value of the name at `index` of this line.
    pub(super) fn value(&self, index: usize) -> Option<&Zeroizing<Vec<u8>>> {
        self.values.get(index)
    }
}

/// The offered lines that `clients` name, with the values of each, and the count of
/// networks that the line rule refused.
///
/// `present` says whether this computer already has a value for a name, so a line
/// states what exists and the button creates what does not.
pub(super) fn offered(
    clients: &[(&SavedNetwork, TailnetClient)],
    mut present: impl FnMut(&SecretName) -> bool,
) -> (Vec<Offered>, usize) {
    let mut lines = Vec::with_capacity(clients.len());
    let mut taken = BTreeSet::new();
    let mut skipped = 0_usize;
    for (network, client) in clients {
        let net = free_net(&net_of(&network.name), &taken);
        let Some(names) = three_names(&net) else {
            skipped += 1;
            continue;
        };
        let presence = names.each_ref().map(&mut present);
        let Some(line) = TailnetLine::of(net.clone(), presence) else {
            skipped += 1;
            continue;
        };
        taken.insert(net);
        lines.push(Offered {
            line,
            values: client.values(),
        });
    }
    (lines, skipped)
}

/// The offer of the tab: the names of `offered`, and never a value.
pub(super) fn offer_of(offered: &[Offered]) -> TailnetOffer {
    TailnetOffer::of(offered.iter().map(|offered| offered.line.clone()).collect())
}

/// The directory of the older Horizon's saved networks, from the home of this build:
/// that version kept them in `cloud` of its home, and this build reads no other.
#[must_use]
pub fn old_store(home: &Path) -> std::path::PathBuf {
    home.join("cloud")
}

#[cfg(test)]
mod tests {
    use super::{FALLBACK_NET, MAX_NET_CHARS, TailnetLine, client_of, free_net, net_of, three_names};
    use crate::named_secrets::SecretName;
    use std::collections::BTreeSet;

    #[test]
    fn the_names_of_a_sixteen_character_network_pass_the_name_rule() {
        let net = net_of("abcdefghijklmnop");
        assert_eq!(net.chars().count(), MAX_NET_CHARS);
        for suffix in ["CLIENT_ID", "CLIENT_SECRET", "TAG"] {
            let name = format!("TS_{net}_{suffix}");
            assert_eq!(
                SecretName::parse(&name).map(|name| name.as_str().to_string()),
                Ok(name.clone()),
                "{name}"
            );
        }
        assert!(three_names(&net).is_some());
    }

    #[test]
    fn every_name_of_the_recipe_passes_the_name_rule() {
        // The three names of the recipe as its examples write them, and three names
        // of a network that a person saved with a name of mixed characters.
        for name in [
            "TS_HOME_CLIENT_ID",
            "TS_HOME_CLIENT_SECRET",
            "TS_HOME_TAG",
            "TS_BUILD_1_CLIENT_ID",
            "TS_203_0_113_10_CLIENT_SECRET",
        ] {
            assert!(SecretName::parse(name).is_ok(), "{name}");
        }
    }

    #[test]
    fn a_saved_name_becomes_a_net_of_upper_case_letters_digits_and_one_underscore() {
        assert_eq!(net_of("Home"), "HOME");
        assert_eq!(net_of("work net"), "WORK_NET");
        assert_eq!(net_of("my-home.ts.net"), "MY_HOME_TS_NET");
        assert_eq!(net_of("a__b"), "A_B");
        assert_eq!(net_of("_leading and trailing_"), "LEADING_AND_TRAI");
        assert_eq!(net_of("!!!"), FALLBACK_NET);
        assert_eq!(net_of(""), FALLBACK_NET);
        assert_eq!(net_of(&"a".repeat(40)).chars().count(), MAX_NET_CHARS);
        assert_eq!(net_of(&format!("{}-", "a".repeat(16))).chars().count(), 16);
    }

    #[test]
    fn a_clash_of_names_takes_a_number_within_the_bound() {
        let mut taken = BTreeSet::new();
        let base = "abcdefghijklmnop";
        let first = free_net(base, &taken);
        assert_eq!(first, base);
        taken.insert(first);
        let second = free_net(base, &taken);
        assert_eq!(second, "abcdefghijklmn_2");
        assert_eq!(second.chars().count(), MAX_NET_CHARS);
        taken.insert(second);
        assert_eq!(free_net(base, &taken), "abcdefghijklmn_3");
        assert_eq!(free_net("HOME", &BTreeSet::from(["HOME".to_string()])), "HOME_2");
    }

    #[test]
    fn only_the_oauth_document_of_the_older_version_is_a_client() {
        let client = client_of(br#"{"kind":"oauth","client_id":"synthetic-id","client_secret":"synthetic-secret"}"#);
        let client = client.expect("the shape of the older version");
        assert_eq!(&*client.values()[0], b"synthetic-id");
        assert_eq!(&*client.values()[1], b"synthetic-secret");
        assert_eq!(&*client.values()[2], b"tag:horizon-worker");
        for value in [
            &br#"{"kind":"personal_token","token":"tskey-api-synthetic"}"#[..],
            br#"{"kind":"oauth","client_id":"synthetic-id"}"#,
            br#"{"kind":"oauth","client_secret":"synthetic-secret"}"#,
            br#"{"kind":"oauth","client_id":"synthetic-id","client_secret":"synthetic-secret","tag":"tag:horizon"}"#,
            br#"{"kind":"oauth","client_id":"","client_secret":"synthetic-secret"}"#,
            br#"{"kind":"oauth","client_id":"synthetic id","client_secret":"synthetic-secret"}"#,
            b"tskey-auth-synthetic12345678901234567890",
            b"",
            b"not json",
        ] {
            assert!(client_of(value).is_none(), "{}", String::from_utf8_lossy(value));
        }
        let long = format!(
            r#"{{"kind":"oauth","client_id":"{}","client_secret":"s"}}"#,
            "i".repeat(257)
        );
        assert!(client_of(long.as_bytes()).is_none(), "a client id over the bound");
        let line_break = br#"{"kind":"oauth","client_id":"synthetic-id","client_secret":"two\nlines"}"#;
        assert!(client_of(line_break).is_none(), "a secret with a line break");
    }

    #[test]
    fn a_line_names_the_three_names_and_the_ones_that_are_here() {
        let line = TailnetLine::of("HOME".to_string(), [true, false, false]).expect("the names of the recipe");
        assert_eq!(line.net(), "HOME");
        assert_eq!(
            line.names().each_ref().map(crate::named_secrets::SecretName::as_str),
            ["TS_HOME_CLIENT_ID", "TS_HOME_CLIENT_SECRET", "TS_HOME_TAG"]
        );
        assert_eq!(
            line.missing().iter().map(|name| name.as_str()).collect::<Vec<_>>(),
            ["TS_HOME_CLIENT_SECRET", "TS_HOME_TAG"]
        );
        assert!(!line.complete());
        assert!(
            TailnetLine::of("HOME".to_string(), [true; 3])
                .expect("the names of the recipe")
                .complete()
        );
    }
}
