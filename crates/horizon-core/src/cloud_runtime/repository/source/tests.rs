use super::*;

fn remote(input: &str) -> Option<(String, String, String)> {
    parse(input).map(|remote| (remote.url, remote.host, remote.name))
}

#[test]
fn reads_every_common_way_to_name_a_repository() {
    let expected = Some((
        "https://github.com/peters/horizon.git".to_owned(),
        "github.com".to_owned(),
        "horizon".to_owned(),
    ));
    for input in [
        "peters/horizon",
        "github.com/peters/horizon",
        "https://github.com/peters/horizon",
        "https://github.com/peters/horizon.git",
        "https://github.com/peters/horizon/tree/main/crates",
        " https://github.com/peters/horizon/  ",
        "https://GitHub.com/peters/horizon/tree/main/crates",
        "GITHUB.COM/peters/horizon",
    ] {
        assert_eq!(remote(input), expected, "{input}");
    }
    assert_eq!(
        remote("git@GitHub.COM:peters/horizon.git"),
        Some((
            "git@github.com:peters/horizon.git".into(),
            "github.com".into(),
            "horizon".into()
        )),
        "the host is one spelling, the path keeps its own"
    );
    assert_eq!(
        remote("git@github.com:peters/horizon.git"),
        Some((
            "git@github.com:peters/horizon.git".into(),
            "github.com".into(),
            "horizon".into()
        ))
    );
}

#[test]
fn names_the_owner_a_clone_is_kept_under() {
    let owner = |input: &str| parse(input).map(|remote| remote.owner);
    assert_eq!(owner("peters/horizon").as_deref(), Some("peters"));
    assert_eq!(owner("git@github.com:Acme/Web.git").as_deref(), Some("Acme"));
    assert_eq!(
        owner("https://gitlab.com/group/sub/app/-/tree/main").as_deref(),
        Some("group/sub"),
        "every group of a GitLab path"
    );
}

#[test]
fn an_owner_or_name_that_windows_reserves_gets_a_folder_it_can_make() {
    let temp = Path::new("/synthetic");
    let remote = parse("https://git.example.org/CON/team./com1.txt").unwrap();
    assert_eq!(remote.owner, "CON/team.", "the link itself is kept as written");
    assert_eq!(
        destination(temp, &remote),
        temp.join("CON_").join("team._").join("com1.txt_")
    );
    for plain in ["acme", "console", "com0", "lpt10", "nul-ish", ".github"] {
        assert_eq!(portable(plain), plain, "{plain}");
    }
    for reserved in ["con", "Aux", "NUL.txt", "COM9", "lpt1", "trailing."] {
        assert_eq!(portable(reserved), format!("{reserved}_"), "{reserved}");
    }
}

#[test]
fn keeps_gitlab_groups_and_drops_browser_suffixes() {
    assert_eq!(
        remote("https://gitlab.com/group/sub/app/-/tree/main"),
        Some((
            "https://gitlab.com/group/sub/app.git".into(),
            "gitlab.com".into(),
            "app".into()
        ))
    );
    assert_eq!(
        remote("gitlab.example.org/team/app").map(|(_, host, _)| host),
        Some("gitlab.example.org".into())
    );
}

#[test]
fn leaves_paths_and_unsafe_input_alone() {
    for input in [
        "",
        "/home/me/horizon",
        "~/horizon",
        "./horizon",
        "-oProxyCommand=x/y",
        "horizon",
        "file:///tmp/repo",
        "ext::sh -c id",
        "https://github.com/peters",
        "owner/repo name",
    ] {
        assert_eq!(parse(input), None, "{input}");
    }
}

#[test]
fn sorts_clone_failures_by_what_the_person_can_do() {
    let github = parse("github.com/demo-org/demo").unwrap();
    for (stderr, expected) in [
        (
            "fatal: could not read Username for 'https://github.com': terminal prompts disabled",
            Failure::SignIn("github.com".into()),
        ),
        (
            "remote: Repository not found.\nfatal: repository 'x' not found",
            Failure::SignIn("github.com".into()),
        ),
        (
            "fatal: unable to access 'x': Could not resolve host: github.com",
            Failure::Network,
        ),
        (
            "fatal: could not create work tree dir 'x': Permission denied",
            Failure::Other("fatal: could not create work tree dir 'x': Permission denied".into()),
        ),
        ("fatal: something odd\n", Failure::Other("fatal: something odd".into())),
    ] {
        assert_eq!(classify(&github, stderr), expected, "{stderr}");
    }
}

