//! The arguments of a start of this binary.
//!
//! `main` judges every argument here before it touches the home, the settings, the
//! sessions or the skills of the person: a mode prints what it prints and ends, and
//! an argument that this build does not know names itself and exits 2.

use std::path::{Path, PathBuf};

use horizon_core::HorizonHome;

/// The name of this program on a command line, as the usage writes it.
pub(crate) const CLI_NAME: &str = "horizon";

/// One start of this binary, judged from its arguments alone.
#[derive(Debug)]
pub(crate) enum Start {
    /// `--help` or `-h`: the usage of this build.
    Help,
    /// `--version` or `-V`: the name and the version of this build.
    Version,
    /// `--agent-work-hook`: one lifecycle answer of an opted-in panel.
    AgentWorkHook,
    /// One of the three modes of the server.
    Sync(SyncMode),
    /// `--browser-mcp`: the browser MCP server on the standard input and output.
    BrowserMcp,
    /// `--browser-host-driver`: the driver of a browser panel that runs beside
    /// the Firefox of a browser host, on the standard input and output of one
    /// `ssh` child.
    BrowserHostDriver(horizon_core::browser::host_driver::HostDriverOptions),
    /// The window of the person, or a command that ends without one.
    Desktop(CliArgs),
}

/// Reads every argument of this start and answers its mode.
///
/// Each mode takes its own arguments alone, and an argument that this build does
/// not know is an error: a start that no mode serves never falls through to the
/// window of the person.
///
/// # Errors
///
/// Returns the reason that the arguments of this start cannot continue.
pub(crate) fn parse_start(args: impl IntoIterator<Item = std::ffi::OsString>) -> Result<Start, String> {
    let args: Vec<std::ffi::OsString> = args.into_iter().collect();
    match args.first().and_then(|first| first.to_str()) {
        Some("--help" | "-h") => alone(&args, Start::Help),
        Some("--version" | "-V") => alone(&args, Start::Version),
        Some("--agent-work-hook") => alone(&args, Start::AgentWorkHook),
        Some("--browser-mcp") => {
            parse_browser_mcp_args(args.iter().cloned())?;
            Ok(Start::BrowserMcp)
        }
        Some("--browser-host-driver") => Ok(Start::BrowserHostDriver(
            horizon_core::browser::host_driver::HostDriverOptions::parse(args.iter().skip(1).cloned())
                .map_err(|error| error.to_string())?,
        )),
        _ => match parse_sync_mode(args.iter().cloned())? {
            Some(mode) => Ok(Start::Sync(mode)),
            None => Ok(Start::Desktop(parse_cli_args(args)?)),
        },
    }
}

/// A mode that takes no other argument.
fn alone(args: &[std::ffi::OsString], start: Start) -> Result<Start, String> {
    match args.get(1) {
        Some(extra) => Err(unknown_argument(extra)),
        None => Ok(start),
    }
}

/// Whether this file is the copy that cargo keeps in `deps` beside its test
/// binaries, where a start rewrites the home of the person without a person
/// asking for it.
pub(crate) fn starts_from_a_build_folder() -> bool {
    std::env::current_exe()
        .ok()
        .and_then(|file| {
            file.parent()
                .map(|folder| folder.file_name() == Some(std::ffi::OsStr::new("deps")))
        })
        .unwrap_or(false)
}

/// The reason of one argument that this build does not know.
fn unknown_argument(argument: &std::ffi::OsStr) -> String {
    format!("unknown argument {}", argument.to_string_lossy())
}

/// The Horizon home of this start, or the end of the start with code 2.
pub(crate) fn prepare_home_or_exit() -> HorizonHome {
    match HorizonHome::prepare() {
        Ok(home) => home,
        Err(problem) => {
            eprintln!("error: {problem}");
            std::process::exit(2);
        }
    }
}

/// One of the three modes of the server, whose work needs no window and no home of
/// the person.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SyncMode {
    /// `horizon --server`: answers the links of the computers of one server home,
    /// and opens the TCP listener when `--listen` names one.
    Server(horizon_sync::ServerOptions),
    /// `horizon --server-link`: relays the standard input and output of one `ssh`
    /// child to the socket of that server.
    Link,
    /// `horizon --server-invite`: asks the server of this home for the first
    /// invitation, and prints the link, the code and its QR code.
    Invite,
}

