//! The OAuth client of a Tailscale network that an older Horizon kept in the
//! keyring, and the three named secrets that a click creates from it.
//!
//! Every reading of these tests is a stand-in program of a private folder, so no
//! test opens the keyring of the person who runs them. The saved networks are files
//! of a temporary folder, because that is where the older version kept them.

use std::fmt::Write as _;
use std::sync::{Arc, Mutex};

use super::super::{
    Bound, KeyringImport, KeyringImportError, KeyringImportOutcome, KeyringImportPlan, Platform, Tailnets,
    create_tailnets, import_from_keyring,
};
use super::stand_in::StandIns;
use crate::named_secrets::{NamedSecretVault, SecretName};
use crate::remote_browser_credential::{RemoteCredentialStore, SharedStore};
use crate::secret_store::SecretStore;

/// The OAuth client that the older version stored, with values that no test prints.
const CLIENT: &str = r#"{"kind":"oauth","client_id":"synthetic-client-id","client_secret":"synthetic-client-secret"}"#;

/// The three values of the client above, in the order of the recipe.
const VALUES: [&str; 3] = ["synthetic-client-id", "synthetic-client-secret", "tag:horizon-worker"];

fn bound() -> Bound {
    Bound::WholeImport(std::time::Duration::from_secs(10))
}

/// The three names of one network, in the order of the recipe.
fn names(net: &str) -> [SecretName; 3] {
    ["CLIENT_ID", "CLIENT_SECRET", "TAG"].map(|suffix| {
        let text = format!("TS_{net}_{suffix}");
        SecretName::parse(&text).unwrap_or_else(|error| panic!("{text}: {error}"))
    })
}

