//! The `frame_sources` section of the settings: the frame source programs that a
//! person named, and the one command line of each of them.
//!
//! A native Device panel shows the pictures of a VNC desktop today. This section
//! adds the second kind of source: a program of this computer that gives raw
//! pictures over a socket, as [`crate::frame_source`] states. Horizon holds no
//! codec and no library of such a program, so the section holds the whole of what
//! Horizon knows about it: a name for the Settings window and an agent, and the
//! command line that starts the program.
//!
//! The line is split into the arguments of one program and never given to a
//! shell, exactly as the recorder line of speech input is. It holds `{target}` at
//! most once, as a whole argument, and an agent that names this source can fill
//! that one argument and nothing else.

use std::ffi::OsString;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::bounded_program::ProgramEnvironment;
use crate::speech::CommandLine;

/// The placeholder that one argument of a line replaces with the target of the
/// viewer that an agent asked for.
pub const TARGET_PLACEHOLDER: &str = "{target}";

/// The longest name of one source, in characters.
pub const MAX_NAME_CHARS: usize = 40;

/// The longest target, in characters.
pub const MAX_TARGET_CHARS: usize = 255;

/// The most sources that one Horizon keeps.
pub const MAX_SOURCES: usize = 64;

/// The frame source programs of this Horizon, by name.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub struct FrameSourcesConfig {
    pub sources: Vec<FrameSource>,
}

/// One program that gives pictures: the name that a person and an agent use, and
/// the line that starts it.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub struct FrameSource {
    pub name: String,
    pub command: String,
}

/// Why a name and a target are not one run of a frame source program.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum FrameSourceError {
    /// No source of this Horizon has that name. The names of this Horizon answer,
    /// and never a command.
    #[error("no frame source of this Horizon is named {name:?}; the names of this Horizon are {listed}")]
    UnknownName { name: String, listed: String },
    /// The name of a source of the settings is not one name.
    #[error("frame_sources: {0}")]
    InvalidName(String),
    /// The line of a source of the settings is not one command line.
    #[error("frame_sources: {0}")]
    InvalidCommand(String),
    /// The target of an agent is not one target.
    #[error("the target of a frame source viewer is not one target: {0}")]
    InvalidTarget(String),
}

impl FrameSourceError {
    /// The code that a caller of `device_panel` branches on.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::UnknownName { .. } => "unknown_frame_source",
            Self::InvalidTarget(_) => "invalid_request",
            Self::InvalidName(_) | Self::InvalidCommand(_) => "invalid_frame_source",
        }
    }

    /// The names of this Horizon, for a caller that must answer them.
    #[must_use]
    pub fn names_of(horizon: &[String]) -> String {
        if horizon.is_empty() {
            "none".to_owned()
        } else {
            horizon.join(", ")
        }
    }
}

/// One run of a frame source program: the program, its arguments and the name
/// that a person and an agent use.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrameSourcePlan {
    pub name: String,
    pub program: String,
    pub arguments: Vec<OsString>,
}

impl FrameSourcePlan {
    /// The path of this program in the folders of `environment`, or `None` when no
    /// folder of it holds the program.
    #[must_use]
    pub fn installed(&self, environment: &ProgramEnvironment) -> Option<std::path::PathBuf> {
        environment.find(&self.program)
    }
}

impl FrameSourcesConfig {
    /// The names of every source, in the order of the section.
    #[must_use]
    pub fn names(&self) -> Vec<String> {
        self.sources
            .iter()
            .map(|source| source.name.trim().to_owned())
            .filter(|name| !name.is_empty())
            .collect()
    }

    /// The source with that name, when this Horizon holds one.
    #[must_use]
    pub fn find(&self, name: &str) -> Option<&FrameSource> {
        let name = name.trim();
        self.sources.iter().find(|source| source.name.trim() == name)
    }

    /// The run of `name` with `target` in place of `{target}`, or no run.
    ///
    /// # Errors
    /// Reports a name that this Horizon does not hold and a target that is not one
    /// target. A source whose line was refused is not a run either, so a section
    /// that a person broke answers `InvalidCommand` here as well.
    pub fn plan(&self, name: &str, target: Option<&str>) -> Result<FrameSourcePlan, FrameSourceError> {
        let target = target.map(check_target).transpose()?;
        let Some(source) = self.find(name) else {
            return Err(FrameSourceError::UnknownName {
                name: name.trim().to_owned(),
                listed: FrameSourceError::names_of(&self.names()),
            });
        };
        let line = CommandLine::parse(source.command.trim()).map_err(|error| {
            FrameSourceError::InvalidCommand(format!("frame_sources.{}: {error}", source.name.trim()))
        })?;
        let target = target.unwrap_or_default();
        Ok(FrameSourcePlan {
            name: source.name.trim().to_owned(),
            program: line.program().to_owned(),
            arguments: line.arguments_with(TARGET_PLACEHOLDER, &target),
        })
    }

