//! The rules of a named secret: its name, its value, and what one computer keeps
//! of it in a secrets file in memory.

use std::sync::{Arc, Mutex};

use super::super::{
    MAX_NAMED_SECRETS, MAX_VALUE_BYTES, NamedSecretError, NamedSecretState, NamedSecretVault, SecretName,
};
use crate::remote_browser_credential::{RemoteCredentialError, SharedCredentialStore};
use crate::secret_store::{SecretStore, UnreadableStore};

const MARKER: &str = "MARKER-9c41-named-value";

fn name(text: &str) -> SecretName {
    SecretName::parse(text).expect("a name of a named secret")
}

/// The published values are one map of the process, so a test that publishes
/// takes this guard and starts from no secret.
fn exclusive() -> super::super::environment::Exclusive {
    super::super::environment::exclusive()
}

fn held(vault: &NamedSecretVault, name: &SecretName) -> Vec<u8> {
    vault
        .value(name)
        .expect("the store answers")
        .expect("a value on this computer")
        .to_vec()
}

fn vault() -> (NamedSecretVault, Arc<Mutex<Arc<SecretStore>>>) {
    let store = Arc::new(Mutex::new(SecretStore::in_memory().expect("a store in memory")));
    let vault = NamedSecretVault::new(Box::new(SharedCredentialStore::new(Arc::clone(&store))));
    (vault, store)
}

#[test]
fn a_name_is_an_environment_variable_name_of_upper_case_letters() {
    for accepted in ["A", "AWS_ACCESS_KEY_ID", "A1", "HETZNER_TOKEN_2"] {
        assert_eq!(name(accepted).as_str(), accepted);
    }
    for refused in ["", "a", "_LEADING", "1LEADING", "with-dash", "with space", "lower_CASE"] {
        assert!(
            matches!(SecretName::parse(refused), Err(NamedSecretError::InvalidName { .. })),
            "{refused:?} is not a name"
        );
    }
    let longest = "A".repeat(super::super::MAX_NAME_CHARS);
    assert!(SecretName::parse(&longest).is_ok(), "64 characters are a name");
    assert!(
        matches!(
            SecretName::parse(&format!("{longest}A")),
            Err(NamedSecretError::InvalidName { .. })
        ),
        "65 characters are not a name"
    );
}

#[test]
fn the_two_names_of_the_mobile_device_cloud_are_names_of_the_person() {
    // `horizon-mobile` reads both values from the environment of its agent panel as
    // `BROWSERSTACK_USERNAME` and `BROWSERSTACK_ACCESS_KEY`, so both names must pass
    // the rule of a name. Neither name shares a reserved prefix.
    for accepted in ["BROWSERSTACK_USERNAME", "BROWSERSTACK_ACCESS_KEY"] {
        assert_eq!(
            name(accepted).as_str(),
            accepted,
            "{accepted} is a name that a person stores in Settings, Secrets"
        );
    }
}

#[test]
fn every_name_of_horizon_the_agents_and_the_system_is_refused() {
    let reserved = [
        "PATH",
        "HOME",
        "USER",
        "SHELL",
        "TERM",
        "LANG",
        "LD_PRELOAD",
        "DYLD_INSERT_LIBRARIES",
        "SSH_AUTH_SOCK",
        "HORIZON",
        "HORIZON_HOME",
        "CLAUDE_CODE_DISABLE_ALTERNATE_SCREEN",
        "CODEX_HOME",
        "ANTHROPIC_API_KEY",
        "OPENAI_API_KEY",
        "XDG_RUNTIME_DIR",
        "DISPLAY",
        "WAYLAND_DISPLAY",
        "DBUS_SESSION_BUS_ADDRESS",
        "TMPDIR",
    ];
    for reserved in reserved {
        assert!(
            matches!(SecretName::parse(reserved), Err(NamedSecretError::ReservedName { .. })),
            "{reserved} belongs to Horizon or to the system"
        );
    }
    // A name that only shares a prefix of a reserved word is a name of the person.
    assert!(SecretName::parse("HOMES").is_ok(), "HOMES is not HOME");
    assert!(SecretName::parse("TERMINAL_KEY").is_ok(), "TERMINAL_KEY is not TERM");
    assert!(SecretName::parse("LANGUAGE_KEY").is_ok(), "LANGUAGE_KEY is not LANG");
    assert_eq!(
        NamedSecretError::ReservedName {
            name: "HOME".to_owned()
        }
        .code(),
        "reserved_secret_name"
    );
}