/// Write `tailnets.json` and, when it is not empty, `tailnet-api-bindings.json` into
/// `cloud`, in the shape of the older version.
fn saved(cloud: &std::path::Path, networks: &[(&str, &str)], slots: &[(&str, &str)], bindings: &str) {
    let tailnets: Vec<String> = networks
        .iter()
        .map(|(id, name)| format!(r#"{{"id":"{id}","name":"{name}"}}"#))
        .collect();
    let slots: Vec<String> = slots.iter().map(|(id, slot)| format!(r#""{id}":"{slot}""#)).collect();
    std::fs::create_dir_all(cloud).expect("the folder of the older Horizon");
    std::fs::write(
        cloud.join("tailnets.json"),
        format!(
            r#"{{"tailnets":[{}],"slots":{{{}}}}}"#,
            tailnets.join(","),
            slots.join(",")
        ),
    )
    .expect("the catalog of the older Horizon");
    if !bindings.is_empty() {
        std::fs::write(cloud.join("tailnet-api-bindings.json"), bindings).expect("the bindings of the older Horizon");
    }
}

/// The stand-in of the Secret Service or of the Keychain, which answers the items of
/// `items`: one (service, account, value) row of each.
fn keyring(stand_ins: &StandIns, platform: Platform, items: &[(&str, &str, &str)]) {
    let (name, script) = match platform {
        Platform::SecretService => {
            let mut services: Vec<&str> = Vec::new();
            for (service, _, _) in items {
                if !services.contains(service) {
                    services.push(service);
                }
            }
            let mut arms = String::new();
            for service in services {
                writeln!(arms, "  {service})").expect("a script");
                for (item_service, account, value) in items.iter().filter(|item| item.0 == service) {
                    write!(
                        arms,
                        "    printf '%s\\n' '[/org/freedesktop/secrets/collection/login/7]'\n\
                         printf '%s\\n' 'label = keyring'\n\
                         printf 'secret = %s\\n' '{value}'\n\
                         printf '%s\\n' 'created = '\n\
                         printf '%s\\n' 'modified = '\n\
                         printf '%s\\n' 'attribute.service = {item_service}' >&2\n\
                         printf '%s\\n' 'attribute.username = {account}' >&2\n"
                    )
                    .expect("a script");
                }
                arms.push_str("    ;;\n");
            }
            // The search names the service as its fifth argument, and a service that
            // this stand-in does not name answers no item.
            (
                "secret-tool",
                format!("#!/bin/sh\ncase \"$5\" in\n{arms}  *) exit 0 ;;\nesac\n"),
            )
        }
        Platform::Keychain => {
            let mut arms = String::new();
            for (service, account, value) in items {
                write!(
                    arms,
                    "  {service},{account})\n    printf '%s\\n' '{value}'\n    exit 0\n    ;;\n"
                )
                .expect("a script");
            }
            // The reading names the service as its third argument and the account as
            // its fifth, and another item is the status of "no such item".
            (
                "security",
                format!("#!/bin/sh\ncase \"$3,$5\" in\n{arms}  *) exit 44 ;;\nesac\n"),
            )
        }
    };
    stand_ins.program(name, &script);
}

/// The secrets file of one test, the named secrets over it, and the handle that the
/// import of the remote browser rows writes through.
fn vault() -> (Arc<SecretStore>, Arc<NamedSecretVault>, SharedStore) {
    let values = SecretStore::in_memory().expect("a secrets file in memory");
    let vault = Arc::new(NamedSecretVault::new(Box::new(Arc::clone(&values))));
    let shared: SharedStore = Arc::new(Mutex::new(
        Box::new(Arc::clone(&values)) as Box<dyn RemoteCredentialStore + Send>
    ));
    (values, vault, shared)
}

/// One scan of the older Horizon's networks, with no remote browser reference of
/// these settings.
fn scan(
    platform: Platform,
    stand_ins: &StandIns,
    cloud: &std::path::Path,
    vault: Arc<NamedSecretVault>,
    shared: &SharedStore,
) -> Result<KeyringImportOutcome, KeyringImportError> {
    let tailnets = Tailnets::new(cloud.to_path_buf(), vault);
    import_from_keyring(
        platform,
        &KeyringImportPlan::of(&horizon_browser::remote::RemoteBrowserConfig::default()),
        &stand_ins.environment(),
        bound(),
        shared,
        Some(&tailnets),
    )
}

/// The value of `name` as bytes, for a check that a value reached the store.
fn value(vault: &NamedSecretVault, name: &SecretName) -> Option<Vec<u8>> {
    vault
        .value(name)
        .expect("the value of the secret")
        .map(|value| value.to_vec())
}

#[test]
fn the_network_of_one_saved_client_becomes_the_three_names_of_the_recipe() {
    for platform in [Platform::SecretService, Platform::Keychain] {
        one_network(platform);
    }
}

fn one_network(platform: Platform) {
    let stand_ins = StandIns::new();
    let temp = tempfile::tempdir().expect("a folder");
    let cloud = temp.path().join("cloud");
    saved(
        &cloud,
        &[("saved-id", "Home")],
        &[],
        r#"{"bindings":{"saved-id":{"state":"active","slot":"client-slot","kind":"oauth"}}}"#,
    );
    keyring(
        &stand_ins,
        platform,
        &[("horizon-cloud-tailnet-api", "client-slot", CLIENT)],
    );
    let (_values, vault, shared) = vault();

    let outcome = scan(platform, &stand_ins, &cloud, Arc::clone(&vault), &shared).expect("the offer of the network");
    let offer = outcome.offer.as_ref().expect("the offer of the scan");
    assert_eq!(offer.lines().len(), 1, "{platform:?}");
    let line = &offer.lines()[0];
    assert_eq!(line.net(), "HOME", "{platform:?}");
    assert_eq!(
        line.names().each_ref().map(SecretName::as_str),
        ["TS_HOME_CLIENT_ID", "TS_HOME_CLIENT_SECRET", "TS_HOME_TAG"],
        "{platform:?}"
    );
    assert_eq!(line.present(), [false; 3], "no value is on this computer yet");
    assert_eq!(outcome.created, Vec::new(), "a scan writes no named secret");

    let created = create_tailnets(
        platform,
        &stand_ins.environment(),
        bound(),
        &Tailnets::new(cloud.clone(), Arc::clone(&vault)),
        Some("HOME"),
    )
    .expect("the three secrets of the network");
    assert_eq!(created.created.len(), 3, "{platform:?}");
    for ((name, text), value) in names("HOME")
        .iter()
        .zip(VALUES)
        .zip(names("HOME").each_ref().map(|name| value(&vault, name)))
    {
        assert_eq!(value, Some(text.as_bytes().to_vec()), "{name}");
    }
}

#[test]
fn the_two_networks_of_two_saved_clients_keep_their_own_names() {
    for platform in [Platform::SecretService, Platform::Keychain] {
        two_networks(platform);
    }
}

fn two_networks(platform: Platform) {
    let stand_ins = StandIns::new();
    let temp = tempfile::tempdir().expect("a folder");
    let cloud = temp.path().join("cloud");
    // The first client is a pasted entry at the saved id, the second one sits at its
    // own slot: both shapes of the older version meet here.
    saved(
        &cloud,
        &[("home-id", "Home"), ("work-id", "Work")],
        &[("home-id", "home-key-slot")],
        "",
    );
    keyring(
        &stand_ins,
        platform,
        &[
            ("horizon-cloud-tailnet-api", "home-id", CLIENT),
            ("horizon-cloud-tailnet-api", "work-id", CLIENT),
            // The enrollment key of the first network is not a client, so it is read
            // and counted and never offered.
            (
                "horizon-cloud-tailnets",
                "home-key-slot",
                "tskey-auth-synthetic12345678901234567890",
            ),
        ],
    );
    let (_values, vault, shared) = vault();
    let outcome = scan(platform, &stand_ins, &cloud, Arc::clone(&vault), &shared).expect("the offer of both networks");
    let offer = outcome.offer.as_ref().expect("the offer of the scan");
    assert_eq!(
        offer.lines().iter().map(super::super::TailnetLine::net).collect::<Vec<_>>(),
        ["HOME", "WORK"],
        "{platform:?}"
    );
    assert_eq!(
        offer.lines()[1].names().each_ref().map(SecretName::as_str),
        ["TS_WORK_CLIENT_ID", "TS_WORK_CLIENT_SECRET", "TS_WORK_TAG"],
        "{platform:?}"
    );
    assert_eq!(
        outcome.report.unused, 1,
        "the enrollment key names no OAuth client of a network: {platform:?}"
    );
}

#[test]
fn a_client_under_the_enrollment_service_is_the_same_network() {
    for platform in [Platform::SecretService, Platform::Keychain] {
        let stand_ins = StandIns::new();
        let temp = tempfile::tempdir().expect("a folder");
        let cloud = temp.path().join("cloud");
        saved(&cloud, &[("saved-id", "Home")], &[("saved-id", "key-slot")], "");
        keyring(&stand_ins, platform, &[("horizon-cloud-tailnets", "key-slot", CLIENT)]);
        let (_values, vault, shared) = vault();
        let outcome =
            scan(platform, &stand_ins, &cloud, Arc::clone(&vault), &shared).expect("the offer of the network");
        let offer = outcome.offer.as_ref().expect("the offer of the scan");
        assert_eq!(offer.lines().len(), 1, "{platform:?}");
        assert_eq!(offer.lines()[0].net(), "HOME", "{platform:?}");
    }
}

#[test]
fn two_names_that_become_one_net_take_a_number() {
    let stand_ins = StandIns::new();
    let temp = tempfile::tempdir().expect("a folder");
    let cloud = temp.path().join("cloud");
    saved(&cloud, &[("first-id", "Home"), ("second-id", "home")], &[], "");
    keyring(
        &stand_ins,
        Platform::SecretService,
        &[
            ("horizon-cloud-tailnet-api", "first-id", CLIENT),
            ("horizon-cloud-tailnet-api", "second-id", CLIENT),
        ],
    );
    let (_values, vault, shared) = vault();
    let outcome =
        scan(Platform::SecretService, &stand_ins, &cloud, Arc::clone(&vault), &shared).expect("the offer of the names");
    let offer = outcome.offer.as_ref().expect("the offer of the scan");
    assert_eq!(
        offer.lines().iter().map(super::super::TailnetLine::net).collect::<Vec<_>>(),
        ["HOME", "HOME_2"]
    );
    assert_eq!(
        offer.lines()[1].names().each_ref().map(SecretName::as_str),
        ["TS_HOME_2_CLIENT_ID", "TS_HOME_2_CLIENT_SECRET", "TS_HOME_2_TAG"]
    );
}

#[test]
fn a_value_that_is_not_the_shape_of_the_older_version_is_counted_and_not_guessed() {
    let stand_ins = StandIns::new();
    let temp = tempfile::tempdir().expect("a folder");
    let cloud = temp.path().join("cloud");
    saved(&cloud, &[("saved-id", "Home")], &[], "");
    keyring(
        &stand_ins,
        Platform::SecretService,
        &[
            // A personal token, a plain enrollment key and a document with a field
            // that the older version never wrote.
            (
                "horizon-cloud-tailnet-api",
                "saved-id",
                r#"{"kind":"personal_token","token":"tskey-api-synthetic"}"#,
            ),
            (
                "horizon-cloud-tailnets",
                "key-slot",
                "tskey-auth-synthetic12345678901234567890",
            ),
            (
                "horizon-cloud-tailnet-api",
                "other-slot",
                r#"{"kind":"oauth","client_id":"synthetic-client-id","client_secret":"synthetic-client-secret","tag":"tag:horizon"}"#,
            ),
        ],
    );
    let (_values, vault, shared) = vault();
    let outcome =
        scan(Platform::SecretService, &stand_ins, &cloud, Arc::clone(&vault), &shared).expect("the offer of no client");
    let offer = outcome.offer.as_ref().expect("the offer of the scan");
    assert!(offer.is_empty(), "no value of the wrong shape becomes a client");
    assert_eq!(outcome.report.unused, 3, "every entry is counted");
    let created = create_tailnets(
        Platform::SecretService,
        &stand_ins.environment(),
        bound(),
        &Tailnets::new(cloud, Arc::clone(&vault)),
        None,
    )
    .expect("no network to create");
    assert_eq!(created.created, Vec::new());
    assert_eq!(created.created_report.created, 0);
}

#[test]
fn a_name_that_this_computer_holds_is_never_written_over() {
    let stand_ins = StandIns::new();
    let temp = tempfile::tempdir().expect("a folder");
    let cloud = temp.path().join("cloud");
    saved(&cloud, &[("saved-id", "Home")], &[], "");
    keyring(
        &stand_ins,
        Platform::SecretService,
        &[("horizon-cloud-tailnet-api", "saved-id", CLIENT)],
    );
    let (_values, vault, shared) = vault();
    let typed = names("HOME")[0].clone();
    vault
        .put(&typed, b"synthetic-typed")
        .expect("the value that a person typed");

    let outcome = scan(Platform::SecretService, &stand_ins, &cloud, Arc::clone(&vault), &shared).expect("the offer");
    let line = &outcome.offer.as_ref().expect("the offer of the scan").lines()[0];
    assert_eq!(line.present(), [true, false, false], "the line says which name exists");
    assert_eq!(
        line.missing().iter().map(|name| name.as_str()).collect::<Vec<_>>(),
        ["TS_HOME_CLIENT_SECRET", "TS_HOME_TAG"]
    );

    let created = create_tailnets(
        Platform::SecretService,
        &stand_ins.environment(),
        bound(),
        &Tailnets::new(cloud, Arc::clone(&vault)),
        None,
    )
    .expect("the two missing secrets");
    assert_eq!(created.created_report.created, 2);
    assert_eq!(created.created_report.already_here, 1);
    assert_eq!(
        value(&vault, &typed),
        Some(b"synthetic-typed".to_vec()),
        "a value that is here stays"
    );
    assert_eq!(value(&vault, &names("HOME")[2]), Some(b"tag:horizon-worker".to_vec()));
}

#[test]
fn the_button_of_a_network_reaches_the_line_of_its_click() {
    let stand_ins = StandIns::new();
    let temp = tempfile::tempdir().expect("a folder");
    let cloud = temp.path().join("cloud");
    saved(&cloud, &[("saved-id", "Home")], &[], "");
    keyring(
        &stand_ins,
        Platform::SecretService,
        &[("horizon-cloud-tailnet-api", "saved-id", CLIENT)],
    );
    let (_values, vault, shared) = vault();
    let mut import =
        KeyringImport::with_programs(Platform::SecretService, stand_ins.environment(), bound()).with_old(cloud);

    import.start(
        KeyringImportPlan::of(&horizon_browser::remote::RemoteBrowserConfig::default()),
        Arc::clone(&shared),
        Some(Arc::clone(&vault)),
    );
    answered(&mut import);
    assert_eq!(import.line().map(|line| line.text.as_str()), Some("Imported 0 values."));
    assert!(import.offer().is_some_and(|offer| offer.lines().len() == 1));
    assert!(import.secrets_line().is_none(), "no named secret is created yet");

    import.start_secrets(Some("HOME".to_string()), Arc::clone(&vault));
    answered(&mut import);
    assert_eq!(
        import.secrets_line().map(|line| line.text.as_str()),
        Some("Created 3 secrets of the network.")
    );
    assert_eq!(import.take_created().len(), 3);
    assert_eq!(import.take_created(), Vec::new(), "the window takes each change once");
}

#[test]
fn no_answer_of_the_import_holds_a_value() {
    let stand_ins = StandIns::new();
    let temp = tempfile::tempdir().expect("a folder");
    let cloud = temp.path().join("cloud");
    saved(&cloud, &[("saved-id", "Home")], &[], "");
    keyring(
        &stand_ins,
        Platform::SecretService,
        &[("horizon-cloud-tailnet-api", "saved-id", CLIENT)],
    );
    let (_values, vault, shared) = vault();
    let outcome = scan(Platform::SecretService, &stand_ins, &cloud, Arc::clone(&vault), &shared).expect("the offer");
    let created = create_tailnets(
        Platform::SecretService,
        &stand_ins.environment(),
        bound(),
        &Tailnets::new(cloud, Arc::clone(&vault)),
        None,
    )
    .expect("the three secrets");

    let mut drawn = vec![
        format!("{outcome:?}"),
        format!("{}", outcome.report),
        format!("{}", outcome.created_report),
        format!("{}", created.created_report),
        format!("{:?}", created.created),
        format!("{:?}", created.offer),
        format!("{:?}", created.changes),
    ];
    for offer in [&outcome.offer, &created.offer].into_iter().flatten() {
        drawn.push(format!("{offer:?}"));
        for line in offer.lines() {
            drawn.push(format!("{line:?}"));
            drawn.push(line.net().to_string());
            drawn.push(format!("{:?}", line.names()));
            drawn.push(format!("{:?}", line.missing()));
        }
    }
    for text in drawn {
        for secret in ["synthetic-client-id", "synthetic-client-secret"] {
            assert!(!text.contains(secret), "an answer holds a value: {text}");
        }
    }
}

/// The worker of one import, until its answer is on the line of the button.
fn answered(import: &mut KeyringImport) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while import.is_busy() && std::time::Instant::now() < deadline {
        import.poll();
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(!import.is_busy(), "the worker answered");
}