    /// Why this section cannot be used, in the words of its rule.
    ///
    /// # Errors
    /// A name that is not one name, a name that two sources hold, a line that is
    /// not one command line, a line that holds `{target}` twice, and a section
    /// above [`MAX_SOURCES`].
    pub fn validate(&self) -> Result<(), String> {
        if self.sources.len() > MAX_SOURCES {
            return Err(format!(
                "frame_sources holds {} programs, and this Horizon keeps at most {MAX_SOURCES}",
                self.sources.len()
            ));
        }
        let mut seen: Vec<String> = Vec::new();
        for source in &self.sources {
            let name = check_name(&source.name)?;
            if seen.contains(&name) {
                return Err(format!("frame_sources holds the name {name:?} twice"));
            }
            let line = CommandLine::parse(source.command.trim())
                .map_err(|error| format!("frame_sources.{name}: {error}"))?;
            if line.whole_arguments(TARGET_PLACEHOLDER) > 1 {
                return Err(format!(
                    "frame_sources.{name} holds {TARGET_PLACEHOLDER} more than once as a whole argument"
                ));
            }
            seen.push(name);
        }
        Ok(())
    }
}

impl FrameSource {
    /// This entry as one line of the Settings window.
    #[must_use]
    pub fn is_set(&self) -> bool {
        !self.name.trim().is_empty() && !self.command.trim().is_empty()
    }
}

fn check_name(name: &str) -> Result<String, String> {
    let name = name.trim();
    let valid = !name.is_empty()
        && name.chars().count() <= MAX_NAME_CHARS
        && name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'));
    if !valid {
        return Err(format!(
            "frame_sources: a name is 1 to {MAX_NAME_CHARS} characters of A-Z, a-z, 0-9, - and _"
        ));
    }
    Ok(name.to_owned())
}

/// One target of an agent: text that replaces one argument of the line of the
/// settings, and never an argument that an agent adds.
fn check_target(target: &str) -> Result<String, FrameSourceError> {
    let target = target.trim();
    let valid = !target.is_empty()
        && target.chars().count() <= MAX_TARGET_CHARS
        && !target.starts_with('-')
        && target
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | ':' | '/' | '-'));
    if !valid {
        return Err(FrameSourceError::InvalidTarget(format!(
            "it is 1 to {MAX_TARGET_CHARS} characters of A-Z, a-z, 0-9, '.', '_', ':', '/' and '-', and it cannot \
             start with '-'"
        )));
    }
    Ok(target.to_owned())
}

/// One command line of this section as the arguments of one program, for a test
/// and for a caller that must show what a source runs.
#[must_use]
pub fn program_of(line: &str) -> Option<String> {
    let line = CommandLine::parse(line.trim()).ok()?;
    Some(line.program().to_owned())
}