/// Whether these arguments ask for the server, for its link or for its first
/// invitation.
///
/// # Errors
/// Returns why an argument that starts one of the three modes cannot continue.
pub(crate) fn parse_sync_mode(mut args: impl Iterator<Item = std::ffi::OsString>) -> Result<Option<SyncMode>, String> {
    let mut options = horizon_sync::ServerOptions::default();
    let mode = match args.next().as_deref() {
        Some(argument) if argument == "--server" => {
            loop {
                match args.next() {
                    None => break,
                    Some(flag) if flag == "--listen" => options.listen = Some(argument_of(&mut args, "--listen")?),
                    Some(flag) if flag == "--name" => options.name = Some(argument_of(&mut args, "--name")?),
                    Some(argument) => {
                        return Err(format!(
                            "`{}` is not part of the server mode",
                            argument.to_string_lossy()
                        ));
                    }
                }
            }
            return Ok(Some(SyncMode::Server(options)));
        }
        Some(argument) if argument == "--server-link" => SyncMode::Link,
        Some(argument) if argument == "--server-invite" => SyncMode::Invite,
        _ => return Ok(None),
    };
    if let Some(argument) = args.next() {
        return Err(format!(
            "`{}` is not part of the server mode",
            argument.to_string_lossy()
        ));
    }
    Ok(Some(mode))
}

/// The value of one flag of the server mode.
fn argument_of(args: &mut impl Iterator<Item = std::ffi::OsString>, flag: &str) -> Result<String, String> {
    let value = args.next().ok_or_else(|| format!("`{flag}` needs one value"))?;
    value
        .to_str()
        .map(str::to_owned)
        .ok_or_else(|| format!("the value of `{flag}` is not text"))
}

/// Whether these arguments ask for the browser MCP server of this computer,
/// whose entry in a client is `--browser-mcp` alone.
///
/// A further argument is an error: an entry that names something this mode does
/// not serve must never fall back to another browser.
pub(crate) fn parse_browser_mcp_args(mut args: impl Iterator<Item = std::ffi::OsString>) -> Result<bool, String> {
    if args.next().is_none_or(|argument| argument != "--browser-mcp") {
        return Ok(false);
    }
    if let Some(argument) = args.next() {
        return Err(format!(
            "`{}` is not part of the browser MCP mode",
            argument.to_string_lossy()
        ));
    }
    Ok(true)
}

pub(crate) const CLI_USAGE: &str = "usage: horizon [<folder>] [--config <path>] [--ephemeral] [--new-session] [--blank]
       horizon [--config <path>] --export-remote-profile <path>
       horizon [--config <path>] --import-remote-profile <path>
       horizon --server [--listen <ip>:<port>] [--name <dns-name>]
       horizon --server-link
       horizon --server-invite
       horizon --browser-host-driver [--profile <name>] [--engine <engine>]

A folder argument opens the board of that folder: Horizon shows the workspace of
the folder, or makes it with one panel of the first agent preset of the settings
whose program this computer has, or a terminal in that folder.";

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum RemoteProfileCommand {
    Export(PathBuf),
    Import(PathBuf),
}

#[derive(Debug)]
pub(crate) struct CliArgs {
    pub(crate) config_path: Option<PathBuf>,
    /// The folder of this start, as the person wrote it. [`resolve_folder`] turns
    /// it into the folder that the board shows.
    pub(crate) folder: Option<PathBuf>,
    pub(crate) new_session: bool,
    pub(crate) ephemeral: bool,
    pub(crate) blank: bool,
    pub(crate) command: Option<Command>,
}

/// The folder that `path` names, or the one sentence that says why it does not
/// name a folder.
///
/// A relative path and `.` resolve against the current directory of this start.
/// The answer is the folder as the board names it: its identity, so a symbolic
/// link, a trailing separator and an interior `.` name one folder. This is the
/// only file read of the arguments, and it runs before a window opens.
///
/// # Errors
///
/// Returns one sentence when the path is not there, or when it is not a folder.
pub(crate) fn resolve_folder(path: &Path) -> Result<PathBuf, String> {
    if !path.exists() {
        return Err(format!("{} is not there", path.display()));
    }
    if !path.is_dir() {
        return Err(format!("{} is not a folder", path.display()));
    }
    Ok(horizon_core::folder_identity(path))
}

/// A command that finishes without opening a window.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Command {
    RemoteProfile(RemoteProfileCommand),
}