#[test]
fn a_linked_worktree_reads_the_origin_of_the_checkout_it_belongs_to() {
    let temp = tempfile::tempdir().unwrap();
    let main = temp.path().join("main");
    std::fs::create_dir_all(main.join(".git").join("worktrees").join("wt")).unwrap();
    std::fs::write(
        main.join(".git").join("config"),
        "[remote \"origin\"]\n\turl = https://github.com/demo-org/demo.git\n",
    )
    .unwrap();
    std::fs::write(
        main.join(".git").join("worktrees").join("wt").join("commondir"),
        "../..\n",
    )
    .unwrap();
    let linked = temp.path().join("linked");
    std::fs::create_dir(&linked).unwrap();
    // The pointer is absolute for a worktree Git made.
    let pointer = main.join(".git").join("worktrees").join("wt");
    std::fs::write(linked.join(".git"), format!("gitdir: {}\n", pointer.display())).unwrap();
    assert_eq!(
        origin_url(&linked).as_deref(),
        Some("https://github.com/demo-org/demo.git")
    );
    assert_eq!(origin_url(temp.path()), None);
}

#[test]
fn an_origin_is_read_from_the_config_file() {
    let temp = tempfile::tempdir().unwrap();
    assert_eq!(origin_url(temp.path()), None);
    std::fs::create_dir(temp.path().join(".git")).unwrap();
    std::fs::write(
        temp.path().join(".git").join("config"),
        "[core]\n\turl = not-this\n[remote \"upstream\"]\n\turl = https://example.org/other.git\n[remote \"origin\"]\n\tfetch = +refs/heads/*:refs/remotes/origin/*\n\turl = https://github.com/demo-org/demo.git\n",
    )
    .unwrap();
    assert_eq!(
        origin_url(temp.path()).as_deref(),
        Some("https://github.com/demo-org/demo.git")
    );
}

#[test]
fn a_plain_http_link_stays_plain_and_never_asks_for_a_token() {
    let plain = parse("http://git.example.org/group/demo").unwrap();
    assert_eq!(plain.url, "http://git.example.org/group/demo.git");
    assert!(Token::new(&plain, "glpat_demo").is_none(), "no token over plain http");
    assert!(matches!(
        classify(&plain, "fatal: could not read Username for 'http://git.example.org'"),
        Failure::Other(text) if text.contains("plain http")
    ));
    assert_eq!(classify(&plain, "remote: Repository not found."), Failure::NotFound);
}

#[test]
fn a_missing_repository_over_ssh_is_not_found_not_a_sign_in() {
    let ssh = parse("git@github.com:demo-org/demo").unwrap();
    assert_eq!(classify(&ssh, "ERROR: Repository not found."), Failure::NotFound);
}

#[test]
fn ssh_failures_never_ask_for_a_token() {
    let ssh = parse("git@github.com:demo-org/demo").unwrap();
    for stderr in [
        "git@github.com: Permission denied (publickey).",
        "Host key verification failed.",
        "fatal: Authentication failed for 'ssh://git@github.com/demo-org/demo.git'",
    ] {
        assert!(
            matches!(classify(&ssh, stderr), Failure::Other(text) if text.contains("over SSH")),
            "{stderr}"
        );
    }
}

#[test]
fn a_token_or_password_in_the_address_never_reaches_git() {
    let remote = parse("https://user:ghp_secret@github.com/demo-org/demo").unwrap();
    assert_eq!(remote.url, "https://github.com/demo-org/demo.git");
    let ssh = parse("ssh://git:secret@github.com/demo-org/demo").unwrap();
    assert_eq!(ssh.url, "ssh://git@github.com/demo-org/demo.git");
    assert!(!format!("{remote:?}{ssh:?}").contains("secret"));
    assert_eq!(parse("ssh://-oProxyCommand=a.b/demo-org/demo"), None);
    assert_eq!(parse("https://-bad.example.org/demo-org/demo"), None);
    assert_eq!(parse("-o@github.com:demo-org/demo"), None);
}

