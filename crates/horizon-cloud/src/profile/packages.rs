//! Private dependency packages restored on the owner's computer, so no package
//! credential reaches a worker.
use super::ProfileError;
use serde::{Deserialize, Serialize};

/// The optional `source.packages` block: a command that restores the repository's
/// dependency packages into a folder on this computer, and the variable that points a
/// worker session at its copy of that folder, such as `NUGET_PACKAGES`.
///
/// The command runs on the owner's computer with the owner's own credentials, so it
/// runs only once the owner has allowed this exact command for this checkout.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Packages {
    /// The program and its arguments, run without a shell in a copy of the committed
    /// tree. [`Packages::DIRECTORY`] stands for the folder it restores into.
    pub restore: Vec<String>,
    /// The environment variable each worker session reads the restored folder from.
    pub env: String,
}

impl Packages {
    /// Replaced by the absolute path of the folder the command restores into.
    pub const DIRECTORY: &'static str = "{dir}";
    const MAX_ARGUMENTS: usize = 32;
    const MAX_ARGUMENT_CHARS: usize = 1024;
    const MAX_ENV_CHARS: usize = 128;
    /// Set by the worker for every session, as in `horizon-worker-session-env`.
    const WORKER_OWNED: [&'static str; 8] = [
        "PATH",
        "HOME",
        "DISPLAY",
        "HORIZON",
        "DISABLE_AUTOUPDATER",
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_WORKSPACE_ID",
        "ANTHROPIC_CUSTOM_HEADERS",
    ];

    pub(super) fn validate(&self) -> Result<(), ProfileError> {
        let arguments_valid = !self.restore.is_empty()
            && self.restore.len() <= Self::MAX_ARGUMENTS
            && self.restore.iter().all(|argument| {
                !argument.is_empty()
                    && argument.chars().count() <= Self::MAX_ARGUMENT_CHARS
                    && !argument.chars().any(char::is_control)
            })
            && !self.restore[0].contains(Self::DIRECTORY)
            && self.restore.iter().any(|argument| argument.contains(Self::DIRECTORY));
        if !arguments_valid {
            return Err(ProfileError::Invalid(
                "source.packages.restore must be a program and at most 31 arguments, each non-empty, at most 1,024 characters and without control characters, with {dir} in an argument",
            ));
        }
        if !Self::valid_env(&self.env) {
            return Err(ProfileError::Invalid(
                "source.packages.env must be an environment variable name of at most 128 letters, digits and underscores, not starting with a digit, and not one the worker sets itself (PATH, HOME, DISPLAY, HORIZON*, LD_* and the agent keys)",
            ));
        }
        Ok(())
    }

    fn valid_env(name: &str) -> bool {
        let mut bytes = name.bytes();
        bytes
            .next()
            .is_some_and(|first| first.is_ascii_alphabetic() || first == b'_')
            && bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_')
            && name.len() <= Self::MAX_ENV_CHARS
            && !Self::WORKER_OWNED.contains(&name)
            && !name.starts_with("HORIZON_")
            && !name.starts_with("LD_")
    }

    /// The command's arguments with [`Self::DIRECTORY`] replaced by `directory`.
    #[must_use]
    pub fn arguments(&self, directory: &str) -> Vec<String> {
        self.restore
            .iter()
            .map(|argument| argument.replace(Self::DIRECTORY, directory))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packages(restore: &[&str], env: &str) -> Packages {
        Packages {
            restore: restore.iter().map(|argument| (*argument).to_owned()).collect(),
            env: env.to_owned(),
        }
    }

    #[test]
    fn a_restore_is_a_program_with_the_folder_in_an_argument() {
        assert!(
            packages(&["dotnet", "restore", "--packages", "{dir}"], "NUGET_PACKAGES")
                .validate()
                .is_ok()
        );
        assert!(
            packages(&["npm", "ci", "--cache={dir}"], "npm_config_cache")
                .validate()
                .is_ok()
        );
        for restore in [
            &[][..],
            &["dotnet", "restore"],
            &["{dir}/restore"],
            &["dotnet", ""],
            &["dotnet", "restore\n--packages", "{dir}"],
        ] {
            assert!(packages(restore, "NUGET_PACKAGES").validate().is_err(), "{restore:?}");
        }
        let long = "a".repeat(1025);
        assert!(
            packages(&["dotnet", &long, "{dir}"], "NUGET_PACKAGES")
                .validate()
                .is_err()
        );
        let many = vec!["{dir}"; 32];
        let mut restore = vec!["dotnet"];
        restore.extend(many);
        assert!(packages(&restore, "NUGET_PACKAGES").validate().is_err());
    }

    #[test]
    fn the_variable_is_never_one_the_worker_owns() {
        for env in ["NUGET_PACKAGES", "_CACHE", "npm_config_cache"] {
            assert!(Packages::valid_env(env), "{env}");
        }
        for env in [
            "",
            "1CACHE",
            "NUGET-PACKAGES",
            "PATH",
            "HOME",
            "HORIZON",
            "HORIZON_SESSION_DIR",
            "LD_PRELOAD",
            "ANTHROPIC_API_KEY",
        ] {
            assert!(!Packages::valid_env(env), "{env}");
        }
        assert!(!Packages::valid_env(&"A".repeat(129)));
    }

    #[test]
    fn the_folder_replaces_every_placeholder() {
        assert_eq!(
            packages(&["tool", "--out={dir}", "{dir}/x"], "CACHE").arguments("/tmp/p"),
            ["tool", "--out=/tmp/p", "/tmp/p/x"]
        );
    }
}
