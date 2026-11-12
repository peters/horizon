//! The binding of libsecret through `python3`, for a Linux system without
//! `secret-tool`.
//!
//! `secret-tool` is one small program of `libsecret-tools`, and a person who keeps
//! a keyring does not always have that package. The `gi` binding of libsecret
//! reads the same items of the same Secret Service, so this reading asks Python for
//! them and takes one JSON document back: the attributes of every item and its
//! value in base64, because a value is bytes and a JSON text is not.
//!
//! The script is embedded in this binary and runs as `python3 -I -c <script>`: `-I`
//! isolates Python from the folders, the variables and the user site packages of
//! this computer, so the import runs the script of this version and nothing else.

use std::ffi::OsString;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use serde::Deserialize;
use zeroize::Zeroizing;

use super::super::KeyringImportError;
use super::{Bound, KeyringEntry, Platform, ended, failure, mentions};
use crate::bounded_program::{ProgramEnvironment, run};

/// The interpreter that the binding of libsecret runs in.
pub(crate) const PROGRAM: &str = "python3";

/// The exit code of the script when an item answers no value at all, which is not
/// an item that this import can read.
const NO_VALUE: i32 = 4;

/// The script, which prints one JSON document on stdout.
const SCRIPT: &str = r#"
import base64, json, sys

import gi

gi.require_version("Secret", "1")
from gi.repository import Secret

service = Secret.Service.get_sync(Secret.ServiceFlags.LOAD_COLLECTIONS)
flags = Secret.SearchFlags.ALL | Secret.SearchFlags.LOAD_SECRETS | Secret.SearchFlags.UNLOCK
items = service.search_sync({"service": SERVICE_NAME}, flags)
entries = []
for item in items:
    value = item.get_secret()
    if value is None:
        sys.exit(NO_VALUE_CODE)
    attributes = item.get_attributes()
    entries.append({
        "attributes": {name: attributes[name] for name in attributes.keys()},
        "secret": base64.b64encode(bytes(value.get())).decode("ascii"),
    })
json.dump({"entries": entries}, sys.stdout)
"#;

/// The script of this version, with the exact service name in it. A service name
/// of this module holds no quote, so it reaches the script as one Python string.
fn script(service: &str) -> String {
    SCRIPT
        .replace("SERVICE_NAME", &format!("\"{service}\""))
        .replace("NO_VALUE_CODE", &NO_VALUE.to_string())
}

/// The arguments of one run of the script.
fn arguments(service: &str) -> Vec<OsString> {
    ["-I", "-c", &script(service)].map(OsString::from).to_vec()
}

/// The entries of `service`, read through the binding of libsecret.
pub(crate) fn read(
    service: &str,
    environment: &ProgramEnvironment,
    bound: Bound,
) -> Result<Vec<KeyringEntry>, KeyringImportError> {
    let Some(program) = environment.find(PROGRAM) else {
        return Err(KeyringImportError::ToolMissing {
            platform: Platform::SecretService,
        });
    };
    let output = run(&program, &arguments(service), environment, bound.program())
        .map_err(|failed| failure(PROGRAM, failed, bound))?;
    if !output.status.success() {
        if lacks_binding(&output.stderr) {
            // The computer has Python and no binding of libsecret, so the package
            // that reads the keyring is the one that the person must install.
            return Err(KeyringImportError::ToolMissing {
                platform: Platform::SecretService,
            });
        }
        if output.code() == Some(NO_VALUE) {
            return Err(KeyringImportError::Unreadable);
        }
        return Err(ended(PROGRAM, &output));
    }
    parse(&output.stdout)
}

/// Whether Python said that it has no `gi` module.
fn lacks_binding(stderr: &[u8]) -> bool {
    mentions(stderr, b"No module named 'gi'")
}

/// The one JSON document of the script.
#[derive(Deserialize)]
struct Document {
    entries: Vec<JsonEntry>,
}

#[derive(Deserialize)]
struct JsonEntry {
    attributes: std::collections::BTreeMap<String, String>,
    secret: String,
}

fn parse(stdout: &[u8]) -> Result<Vec<KeyringEntry>, KeyringImportError> {
    let document: Document = serde_json::from_slice(stdout).map_err(|_| KeyringImportError::Unreadable)?;
    document.entries.into_iter().map(entry).collect()
}

/// One entry of the document: its account and its decoded value.
fn entry(entry: JsonEntry) -> Result<KeyringEntry, KeyringImportError> {
    let user = entry
        .attributes
        .get("username")
        .cloned()
        .ok_or(KeyringImportError::Unreadable)?;
    let encoded = Zeroizing::new(entry.secret);
    let value = Zeroizing::new(
        STANDARD
            .decode(encoded.as_bytes())
            .map_err(|_| KeyringImportError::Unreadable)?,
    );
    Ok(KeyringEntry { user, value })
}

#[cfg(test)]
mod tests {
    use super::{arguments, parse, script};

    #[test]
    fn the_script_names_the_service_of_the_older_horizon_and_no_value_code_remains() {
        let script = script("horizon-cloud-tailnet-api");
        assert!(script.contains("\"horizon-cloud-tailnet-api\""), "{script}");
        assert!(script.contains("sys.exit(4)"), "{script}");
        assert!(!script.contains("SERVICE_NAME") && !script.contains("NO_VALUE_CODE"));
    }

    #[test]
    fn the_arguments_isolate_python_and_carry_the_script() {
        let arguments = arguments("horizon-remote-browser");
        assert_eq!(arguments[0], "-I");
        assert_eq!(arguments[1], "-c");
        assert!(arguments[2].to_string_lossy().contains("gi.require_version"));
    }

    #[test]
    fn one_document_holds_an_account_and_a_base64_value() {
        let document = br#"{"entries":[{"attributes":{"service":"horizon-remote-browser","username":"https://grid.example.net|remote-browser/grid/key"},"secret":"c3ludGhldGljLXZhbHVl"}]}"#;
        let entries = parse(document).expect("one entry");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].user, "https://grid.example.net|remote-browser/grid/key");
        assert_eq!(&*entries[0].value, b"synthetic-value");
    }

    #[test]
    fn an_entry_without_an_account_is_refused() {
        assert!(parse(br#"{"entries":[{"attributes":{},"secret":"c3ludGhldGlj"}]}"#).is_err());
    }

    #[test]
    fn a_value_that_is_not_base64_is_refused() {
        assert!(parse(br#"{"entries":[{"attributes":{"username":"a"},"secret":"not base64!"}]}"#).is_err());
    }
}