/// Parse the launch flags. The remote-profile commands take exactly one
/// operand that is not itself a flag, and at most one of them may be given. An
/// argument that this build does not know is an error, so a typo can never fall
/// through to an ordinary launch or run another command.
pub(crate) fn parse_cli_args(args: impl IntoIterator<Item = std::ffi::OsString>) -> Result<CliArgs, String> {
    let mut config_path = None;
    let mut folder: Option<PathBuf> = None;
    let mut new_session = false;
    let mut ephemeral = false;
    let mut blank = false;
    let mut command: Option<Command> = None;

    let mut args = args.into_iter().peekable();
    while let Some(arg) = args.next() {
        let Some(flag) = arg.to_str() else {
            return Err(unknown_argument(&arg));
        };
        // One folder of this start, as a person types `horizon .`: an argument
        // that names no flag. The path is judged after every argument is read.
        if !flag.starts_with('-') && !flag.is_empty() {
            if folder.replace(PathBuf::from(flag)).is_some() {
                return Err("give one folder only".to_string());
            }
            continue;
        }
        let mut operand = |flag: &str| -> Result<PathBuf, String> {
            match args.next() {
                Some(value) if !value.is_empty() && !value.to_string_lossy().starts_with('-') => {
                    Ok(PathBuf::from(value))
                }
                _ => Err(format!("{flag} needs a path")),
            }
        };
        match flag {
            "--config" | "-c" => config_path = Some(operand(flag)?),
            "--export-config" => {
                return Err(
                    "--export-config is gone: the settings live in settings.sqlite, and the Settings window writes them"
                        .to_string(),
                );
            }
            "--export-remote-profile" | "--import-remote-profile" => {
                let path = operand(flag)?;
                let profile = if flag == "--export-remote-profile" {
                    RemoteProfileCommand::Export(path)
                } else {
                    RemoteProfileCommand::Import(path)
                };
                set_command(&mut command, Command::RemoteProfile(profile))?;
            }
            "--new-session" => new_session = true,
            "--ephemeral" => ephemeral = true,
            "--blank" => blank = true,
            unknown => return Err(unknown_argument(std::ffi::OsStr::new(unknown))),
        }
    }

    Ok(CliArgs {
        config_path,
        folder,
        new_session,
        ephemeral,
        blank,
        command,
    })
}

