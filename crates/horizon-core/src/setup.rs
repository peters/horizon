//! Set up: the one action that starts an agent to rent a machine, to add the
//! Horizon server, to set up a browser that stays signed in, or to join a
//! server that the person already has.
//!
//! Horizon does not do the work of Set up itself. It opens a panel of an agent
//! program that the person installed and gives that program one first message,
//! which names the `horizon-setup` skill and the goal. The agent draws the cards
//! of the steps in its own panel.
//!
//! # Which agent
//!
//! Set up starts the agent preset that the settings name first among the agent
//! programs that are installed on this computer, as the person orders them in
//! Settings, Presets. A preset whose program sits on the `PATH` is installed.
//! A program that takes no first message is not offered at all, because the
//! person would then have to describe the goal again by hand: Gemini and
//! `KiloCode` have no argument for an opening message, and Horizon never types
//! one into a program.
//!
//! # The first message
//!
//! The message is one sentence, and this module is its only writer:
//! `Use the horizon-setup skill. The person pressed Set up. Goal: <goal>.`
//! The goal is one of `rent`, `server`, `browser`, `join`, or `choose` when the
//! person pressed the plain control and not one of the four tiles.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use crate::agents::agent_definition;
use crate::config::PresetConfig;
use crate::panel::PanelKind;

/// The skill that does the work of Set up.
pub const SETUP_SKILL: &str = "horizon-setup";

/// The goal that the person had in mind when they pressed Set up.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SetupGoal {
    /// The person pressed Set up without a goal, so the agent asks.
    #[default]
    Choose,
    /// Rent a cloud machine in the person's own provider account.
    Rent,
    /// Set up the Horizon server.
    Server,
    /// Set up a browser that stays signed in.
    Browser,
    /// Join a server that the person already has.
    Join,
}

impl SetupGoal {
    /// Every goal, in the order a person meets them.
    pub const ALL: [Self; 5] = [Self::Choose, Self::Rent, Self::Server, Self::Browser, Self::Join];

    /// The goal as the first message and the agent read it.
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::Choose => "choose",
            Self::Rent => "rent",
            Self::Server => "server",
            Self::Browser => "browser",
            Self::Join => "join",
        }
    }

    /// The one sentence that the agent of Set up reads first.
    #[must_use]
    pub fn message(self) -> String {
        format!(
            "Use the {SETUP_SKILL} skill. The person pressed Set up. Goal: {}.",
            self.word()
        )
    }
}

/// The arguments that give `message` to the agent `kind` as its first message,
/// or `None` when the program of that kind takes no first message.
///
/// Horizon launches an agent in an interactive shell, so these arguments are
/// words after the program on that line: the message is one argument, quoted by
/// the shell path that builds the line.
#[must_use]
pub fn first_message_args(kind: PanelKind, message: &str) -> Option<Vec<String>> {
    match kind {
        // OpenCode reads its opening message from a flag, not from a positional
        // argument.
        PanelKind::OpenCode => Some(vec!["--prompt".to_owned(), message.to_owned()]),
        PanelKind::Claude | PanelKind::Codex | PanelKind::Pi | PanelKind::Grok => Some(vec![message.to_owned()]),
        _ => None,
    }
}

/// Whether the agent program of `kind` takes a first message, so Set up can
/// offer it.
#[must_use]
pub fn takes_first_message(kind: PanelKind) -> bool {
    first_message_args(kind, "message").is_some()
}

/// One agent program that Set up can start: a preset of the settings, the kind
/// of its agent and the program that the search looks for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SetupAgent {
    /// Position of the preset in the settings, so the caller starts exactly the
    /// preset that the person saved.
    pub preset: usize,
    /// The kind of the agent of that preset.
    pub kind: PanelKind,
    /// The program of the preset, or the default program of its kind.
    pub program: String,
}

impl SetupAgent {
    /// The name that the person reads for this program.
    #[must_use]
    pub fn display_name(&self) -> &'static str {
        agent_definition(self.kind).map_or("Agent", |definition| definition.display_name)
    }
}

/// The agent programs that Set up offers, in the order of the settings: one for
/// each preset of an agent that takes a first message.
///
/// A program that two presets name appears once, at its first place, because the
/// first of them is the one that Set up starts.
#[must_use]
pub fn setup_agents(presets: &[PresetConfig]) -> Vec<SetupAgent> {
    let mut agents: Vec<SetupAgent> = Vec::new();
    for (index, preset) in presets.iter().enumerate() {
        if !takes_first_message(preset.kind) {
            continue;
        }
        let program = preset_program(preset);
        if agents
            .iter()
            .any(|agent| agent.kind == preset.kind && agent.program == program)
        {
            continue;
        }
        agents.push(SetupAgent {
            preset: index,
            kind: preset.kind,
            program,
        });
    }
    agents
}