#[test]
fn a_value_is_text_of_one_to_sixteen_kibibytes() {
    let name = name("HETZNER_TOKEN");
    assert!(super::super::check_value(&name, b"x").is_ok(), "one byte is a value");
    assert!(
        super::super::check_value(&name, MARKER.as_bytes()).is_ok(),
        "text is a value"
    );
    assert!(
        super::super::check_value(&name, &vec![b'k'; MAX_VALUE_BYTES]).is_ok(),
        "16 KiB is a value"
    );
    assert_eq!(
        super::super::check_value(&name, b""),
        Err(NamedSecretError::InvalidValue {
            name: "HETZNER_TOKEN".to_owned()
        })
    );
    assert_eq!(
        super::super::check_value(&name, &vec![b'k'; MAX_VALUE_BYTES + 1]),
        Err(NamedSecretError::InvalidValue {
            name: "HETZNER_TOKEN".to_owned()
        })
    );
    for refused in [&b"two\nlines"[..], b"two\rlines", b"nul\0byte", b"\xff\xfe"] {
        assert_eq!(
            super::super::check_value(&name, refused),
            Err(NamedSecretError::ValueNotText {
                name: "HETZNER_TOKEN".to_owned()
            }),
            "{refused:?} is not the text of a variable"
        );
    }
    // A refusal names the secret and never quotes its value.
    let refusal = super::super::check_value(&name, b"secret\nvalue")
        .unwrap_err()
        .to_string();
    assert!(!refusal.contains("secret"), "a refusal holds no value: {refusal}");
}

#[test]
fn a_value_is_stored_replaced_and_removed_through_the_vault() {
    let _exclusive = exclusive();
    let (vault, store) = vault();
    let token = name("HETZNER_TOKEN");
    vault.set_names(["HETZNER_TOKEN".to_owned()]);
    assert_eq!(vault.state(&token), Ok(NamedSecretState::NoValue), "no value yet");

    let change = vault.put(&token, MARKER.as_bytes()).expect("the value");
    assert!(change.stored);
    assert_eq!(vault.state(&token), Ok(NamedSecretState::Present));
    assert_eq!(held(&vault, &token), MARKER.as_bytes());
    assert_eq!(
        store.lock().expect("the store").ids().expect("the ids").len(),
        1,
        "one item of the store"
    );
    assert_eq!(vault.local_ids(), vec!["named/HETZNER_TOKEN".to_owned()]);

    vault.put(&token, b"second-value").expect("the replacement");
    assert_eq!(held(&vault, &token), b"second-value");
    assert_eq!(
        store.lock().expect("the store").ids().expect("the ids").len(),
        1,
        "a replacement keeps one item"
    );

    let removed = vault.remove(&token).expect("the removal");
    assert!(!removed.stored);
    assert_eq!(vault.state(&token), Ok(NamedSecretState::NoValue));
    assert!(
        store.lock().expect("the store").ids().expect("the ids").is_empty(),
        "the item is gone"
    );
    assert!(vault.names().is_empty(), "the name goes with the value");
    // A name that this computer does not hold is already removed.
    vault.remove(&token).expect("a second removal");
}

#[test]
fn the_names_of_the_settings_are_the_only_names_that_are_kept() {
    let (vault, _store) = vault();
    vault.set_names([
        "GOOD_NAME".to_owned(),
        "HOME".to_owned(),
        "bad_name".to_owned(),
        "GOOD_NAME".to_owned(),
    ]);
    assert_eq!(vault.names(), vec![name("GOOD_NAME")], "one good name, once");
}