/// Keep one command, so a second one is an error instead of a silent winner.
fn set_command(command: &mut Option<Command>, next: Command) -> Result<(), String> {
    if command.replace(next).is_some() {
        return Err("give one command only, once: --export-remote-profile or --import-remote-profile".to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use super::{
        CLI_USAGE, Command, RemoteProfileCommand, Start, SyncMode, parse_cli_args, parse_start, parse_sync_mode,
        resolve_folder,
    };

    fn parse(args: &[&str]) -> Result<super::CliArgs, String> {
        parse_cli_args(args.iter().map(|arg| OsString::from(*arg)))
    }

    fn start(args: &[&str]) -> Result<Start, String> {
        parse_start(args.iter().map(|arg| OsString::from(*arg)))
    }

    /// The usage, the version and the hook take no other argument, and an
    /// argument that this build does not know ends the start instead of opening
    /// the window of the person.
    #[test]
    fn a_mode_takes_its_own_arguments_alone() {
        for (arguments, help, version, hook) in [
            (&["--help"][..], true, false, false),
            (&["-h"][..], true, false, false),
            (&["--version"][..], false, true, false),
            (&["-V"][..], false, true, false),
            (&["--agent-work-hook"][..], false, false, true),
        ] {
            let mode = start(arguments).expect("the mode");
            assert_eq!(matches!(mode, Start::Help), help, "{arguments:?}");
            assert_eq!(matches!(mode, Start::Version), version, "{arguments:?}");
            assert_eq!(matches!(mode, Start::AgentWorkHook), hook, "{arguments:?}");
            let extra = [arguments[0], "--list"];
            let error = start(&extra).expect_err("a mode takes no other argument");
            assert_eq!(error, "unknown argument --list", "{extra:?}");
        }
        assert!(matches!(
            start(&["--browser-mcp"]).expect("the browser MCP mode"),
            Start::BrowserMcp
        ));
        assert!(
            matches!(start(&[]).expect("a start with no argument"), Start::Desktop(_)),
            "no argument is the window of the person"
        );
    }

    #[test]
    fn the_host_driver_names_its_profile_and_its_engine() {
        let Start::BrowserHostDriver(options) = start(&["--browser-host-driver"]).expect("the mode") else {
            panic!("the browser host driver mode");
        };
        assert_eq!(options.profile(), "default");
        assert_eq!(options.engine().as_str(), "firefox");

        let Start::BrowserHostDriver(options) =
            start(&["--browser-host-driver", "--profile", "mail", "--engine", "firefox"]).expect("the mode")
        else {
            panic!("the browser host driver mode");
        };
        assert_eq!(options.profile(), "mail");

        for bad in [
            &["--browser-host-driver", "--profile"][..],
            &["--browser-host-driver", "--profile", "Mail"],
            &["--browser-host-driver", "--engine", "safari"],
            &["--browser-host-driver", "--engine", "chromium"],
            &["--browser-host-driver", "--profile", "mail", "--profile", "other"],
            &["--browser-host-driver", "--listen", "127.0.0.1:1"],
        ] {
            let error = start(bad).expect_err("the mode refuses it");
            assert!(!error.is_empty(), "{bad:?}");
        }
    }

    #[test]
    fn a_folder_is_one_operand_of_this_start() {
        let args = parse(&["."]).expect("the current folder");
        assert_eq!(args.folder.as_deref(), Some(std::path::Path::new(".")));
        assert_eq!(
            parse(&["--config", "c.json", "repo", "--ephemeral"])
                .expect("a folder beside the other options")
                .folder
                .as_deref(),
            Some(std::path::Path::new("repo")),
            "a folder stands beside the options of the start"
        );
        assert_eq!(
            parse(&["--blank", "/tmp/my repo"]).expect("a path with a space").folder,
            Some(std::path::PathBuf::from("/tmp/my repo"))
        );
        assert!(
            parse(&["one", "two"]).is_err(),
            "one folder only: a second operand is an error"
        );
        assert!(
            parse(&[]).expect("no argument").folder.is_none(),
            "a start without a folder opens the board of the last run"
        );
    }

    #[test]
    fn a_folder_that_is_not_a_folder_ends_with_one_sentence() {
        let temp = tempfile::tempdir().expect("temp dir");
        let file = temp.path().join("a-file");
        std::fs::write(&file, b"not a folder").expect("the file");

        let missing = resolve_folder(&temp.path().join("gone")).expect_err("a path that is gone");
        assert_eq!(missing, format!("{} is not there", temp.path().join("gone").display()));

        let refused = resolve_folder(&file).expect_err("a file is not a folder");
        assert_eq!(refused, format!("{} is not a folder", file.display()));

        let folder = temp.path().join("repo");
        std::fs::create_dir(&folder).expect("the folder");
        assert_eq!(
            resolve_folder(&folder).expect("a folder"),
            horizon_core::folder_identity(&folder),
            "the answer is the identity of the folder"
        );
        let spelled = std::path::PathBuf::from(format!("{}/", folder.display()));
        assert_eq!(
            resolve_folder(&spelled).expect("a folder with a trailing separator"),
            horizon_core::folder_identity(&folder)
        );
    }

    #[test]
    fn the_usage_names_the_folder_of_a_start() {
        assert!(CLI_USAGE.contains("horizon [<folder>]"), "{CLI_USAGE}");
        assert!(CLI_USAGE.contains("one panel"), "the usage says what the folder gives");
    }

    #[test]
    fn an_argument_that_this_build_does_not_know_ends_the_start() {
        for arguments in [
            &["--list"][..],
            &["--nocapture"],
            &["--test-threads", "4"],
            &["--config", "c.json", "--list"],
        ] {
            let error = start(arguments).expect_err("the argument is refused");
            assert!(error.starts_with("unknown argument "), "{arguments:?}: {error}");
            assert!(
                !error.contains("--blank") && error.len() < 80,
                "one line names only the argument: {error}"
            );
        }
        let error = start(&["--definitely-unknown"]).expect_err("refused");
        assert_eq!(error, "unknown argument --definitely-unknown");
    }

    #[test]
    fn remote_profile_commands_take_one_real_operand_and_exclude_each_other() {
        let args = parse(&["--config", "c.json", "--export-remote-profile", "p.json"]).expect("export");
        assert_eq!(
            args.command,
            Some(Command::RemoteProfile(RemoteProfileCommand::Export("p.json".into())))
        );
        assert_eq!(args.config_path.as_deref(), Some(std::path::Path::new("c.json")));
        let args = parse(&["--import-remote-profile", "p.json", "--ephemeral"]).expect("import");
        assert_eq!(
            args.command,
            Some(Command::RemoteProfile(RemoteProfileCommand::Import("p.json".into())))
        );
        assert!(args.ephemeral);

        for bad in [
            &["--import-remote-profile"][..],
            &["--import-remote-profile", "--ephemeral"],
            &["--export-remote-profile", ""],
            &["--config"],
            &["--export-remote-profile", "a.json", "--import-remote-profile", "b.json"],
            &["--export-remote-profile", "a.json", "--export-remote-profile", "b.json"],
        ] {
            assert!(parse(bad).is_err(), "{bad:?}");
        }
        assert!(
            parse(&["--blank", "--new-session"])
                .expect("plain launch")
                .command
                .is_none()
        );
    }

    #[test]
    fn the_server_and_its_link_are_one_flag_alone() {
        assert_eq!(
            parse_sync_mode(["--server"].map(Into::into).into_iter()).expect("server"),
            Some(SyncMode::Server(horizon_sync::ServerOptions::default()))
        );
        assert_eq!(
            parse_sync_mode(["--server-link"].map(Into::into).into_iter()).expect("link"),
            Some(SyncMode::Link)
        );
        assert_eq!(
            parse_sync_mode(["--server-invite"].map(Into::into).into_iter()).expect("invite"),
            Some(SyncMode::Invite)
        );
        assert_eq!(
            parse_sync_mode(["--blank"].map(Into::into).into_iter()).expect("the desktop"),
            None
        );
        assert_eq!(parse_sync_mode(std::iter::empty()).expect("no argument"), None);
        for bad in [
            &["--server", "--config"][..],
            &["--server-link", "extra"],
            &["--server-invite", "extra"],
            &["--server", "--listen"],
            &["--server", "--port", "7000"],
            &["--server", "--listen", "127.0.0.1:7000", "extra"],
        ] {
            assert!(
                parse_sync_mode(bad.iter().map(Into::into)).is_err(),
                "{bad:?} must not fall through to the desktop"
            );
        }
    }

    #[test]
    fn the_server_takes_the_address_of_its_listener_and_the_name_of_its_links() {
        let mode = parse_sync_mode(
            ["--server", "--listen", "127.0.0.1:7000", "--name", "server-computer"]
                .map(Into::into)
                .into_iter(),
        )
        .expect("a server with a listener")
        .expect("the server mode");
        let SyncMode::Server(options) = mode else {
            panic!("the server mode");
        };
        assert_eq!(options.listen.as_deref(), Some("127.0.0.1:7000"));
        assert_eq!(options.name.as_deref(), Some("server-computer"));
        let info = options
            .listener()
            .expect("the listener of the mode")
            .expect("one listener");
        assert_eq!(info.address().to_string(), "127.0.0.1:7000");
        assert_eq!(
            info.join_link("ABCD-EFGH-JKMN").expect("a link"),
            "horizon://server-computer:7000/join#ABCD-EFGH-JKMN",
            "the name of the server is the host of a link"
        );

        // A person never writes a wildcard address, and the rule says so.
        let wildcard = parse_sync_mode(["--server", "--listen", "0.0.0.0:7000"].map(Into::into).into_iter())
            .expect("the mode parses")
            .expect("the server mode");
        let SyncMode::Server(options) = wildcard else {
            panic!("the server mode");
        };
        let error = options.listener().expect_err("a wildcard address is refused");
        assert!(error.to_string().contains("100.64.0.0/10"), "{error}");
        assert!(
            horizon_sync::ServerOptions::default()
                .listener()
                .expect("no listener")
                .is_none(),
            "the listener is off without --listen"
        );
    }

    #[test]
    fn a_profile_command_takes_its_path_and_runs_alone() {
        let args = parse(&["--config", "c.json", "--export-remote-profile", "out.json"]).expect("one profile command");
        assert_eq!(args.config_path.as_deref(), Some(std::path::Path::new("c.json")));
        assert_eq!(
            args.command,
            Some(Command::RemoteProfile(RemoteProfileCommand::Export("out.json".into())))
        );

        for bad in [
            &["--export-remote-profile", "a.json", "--import-remote-profile", "b.json"][..],
            &["--export-remote-profile", "a.json", "--export-remote-profile", "b.json"],
        ] {
            assert!(parse(bad).is_err(), "{bad:?}");
        }
        let error = parse(&["--export-config"]).expect_err("the removed export is refused");
        assert!(error.contains("settings.sqlite"), "{error}");
    }
}
