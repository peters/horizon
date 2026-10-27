//! The managed ssh entry of the browser VM, in a temporary home of a test.
//!
//! The scan of the host keys runs a stand-in `ssh-keyscan`, so no test reaches a machine. The
//! last test asks the real `ssh` what the written configuration resolves to, because that is
//! the reader the entry exists for.
use std::os::unix::fs::PermissionsExt;
use std::process::Stdio;

use super::*;

const ADDRESS: &str = "203.0.113.10";
const OTHER: &str = "198.51.100.7";
const IDENTITY: &str = "/home/person/.horizon/cloud/identity";
const KEY: &str = "AAAAC3NzaC1lZDI1NTE5AAAAIC3gDh/ATgAW1C2pZQvrgzjlkoHoJffEJkJ3dvWAkIdK";
const RSA: &str = "AAAAB3NzaC1yc2EAAAADAQABAAABAQDcHE4OpYC6aSxV4xST6/PtOp2qGbFrLUiZvgBzpPJPuxl/6+qWn1tcwx2mQs88XGPPE3LghL2j7V3KA2MzCh5iCa6QUb2grsboyuvCPWvFZMBMtoFeYDHPiu7qqolxdL1yHK2HUi/w/3X4gt5GKCNDE4E20rB4YsKmmfLcQUOJtUUcfTw5bQUTx47yjcl5Myun+O8vadOCypOhNWmYC8EVz9z2U2p+Q5VM5F9sP+K02Gvodzvcq449tpqGjKQLPJvwitsG8J5CKy1JQ7RvxwtmgHgdCUUnJcRYQfUWH13UZeQPXuUYIJrm+0hzDuuhjoScimTkcxuZ1i9xL7CgMromn";
/// The hosts and the comment of a person, which the entry must leave as they were.
const PERSON: &str =
    "Host laptop\n  HostName 192.0.2.44\n  User person\n\n# the office machine\nHost office\n  HostName 192.0.2.45\n";

/// A home of a person at `folder`, with the ssh configuration that `config` names.
fn home(config: Option<&str>) -> tempfile::TempDir {
    let home = tempfile::tempdir().expect("a temporary home");
    if let Some(config) = config {
        private_folder(&home.path().join(".ssh")).expect("the ssh folder");
        std::fs::write(home.path().join(".ssh").join("config"), config).expect("the config");
    }
    home
}

/// A stand-in `ssh-keyscan` that runs `body` as a shell script.
fn scanner(body: &str) -> (tempfile::TempDir, HostKeyScan) {
    let folder = tempfile::tempdir().expect("a temporary folder");
    let program = folder.path().join("ssh-keyscan");
    std::fs::write(&program, format!("#!/bin/sh\n{body}\n")).expect("the program");
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).expect("an executable");
    runnable(&program);
    let scan = HostKeyScan {
        program,
        attempts: 2,
        connect_timeout_secs: 1,
        pause: Duration::ZERO,
    };
    (folder, scan)
}