#[test]
fn a_name_that_could_leave_the_clone_folder_is_refused() {
    for input in [
        r"github.com/demo-org/..\\victim",
        r"github.com/demo-org/C:\\victim",
        "github.com/demo-org/..",
        "https://github.com/demo-org/a%2Fb",
        "https://gitlab.com/group/../demo",
    ] {
        assert_eq!(parse(input), None, "{input}");
    }
    assert!(
        parse("github.com/demo-org/.github").is_some(),
        "a leading dot is a real name"
    );
}

#[test]
fn a_helper_counts_only_while_it_is_in_force_for_the_origin() {
    let scope = "https://github.com/";
    assert!(!helper_configured("", scope));
    assert!(helper_configured("credential.helper store\n", scope));
    assert!(
        !helper_configured("credential.helper store\ncredential.helper \n", scope),
        "an empty one resets"
    );
    assert!(helper_configured("credential.https://github.com.helper cache\n", scope));
    assert!(
        !helper_configured("credential.https://gitlab.com.helper cache\n", scope),
        "another host's helper"
    );
    assert!(helper_configured(
        "credential.helper \ncredential.helper store\n",
        scope
    ));
}

#[test]
fn a_token_travels_as_a_basic_header_for_its_host_only() {
    let github_remote = parse("github.com/demo-org/demo").unwrap();
    let github = Token::new(&github_remote, " ghp_demo \n").unwrap();
    assert_eq!(github.scope, "https://github.com/");
    assert_eq!(
        github.header(),
        format!(
            "Authorization: Basic {}",
            base64::engine::general_purpose::STANDARD.encode("x-access-token:ghp_demo")
        )
    );
    let gitlab = parse("https://gitlab.example.org:8443/group/demo").unwrap();
    assert_eq!(
        Token::new(&gitlab, "glpat_demo").unwrap().scope,
        "https://gitlab.example.org:8443/"
    );
    assert!(Token::new(&gitlab, "with space").is_none());
    assert!(Token::new(&gitlab, "  ").is_none());
    let ssh = parse("git@github.com:demo-org/demo").unwrap();
    assert!(Token::new(&ssh, "ghp_demo").is_none(), "a token never applies over SSH");
}

#[test]
fn a_new_clone_never_goes_beyond_the_folders_a_search_looks_in() {
    let temp = tempfile::tempdir().unwrap();
    let remote = parse("peters/horizon").unwrap();
    let owner = temp.path().join("peters");
    std::fs::create_dir_all(owner.join("horizon")).unwrap();
    for n in 2..=CANDIDATES {
        std::fs::create_dir(owner.join(format!("horizon-{n}"))).unwrap();
    }
    assert_eq!(
        destination(temp.path(), &remote),
        owner.join("horizon"),
        "with every candidate taken there is no later one to hide a checkout in"
    );
}

#[test]
fn picks_a_free_folder_named_for_the_repository() {
    let temp = tempfile::tempdir().unwrap();
    let remote = parse("peters/horizon").unwrap();
    assert_eq!(destination(temp.path(), &remote), temp.path().join("peters/horizon"));
    std::fs::create_dir_all(temp.path().join("peters/horizon")).unwrap();
    assert_eq!(destination(temp.path(), &remote), temp.path().join("peters/horizon-2"));
    // Another owner's repository of the same name has a folder of its own.
    let other = parse("acme/horizon").unwrap();
    assert_eq!(destination(temp.path(), &other), temp.path().join("acme/horizon"));
    // An earlier clone straight under the parent is no reason to number a new one.
    std::fs::create_dir(temp.path().join("tools")).unwrap();
    assert_eq!(
        destination(temp.path(), &parse("acme/tools").unwrap()),
        temp.path().join("acme/tools")
    );
    assert_eq!(default_parent(temp.path()), temp.path().join("Horizon"));
    std::fs::create_dir(temp.path().join("code")).unwrap();
    assert_eq!(default_parent(temp.path()), temp.path().join("code"));
}