#[test]
fn this_computer_keeps_at_most_sixty_four_named_secrets() {
    let (vault, _store) = vault();
    let many: Vec<String> = (0..=MAX_NAMED_SECRETS).map(|index| format!("KEY_{index}")).collect();
    vault.set_names(many.clone());
    assert_eq!(vault.names().len(), MAX_NAMED_SECRETS);

    let mut refused = None;
    for text in &many {
        let name = name(text);
        if let Err(error) = vault.put(&name, b"value") {
            refused = Some(error);
        }
    }
    // The list holds 64 names, so every one of them takes a value; the 65th name
    // never entered the list.
    assert!(refused.is_none(), "no name of the list is refused");
    assert_eq!(vault.local_ids().len(), MAX_NAMED_SECRETS);
    assert_eq!(vault.names().len(), MAX_NAMED_SECRETS, "the limit holds");
}

#[test]
fn a_value_is_published_for_the_next_agent_panel_of_this_computer() {
    let _exclusive = exclusive();
    let (vault, _store) = vault();
    let token = name("HETZNER_TOKEN");
    vault.set_names(["HETZNER_TOKEN".to_owned()]);
    vault.put(&token, MARKER.as_bytes()).expect("the value");
    assert_eq!(super::super::published_names(), vec![token.clone()]);

    let mut environment = std::collections::HashMap::new();
    super::super::apply(&mut environment);
    assert_eq!(environment.get("HETZNER_TOKEN").map(String::as_str), Some(MARKER));

    vault.remove(&token).expect("the removal");
    let mut environment = std::collections::HashMap::new();
    super::super::apply(&mut environment);
    assert!(environment.is_empty(), "a removed value is in no panel");
}

#[test]
fn a_refresh_reads_the_store_and_publishes_only_the_values_that_are_there() {
    let _exclusive = exclusive();
    let (vault, store) = vault();
    vault.set_names(["WITH_VALUE".to_owned(), "WITHOUT_VALUE".to_owned()]);
    // A value of an earlier run of this computer, stored before the names were
    // read: the refresh is what publishes it.
    store
        .lock()
        .expect("the store")
        .write("named/WITH_VALUE", MARKER.as_bytes())
        .expect("the value");

    let present = vault.refresh().expect("the refresh");
    assert_eq!(present, vec![name("WITH_VALUE")]);
    assert_eq!(super::super::published_names(), vec![name("WITH_VALUE")]);
    assert_eq!(vault.local_ids(), vec!["named/WITH_VALUE".to_owned()]);
}

#[test]
fn a_secrets_file_that_cannot_be_read_is_one_typed_refusal_and_no_value() {
    let _exclusive = exclusive();
    let vault = NamedSecretVault::new(Box::new(UnreadableStore::new(RemoteCredentialError::File {
        path: std::path::PathBuf::from("/home/person/.horizon/secrets.sqlite"),
        reason: "it is a directory".to_owned(),
    })));
    let token = name("HETZNER_TOKEN");
    vault.set_names(["HETZNER_TOKEN".to_owned()]);
    assert!(
        matches!(vault.value(&token), Err(NamedSecretError::Unavailable { .. })),
        "a file that cannot be read is a refusal and not a value"
    );
    let refused = vault.put(&token, MARKER.as_bytes()).expect_err("the file");
    assert_eq!(refused.code(), "secret_store_unavailable");
    assert!(
        refused.to_string().contains("secrets.sqlite"),
        "the refusal names the file: {refused}"
    );
    assert!(matches!(vault.state(&token), Err(NamedSecretError::Unavailable { .. })));
    assert!(matches!(vault.refresh(), Err(NamedSecretError::Unavailable { .. })));
    assert!(vault.local_ids().is_empty(), "no value, so no secret travels");
}

#[test]
fn the_store_address_of_a_named_secret_is_its_reference_under_an_origin_of_horizon() {
    let locator = super::super::locator(&name("HETZNER_TOKEN"));
    assert_eq!(locator.origin, super::super::NAMED_SECRET_ORIGIN);
    assert_eq!(locator.slot.as_deref(), Some("named/HETZNER_TOKEN"));
    assert_eq!(locator.reference.as_str(), "named/HETZNER_TOKEN");
}