/// The program of one target, for the sentence of a refusal: a name that is not on
/// this computer.
#[must_use]
pub fn missing_program(plan: &FrameSourcePlan, environment: &ProgramEnvironment) -> Option<String> {
    match plan.installed(environment) {
        Some(_) => None,
        None => Some(format!(
            "{} is not installed on this computer",
            Path::new(&plan.program).display()
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Config;

    fn section(entries: &[(&str, &str)]) -> FrameSourcesConfig {
        FrameSourcesConfig {
            sources: entries
                .iter()
                .map(|(name, command)| FrameSource {
                    name: (*name).to_owned(),
                    command: (*command).to_owned(),
                })
                .collect(),
        }
    }

    #[test]
    fn the_default_section_names_no_program() {
        let sources = FrameSourcesConfig::default();
        assert!(sources.sources.is_empty());
        assert!(sources.names().is_empty());
        assert!(sources.validate().is_ok());
        assert_eq!(sources.plan("anything", None), Err(FrameSourceError::UnknownName {
            name: "anything".into(),
            listed: "none".into(),
        }));
    }

    #[test]
    fn a_name_finds_its_line_and_the_target_replaces_one_whole_argument() {
        let sources = section(&[("bench", "frame-tool --size 1920x1080 {target}")]);
        let plan = sources.plan("bench", Some("lab:1")).expect("the run of a named source");
        assert_eq!(plan.name, "bench");
        assert_eq!(plan.program, "frame-tool");
        assert_eq!(plan.arguments, [OsString::from("--size"), "1920x1080".into(), "lab:1".into()]);

        // A viewer that names no target passes an empty argument for the
        // placeholder, so the argument count of the line never changes.
        let plan = sources.plan("bench", None).expect("the run without a target");
        assert_eq!(plan.arguments, [OsString::from("--size"), "1920x1080".into(), String::new().into()]);
    }

    #[test]
    fn an_unknown_name_lists_the_names_and_never_a_command() {
        let sources = section(&[("bench", "frame-tool --secret-line {target}"), ("lab", "other-tool")]);
        let error = sources.plan("missing", None).expect_err("an unknown name");
        assert_eq!(error.code(), "unknown_frame_source");
        let text = error.to_string();
        assert!(text.contains("bench, lab"), "{text}");
        assert!(!text.contains("frame-tool"), "the answer holds no command: {text}");
    }

    #[test]
    fn a_target_follows_its_rule() {
        let sources = section(&[("bench", "frame-tool {target}")]);
        for target in ["", "-starts-with-a-dash", "has space", "a\"b", "a\nb", &"a".repeat(256)] {
            let error = sources.plan("bench", Some(target)).expect_err("a target that is refused");
            assert_eq!(error.code(), "invalid_request", "{target:?}: {error}");
        }
        for target in ["a", "lab:1/desk_2", "a.b-c", &"a".repeat(255)] {
            sources.plan("bench", Some(target)).expect("a target of the rule");
        }
    }

    #[test]
    fn the_line_of_a_source_is_split_and_never_given_to_a_shell() {
        let sources = section(&[("bench", "frame-tool 'one argument' {target}; rm -rf ~")]);
        let plan = sources.plan("bench", Some("lab")).expect("a run of one program");
        assert_eq!(
            plan.arguments,
            [
                OsString::from("one argument"),
                "lab;".into(),
                "rm".into(),
                "-rf".into(),
                "~".into()
            ]
        );
    }

    #[test]
    fn a_name_a_command_and_a_placeholder_follow_their_rules() {
        for entries in [
            vec![("", "frame-tool")],
            vec![("has space", "frame-tool")],
            vec![("a-name-that-is-longer-than-forty-characters-in-total", "t")],
            vec![("bench", "frame-tool 'open")],
            vec![("bench", "")],
            vec![("bench", "frame-tool {target} {target}")],
            vec![("bench", "one"), ("bench", "two")],
        ] {
            let error = section(&entries).validate().expect_err("a section that is refused");
            assert!(!error.is_empty());
        }
        section(&[("bench_1", "frame-tool --out={target}")])
            .validate()
            .expect("a placeholder inside an argument is not a second target");
    }

    #[test]
    fn the_section_round_trips_through_the_settings_json() {
        let mut config = Config::default();
        config.frame_sources = section(&[("bench", "frame-tool {target}")]);
        let json = config.to_json().expect("the settings as JSON");
        assert!(json.contains("\"frame_sources\""), "{json}");
        let loaded = Config::from_json(&json).expect("the settings from JSON");
        assert_eq!(loaded.frame_sources, config.frame_sources);
        loaded.validate().expect("the section of this settings");
    }

    #[test]
    fn the_section_follows_the_server_as_the_recorder_of_speech_does() {
        assert_eq!(crate::settings_sync::scope("frame_sources"), Some(crate::settings_sync::Scope::Shared));
        assert!(crate::settings_sync::is_shared("frame_sources"));
    }

    #[test]
    fn a_program_that_no_folder_of_the_environment_holds_is_named() {
        let sources = section(&[("bench", "frame-tool-missing {target}")]);
        let plan = sources.plan("bench", None).expect("a run");
        let environment = ProgramEnvironment::with_path("/nonexistent-folder-for-this-test");
        assert_eq!(
            missing_program(&plan, &environment).as_deref(),
            Some("frame-tool-missing is not installed on this computer")
        );
        assert_eq!(program_of("frame-tool --flag"), Some("frame-tool".to_owned()));
    }
}