/// Wait until the kernel lets this program run. Another test thread can fork while the file is
/// still open for writing, and an exec then fails with `Text file busy`. The exit status of a
/// stand-in does not matter here: only the exec does.
fn runnable(program: &Path) {
    for _ in 0..200 {
        let status = Command::new(program)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        if status.is_ok() {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("{} never became runnable", program.display());
}

/// A stand-in scan that offers the keys of `address`.
fn trusted() -> (tempfile::TempDir, HostKeyScan) {
    scanner(&format!(
        "printf '%s\\n' '{ADDRESS} ssh-ed25519 {KEY}' '{ADDRESS} ssh-rsa {RSA}'"
    ))
}

fn entry<'a>(home: &'a Path, root: &'a Path, scan: &HostKeyScan) -> Entry<'a> {
    Entry::with_scan(home, root, scan.clone())
}

fn config(home: &Path) -> String {
    std::fs::read_to_string(home.join(".ssh").join("config")).unwrap_or_default()
}

/// The block that the code writes, with `address` and the paths of this home and root.
fn expected_block(root: &Path, address: &str, identity: &str) -> String {
    let known = root.join(KNOWN_HOSTS);
    format!(
        "# >>> horizon browser vm; written by Horizon, do not edit\n\
         Host horizon-browser-vm\n  \
         HostName {address}\n  \
         User root\n  \
         IdentityFile {identity}\n  \
         IdentitiesOnly yes\n  \
         UserKnownHostsFile {}\n  \
         HostKeyAlias horizon-browser-vm\n\
         # <<< horizon browser vm\n",
        known.display()
    )
}

#[test]
fn the_block_goes_to_the_top_and_the_hosts_of_the_person_stay_byte_for_byte() {
    let home = home(Some(PERSON));
    let root = tempfile::tempdir().expect("a temporary cloud root");
    let (_scan, scan) = trusted();

    entry(home.path(), root.path(), &scan)
        .write(ADDRESS, Path::new(IDENTITY))
        .expect("the entry");

    let written = config(home.path());
    let block = expected_block(root.path(), ADDRESS, IDENTITY);
    assert!(written.starts_with(&block), "{written}");
    assert_eq!(
        written.strip_prefix(&block),
        Some(PERSON),
        "every byte of the hosts of the person is kept, and the block is first for ssh"
    );
    let known = std::fs::read_to_string(root.path().join(KNOWN_HOSTS)).expect("the managed file");
    assert_eq!(
        known,
        format!("horizon-browser-vm ssh-ed25519 {KEY}\nhorizon-browser-vm ssh-rsa {RSA}\n"),
        "ssh looks up the alias, never the address behind it"
    );
}

#[test]
fn a_second_write_keeps_one_block_with_the_newest_address() {
    let home = home(Some(PERSON));
    let root = tempfile::tempdir().expect("a temporary cloud root");
    let (_scan, scan) = trusted();
    let entry = entry(home.path(), root.path(), &scan);

    entry.write(ADDRESS, Path::new(IDENTITY)).expect("the entry");
    entry.write(OTHER, Path::new(IDENTITY)).expect("the entry again");

    let written = config(home.path());
    assert_eq!(written.matches("Host horizon-browser-vm").count(), 1, "{written}");
    assert_eq!(written.matches("# <<< horizon browser vm").count(), 1, "{written}");
    assert!(written.contains(&format!("  HostName {OTHER}\n")), "{written}");
    assert!(!written.contains(ADDRESS), "the address of the older machine is gone");
    assert!(written.ends_with(PERSON), "{written}");
}

#[test]
fn the_name_of_a_person_is_never_taken_over() {
    for own in [
        "Host horizon-browser-vm\n  HostName 192.0.2.44\n",
        "Host Horizon-Browser-VM\n  HostName 192.0.2.44\n",
        "Host laptop,horizon-browser-vm\n  HostName 192.0.2.44\n",
        "Host *\n  User person\nHost horizon-browser-vm\n",
    ] {
        let text = format!("{PERSON}{own}");
        let home = home(Some(&text));
        let root = tempfile::tempdir().expect("a temporary cloud root");
        let (_scan, scan) = trusted();
        let entry = entry(home.path(), root.path(), &scan);

        let refused = entry
            .write(ADDRESS, Path::new(IDENTITY))
            .expect_err("the name is taken");
        assert!(refused.to_string().contains("horizon-browser-vm"), "{refused}");
        assert_eq!(
            config(home.path()),
            text,
            "nothing is written into the file of the person"
        );
        assert!(
            !root.path().join(KNOWN_HOSTS).exists(),
            "a refused entry trusts no key: {own}"
        );

        entry.remove().expect_err("removal refuses too");
        assert_eq!(config(home.path()), text, "nothing is removed either: {own}");
    }
}

#[test]
fn a_removal_keeps_the_hosts_of_the_person_and_leaves_no_marker() {
    let home = home(Some(PERSON));
    let root = tempfile::tempdir().expect("a temporary cloud root");
    let (_scan, scan) = trusted();
    let entry = entry(home.path(), root.path(), &scan);
    entry.write(ADDRESS, Path::new(IDENTITY)).expect("the entry");
    assert!(root.path().join(KNOWN_HOSTS).exists());

    entry.remove().expect("the removal");

    assert_eq!(config(home.path()), PERSON, "the hosts of the person stay as they were");
    assert!(
        !root.path().join(KNOWN_HOSTS).exists(),
        "no key of a gone machine stays"
    );
}

#[test]
fn a_configuration_that_only_held_the_block_is_gone_after_a_removal() {
    let home = home(None);
    let root = tempfile::tempdir().expect("a temporary cloud root");
    let (_scan, scan) = trusted();
    let entry = entry(home.path(), root.path(), &scan);
    entry.write(ADDRESS, Path::new(IDENTITY)).expect("the entry");

    entry.remove().expect("the removal");

    assert!(
        !home.path().join(".ssh").join("config").exists(),
        "the file that Horizon made for its own block is gone, as it was before the deploy"
    );
}

#[test]
fn the_folder_and_both_files_are_private() {
    let home = home(Some(PERSON));
    let root = tempfile::tempdir().expect("a temporary cloud root");
    let (_scan, scan) = trusted();
    entry(home.path(), root.path(), &scan)
        .write(ADDRESS, Path::new(IDENTITY))
        .expect("the entry");

    let mode = |path: &Path| {
        std::fs::metadata(path)
            .map(|meta| meta.permissions().mode() & 0o777)
            .expect("the file")
    };
    assert_eq!(mode(&home.path().join(".ssh")), 0o700, "ssh requires a private folder");
    assert_eq!(mode(&home.path().join(".ssh").join("config")), 0o600);
    assert_eq!(mode(&root.path().join(KNOWN_HOSTS)), 0o600);
}

#[test]
fn a_machine_whose_sshd_is_not_up_yet_is_scanned_again_until_it_answers() {
    // A live resume showed the failure this covers: Hetzner reported the server as running
    // while its sshd still refused connections, so a short scan wrote the entry with no key
    // and every route failed with "Host key verification failed". The scan now keeps asking.
    let home = home(None);
    let root = tempfile::tempdir().expect("a temporary cloud root");
    let counter = root.path().join("attempts");
    let (folder, mut scan) = scanner(&format!(
        r#"count=$(cat "{counter}" 2>/dev/null || echo 0); count=$((count + 1)); echo "$count" > "{counter}"; if [ "$count" -ge 5 ]; then printf '%s\n' '{ADDRESS} ssh-ed25519 {KEY}'; fi"#,
        counter = counter.display()
    ));
    scan.program = folder.path().join("ssh-keyscan");
    // The harness keeps its stand-ins at two attempts; this one needs five to answer.
    scan.attempts = 6;

    let trust = entry(home.path(), root.path(), &scan)
        .write(ADDRESS, Path::new(IDENTITY))
        .expect("the entry");

    assert_eq!(trust.note, None, "the fifth attempt is the one that answers");
    assert_eq!(
        std::fs::read_to_string(&counter).expect("the attempts").trim(),
        "5",
        "the scan stopped as soon as a key arrived"
    );
    assert!(
        std::fs::read_to_string(root.path().join(KNOWN_HOSTS))
            .expect("the managed known_hosts")
            .contains(KEY)
    );
}

#[test]
fn a_machine_that_is_already_trusted_needs_no_new_key() {
    let home = home(Some(PERSON));
    let root = tempfile::tempdir().expect("a temporary cloud root");
    let (_scan, scan) = trusted();
    let entry = entry(home.path(), root.path(), &scan);
    entry.write(ADDRESS, Path::new(IDENTITY)).expect("the entry");
    let trusted = std::fs::read(root.path().join(KNOWN_HOSTS)).expect("the managed file");

    let again = entry.write(ADDRESS, Path::new(IDENTITY)).expect("the entry again");

    assert_eq!(again.note, None, "a trusted machine reports nothing");
    assert_eq!(
        std::fs::read(root.path().join(KNOWN_HOSTS)).expect("the managed file"),
        trusted,
        "no key is added twice"
    );
    assert_eq!(config(home.path()), {
        let mut expected = expected_block(root.path(), ADDRESS, IDENTITY);
        expected.push_str(PERSON);
        expected
    });
}

#[test]
fn a_scan_that_fails_is_a_note_and_not_a_failed_deploy() {
    let home = home(Some(PERSON));
    let root = tempfile::tempdir().expect("a temporary cloud root");
    let (_scan, scan) = scanner("printf '%s\\n' 'connect: Connection refused' >&2\nexit 1");

    let trust = entry(home.path(), root.path(), &scan)
        .write(ADDRESS, Path::new(IDENTITY))
        .expect("a deploy that rented a machine is not undone by a scan");

    let note = trust.note.expect("the reason is recorded");
    assert!(note.contains(ADDRESS), "{note}");
    assert!(note.contains("Connection refused"), "{note}");
    assert!(
        config(home.path()).starts_with(&expected_block(root.path(), ADDRESS, IDENTITY)),
        "the entry stays in place, so the machine is still reachable"
    );
    assert!(!root.path().join(KNOWN_HOSTS).exists(), "a failed scan trusts nothing");
}

#[test]
fn an_address_that_is_not_one_host_name_is_refused() {
    let home = home(Some(PERSON));
    let root = tempfile::tempdir().expect("a temporary cloud root");
    let (_scan, scan) = trusted();
    let entry = entry(home.path(), root.path(), &scan);

    for address in ["203.0.113.10\nProxyCommand evil", "", "203.0.113.10 User person"] {
        entry
            .write(address, Path::new(IDENTITY))
            .expect_err("an address of another shape would add a line of its own");
    }
    assert_eq!(config(home.path()), PERSON, "no refused address reaches the file");
}

/// What the real `ssh` resolves the alias to, as one value for each keyword.
fn resolved(config: &Path, alias: &str) -> Vec<String> {
    let output = Command::new("ssh")
        .arg("-G")
        .arg("-F")
        .arg(config)
        .arg(alias)
        .stdin(Stdio::null())
        .output()
        .expect("ssh of the OpenSSH tools");
    assert!(
        output.status.success(),
        "ssh read the configuration: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.lines()
        .filter(|line| {
            [
                "hostname",
                "user",
                "identityfile",
                "userknownhostsfile",
                "identitiesonly",
            ]
            .iter()
            .any(|keyword| line.starts_with(&format!("{keyword} ")))
        })
        .map(str::to_owned)
        .collect()
}

#[test]
fn the_entry_names_its_own_host_key_alias_because_ssh_keys_the_address_it_connects_to() {
    // ssh checks the host key against the name it connected to, which is `HostName` and not the
    // alias of the block. Kipping the keys under the alias without `HostKeyAlias` makes every
    // route fail with "Host key verification failed", which a live machine showed and `ssh -G`
    // cannot: it parses the configuration and never verifies a key.
    let home = home(None);
    let root = tempfile::tempdir().expect("a temporary cloud root");
    let (_scan, scan) = trusted();
    entry(home.path(), root.path(), &scan)
        .write(ADDRESS, Path::new(IDENTITY))
        .expect("the entry");

    let written = config(home.path());
    assert!(
        written.contains("\n  HostKeyAlias horizon-browser-vm\n"),
        "the block names the alias that its known_hosts file is keyed by: {written}"
    );
}

#[test]
fn ssh_resolves_the_alias_to_the_machine_with_its_key_and_its_known_hosts() {
    let home = home(Some(PERSON));
    let root = tempfile::tempdir().expect("a temporary cloud root");
    let (_scan, scan) = trusted();
    let entry = entry(home.path(), root.path(), &scan);
    // The identity is a stand-in: `ssh -G` resolves the configuration and reads no key.
    entry.write(ADDRESS, Path::new(IDENTITY)).expect("the entry");

    let values = resolved(&home.path().join(".ssh").join("config"), "horizon-browser-vm");

    let known = root.path().join(KNOWN_HOSTS);
    for expected in [
        format!("hostname {ADDRESS}"),
        "user root".to_owned(),
        format!("identityfile {IDENTITY}"),
        format!("userknownhostsfile {}", known.display()),
        "identitiesonly yes".to_owned(),
    ] {
        assert!(values.contains(&expected), "ssh resolves `{expected}`: {values:?}");
    }
}