/// The program that a preset starts: its own command when it names one, and the
/// default program of its agent kind otherwise. Only the program is taken, so a
/// command line that also holds arguments still names one program.
#[must_use]
pub fn preset_program(preset: &PresetConfig) -> String {
    preset
        .command
        .as_deref()
        .and_then(|command| command.split_whitespace().next())
        .map(str::to_owned)
        .or_else(|| agent_definition(preset.kind).map(|definition| definition.default_command.to_owned()))
        .unwrap_or_default()
}

/// The first agent, in the order of the settings, whose program is installed.
///
/// `installed` answers whether one program of the `PATH` of this computer is an
/// executable file. The search runs on a worker, because it reads folders.
#[must_use]
pub fn first_installed(agents: &[SetupAgent], mut installed: impl FnMut(&str) -> bool) -> Option<usize> {
    agents
        .iter()
        .position(|agent| !agent.program.is_empty() && installed(&agent.program))
}

/// What the single panel of a workspace of a folder is: the program of one
/// preset of the settings, or the general program of this computer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FolderPanel {
    /// The index of the preset of the settings whose program starts the panel.
    Preset(usize),
    /// A terminal of this computer.
    Terminal,
}

/// The choice of the one panel of a workspace that Horizon makes for a folder.
///
/// The rule is the rule of Set up: the first agent preset of the settings, in the
/// order of the settings, whose program this computer has. The caller gives the
/// answer of whether a program is installed, because the search reads folders and
/// runs off the frame. A computer with no agent program starts a terminal instead.
#[must_use]
pub fn folder_panel(presets: &[PresetConfig], mut installed: impl FnMut(&str) -> bool) -> FolderPanel {
    let agents = setup_agents(presets);
    let programs: Vec<String> = agents.iter().map(|agent| agent.program.clone()).collect();
    let found: Vec<bool> = programs.iter().map(|program| installed(program)).collect();
    agents
        .iter()
        .zip(found)
        .find(|(_, found)| *found)
        .map_or(FolderPanel::Terminal, |(agent, _)| FolderPanel::Preset(agent.preset))
}

/// The file that runs `program` on this computer, or `None` when no folder of
/// `PATH` holds an executable of that name.
///
/// A program that carries a path separator is checked where it says, and never
/// in the folders of `PATH`.
#[must_use]
pub fn find_program(program: &str, path: Option<&OsStr>) -> Option<PathBuf> {
    if program.is_empty() {
        return None;
    }
    if program.contains('/') {
        let file = Path::new(program);
        return is_executable_file(file).then(|| file.to_path_buf());
    }
    let path = path.map(OsStr::to_os_string).or_else(|| std::env::var_os("PATH"))?;
    std::env::split_paths(&path)
        .map(|folder| folder.join(program))
        .find(|file| is_executable_file(file))
}