#[test]
fn clones_a_local_repository_and_recognises_the_checkout() {
    let temp = tempfile::tempdir().unwrap();
    let origin = temp.path().join("origin");
    let git = |dir: &Path, args: &[&str]| {
        let status = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
            .args(args)
            .stdout(Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "{args:?}");
    };
    std::fs::create_dir(&origin).unwrap();
    git(&origin, &["init", "-q"]);
    git(&origin, &["commit", "-q", "--allow-empty", "-m", "first"]);
    let remote = Remote {
        url: origin.to_string_lossy().into_owned(),
        host: "example.com".into(),
        owner: "demo-org".into(),
        name: "origin".into(),
    };
    let target = temp.path().join("nested/clone");
    assert!(!is_checkout(&target));
    assert_eq!(probe(&remote, None, &Cancellation::default()), Ok(()));
    let missing = Remote {
        url: temp.path().join("missing").to_string_lossy().into_owned(),
        ..remote.clone()
    };
    assert!(probe(&missing, None, &Cancellation::default()).is_err());
    let cancelled = Cancellation::default();
    cancelled.cancel();
    assert_eq!(probe(&remote, None, &cancelled), Err(Failure::Cancelled));
    clone(&remote, &target, None, &Cancellation::default(), &Progress::default()).unwrap();
    assert!(is_checkout(&target));
}

#[test]
fn a_cancelled_or_failed_clone_removes_only_what_it_made() {
    let temp = tempfile::tempdir().unwrap();
    let remote = Remote {
        url: temp.path().join("missing").to_string_lossy().into_owned(),
        host: "example.com".into(),
        owner: "demo-org".into(),
        name: "missing".into(),
    };
    let cancelled = Cancellation::default();
    cancelled.cancel();
    let fresh = temp.path().join("fresh");
    assert_eq!(
        clone(&remote, &fresh, None, &cancelled, &Progress::default()),
        Err(Failure::Cancelled)
    );
    assert!(!fresh.exists(), "a folder the clone made is removed");
    let theirs = temp.path().join("theirs");
    std::fs::create_dir(&theirs).unwrap();
    std::fs::write(theirs.join("notes.txt"), "keep").unwrap();
    assert!(clone(&remote, &theirs, None, &Cancellation::default(), &Progress::default()).is_err());
    assert!(
        theirs.join("notes.txt").is_file(),
        "a folder that was already there stays"
    );
}

#[test]
fn a_checkout_already_under_the_parent_is_recognised_for_its_own_link_only() {
    let temp = tempfile::tempdir().unwrap();
    let origin = temp.path().join("origin");
    std::fs::create_dir(&origin).unwrap();
    let git = |dir: &Path, args: &[&str]| {
        let status = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
            .args(args)
            .stdout(Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "{args:?}");
    };
    git(&origin, &["init", "-q"]);
    git(&origin, &["commit", "-q", "--allow-empty", "-m", "first"]);
    let remote = parse("github.com/demo-org/demo").unwrap();
    let checkout = temp.path().join("demo");
    git(temp.path(), &["clone", "-q", origin.to_str().unwrap(), "demo"]);
    assert_eq!(existing(temp.path(), &remote), None, "another origin is not this link");
    git(&checkout, &["remote", "set-url", "origin", &remote.url]);
    assert_eq!(existing(temp.path(), &remote), Some(checkout.clone()));
    // A second copy made beside an occupied name is found too.
    let beside = temp.path().join("demo-2");
    git(temp.path(), &["clone", "-q", origin.to_str().unwrap(), "demo-2"]);
    git(
        &beside,
        &["remote", "set-url", "origin", "https://github.com/demo-org/other.git"],
    );
    git(
        &checkout,
        &["remote", "set-url", "origin", "https://github.com/demo-org/other.git"],
    );
    git(&beside, &["remote", "set-url", "origin", &remote.url]);
    assert_eq!(existing(temp.path(), &remote), Some(beside.clone()));
    // A gap before it (the first name taken by a file) does not hide it.
    std::fs::remove_dir_all(&checkout).unwrap();
    std::fs::write(&checkout, "not a folder").unwrap();
    assert_eq!(existing(temp.path(), &remote), Some(beside.clone()));
    // A clone in the owner's folder, where new clones go, comes first.
    let kept = temp.path().join("demo-org/demo");
    std::fs::create_dir(temp.path().join("demo-org")).unwrap();
    git(
        &temp.path().join("demo-org"),
        &["clone", "-q", origin.to_str().unwrap(), "demo"],
    );
    git(&kept, &["remote", "set-url", "origin", &remote.url]);
    assert_eq!(existing(temp.path(), &remote), Some(kept));
}