fn is_executable_file(file: &Path) -> bool {
    let Ok(metadata) = std::fs::metadata(file) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::{
        FolderPanel, SetupGoal, find_program, first_installed, first_message_args, folder_panel, setup_agents,
        takes_first_message,
    };
    use crate::config::PresetConfig;
    use crate::panel::{PanelKind, PanelResume};

    fn preset(name: &str, kind: PanelKind, command: Option<&str>) -> PresetConfig {
        PresetConfig {
            name: name.to_owned(),
            alias: None,
            kind,
            command: command.map(str::to_owned),
            args: Vec::new(),
            resume: PanelResume::Fresh,
            ssh_connection: None,
        }
    }

    #[test]
    fn a_folder_takes_the_first_agent_preset_whose_program_this_computer_has() {
        let presets = vec![
            preset("Shell", PanelKind::Shell, None),
            preset("Gemini", PanelKind::Gemini, None),
            preset("Claude Code", PanelKind::Claude, None),
            preset("Codex", PanelKind::Codex, None),
        ];

        assert_eq!(
            folder_panel(&presets, |program| program == "codex"),
            FolderPanel::Preset(3),
            "only Codex is installed, so its preset starts the panel"
        );
        assert_eq!(
            folder_panel(&presets, |_| true),
            FolderPanel::Preset(2),
            "the order of the settings decides between two installed agents"
        );
        assert_eq!(
            folder_panel(&presets, |_| false),
            FolderPanel::Terminal,
            "a computer with no agent program starts a terminal"
        );
        assert_eq!(
            folder_panel(&presets[..1], |_| true),
            FolderPanel::Terminal,
            "a settings list of one terminal preset starts a terminal"
        );
    }

    #[test]
    fn a_folder_never_takes_an_agent_that_has_no_first_message() {
        let presets = vec![
            preset("Kilo", PanelKind::KiloCode, None),
            preset("Gemini", PanelKind::Gemini, None),
        ];

        assert_eq!(
            folder_panel(&presets, |_| true),
            FolderPanel::Terminal,
            "a program that takes no first message is no agent of a folder"
        );
    }

    #[test]
    fn a_goal_names_the_skill_and_itself_in_the_first_message() {
        assert_eq!(
            SetupGoal::Rent.message(),
            "Use the horizon-setup skill. The person pressed Set up. Goal: rent."
        );
        assert_eq!(
            SetupGoal::Choose.message(),
            "Use the horizon-setup skill. The person pressed Set up. Goal: choose."
        );
        for goal in SetupGoal::ALL {
            assert!(goal.message().ends_with(&format!("Goal: {}.", goal.word())));
            assert_eq!(goal.message().matches("Set up").count(), 1);
        }
    }

    #[test]
    fn only_the_programs_with_a_first_message_are_offered() {
        for kind in [
            PanelKind::Claude,
            PanelKind::Codex,
            PanelKind::Pi,
            PanelKind::Grok,
            PanelKind::OpenCode,
        ] {
            assert!(takes_first_message(kind), "{kind:?} takes a first message");
        }
        for kind in [
            PanelKind::Gemini,
            PanelKind::KiloCode,
            PanelKind::Shell,
            PanelKind::Block,
        ] {
            assert!(
                !takes_first_message(kind),
                "{kind:?} takes no first message, so Set up cannot offer it"
            );
        }

        assert_eq!(
            first_message_args(PanelKind::Claude, "hello"),
            Some(vec!["hello".to_owned()])
        );
        assert_eq!(
            first_message_args(PanelKind::OpenCode, "hello"),
            Some(vec!["--prompt".to_owned(), "hello".to_owned()])
        );
        assert_eq!(first_message_args(PanelKind::Gemini, "hello"), None);
    }

    #[test]
    fn the_agents_keep_the_order_of_the_settings_and_skip_what_set_up_cannot_start() {
        let presets = vec![
            preset("Shell", PanelKind::Shell, None),
            preset("Gemini", PanelKind::Gemini, None),
            preset("Claude", PanelKind::Claude, None),
            preset("Grok fast", PanelKind::Grok, Some("/opt/grok --no-leader")),
            preset("Claude again", PanelKind::Claude, None),
            preset("OpenCode", PanelKind::OpenCode, Some("opencode")),
        ];
        let agents = setup_agents(&presets);
        let described: Vec<(usize, PanelKind, &str)> = agents
            .iter()
            .map(|agent| (agent.preset, agent.kind, agent.program.as_str()))
            .collect();
        assert_eq!(
            described,
            vec![
                (2, PanelKind::Claude, "claude"),
                (3, PanelKind::Grok, "/opt/grok"),
                (5, PanelKind::OpenCode, "opencode"),
            ],
            "the settings order decides, and a repeated program appears once"
        );
        assert_eq!(agents[0].display_name(), "Claude");
    }

    #[test]
    fn the_first_installed_agent_of_the_settings_wins() {
        let presets = vec![
            preset("Claude", PanelKind::Claude, None),
            preset("Codex", PanelKind::Codex, None),
        ];
        let agents = setup_agents(&presets);

        assert_eq!(
            first_installed(&agents, |program| program == "codex"),
            Some(1),
            "the only installed program decides"
        );
        assert_eq!(
            first_installed(&agents, |program| program == "claude" || program == "codex"),
            Some(0),
            "with both installed, the first preset of the settings wins"
        );
        assert_eq!(first_installed(&agents, |_| false), None);
    }

    #[test]
    fn a_program_is_found_only_when_it_is_an_executable_file() {
        let folder = tempfile::tempdir().expect("a folder of its own");
        let first = folder.path().join("first");
        let second = folder.path().join("second");
        std::fs::create_dir_all(&first).expect("first folder");
        std::fs::create_dir_all(&second).expect("second folder");
        let program = crate::util::stand_in_program_in(&second, "agent", "exit 0").expect("a stand-in program");
        let path = std::env::join_paths([first.as_path(), second.as_path()]).expect("a path");

        assert_eq!(
            find_program("agent", Some(&path)),
            Some(program.clone()),
            "the search passes a folder without the file"
        );
        assert_eq!(
            find_program(&program.to_string_lossy(), Some(&path)),
            Some(program.clone()),
            "a program that names its path is used where it says"
        );
        assert_eq!(find_program("missing", Some(&path)), None);
        assert_eq!(find_program("", Some(&path)), None);

        // A file that nobody can run is not the program of this computer.
        let plain = first.join("agent");
        std::fs::write(&plain, "not a program\n").expect("a plain file writes");
        assert_eq!(find_program("agent", Some(&path)), Some(program));
    }
}
