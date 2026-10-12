//! What the proof reads from the setup and the repository before it runs: the images
//! deployment would build, and whether the repository restores .NET packages, which
//! this computer restores first when the person allows it.
use std::path::Path;
use std::process::Command;

/// The images deployment would build: each profile's build with the build arguments
/// its capabilities give, the default profile's first when it has one, every other one
/// that differs from all before it. A profile without a build section runs a published
/// image.
pub(super) fn distinct_builds(
    config: &horizon_core::cloud_panel::CloudConfig,
    arguments: impl Fn(&horizon_core::cloud_panel::Profile) -> Vec<String>,
) -> Result<Vec<(horizon_core::cloud_panel::Build, Vec<String>)>, String> {
    let default = config
        .profiles
        .get(&config.default)
        .ok_or("the default profile is missing")?;
    let mut builds: Vec<_> = default
        .build
        .clone()
        .map(|build| (build, arguments(default)))
        .into_iter()
        .collect();
    for profile in config.profiles.values() {
        if let Some(build) = &profile.build {
            let candidate = (build.clone(), arguments(profile));
            if !builds.contains(&candidate) {
                builds.push(candidate);
            }
        }
    }
    Ok(builds)
}

/// What a bounded walk near the top of a repository finds of .NET: whether it has a
/// package configuration, its solutions and its projects, and whether the walk stopped
/// at its bound. The walk never follows links and skips hidden and build folders.
#[derive(Default)]
struct Dotnet {
    configs: bool,
    solutions: Vec<std::path::PathBuf>,
    projects: Vec<std::path::PathBuf>,
    bounded: bool,
}

fn dotnet(repository: &Path) -> Dotnet {
    const CONFIGS: [&str; 3] = ["nuget.config", "NuGet.Config", "Directory.Packages.props"];
    const SOLUTIONS: [&str; 2] = ["sln", "slnx"];
    const PROJECTS: [&str; 3] = ["csproj", "fsproj", "vbproj"];
    const SKIPPED: [&str; 5] = ["node_modules", "target", "bin", "obj", "vendor"];
    let mut found = Dotnet::default();
    let mut pending = vec![(repository.to_path_buf(), 0)];
    let mut seen = 0;
    while let Some((folder, depth)) = pending.pop() {
        for entry in std::fs::read_dir(&folder).into_iter().flatten().flatten() {
            seen += 1;
            if seen > 5000 {
                found.bounded = true;
                return found;
            }
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let Ok(kind) = entry.file_type() else { continue };
            let extension = Path::new(name.as_ref())
                .extension()
                .map(|extension| extension.to_string_lossy().into_owned())
                .unwrap_or_default();
            if kind.is_dir() {
                if depth < 3 && !name.starts_with('.') && !SKIPPED.contains(&name.as_ref()) {
                    pending.push((entry.path(), depth + 1));
                }
            } else if kind.is_file() {
                if CONFIGS.contains(&name.as_ref()) {
                    found.configs = true;
                } else if SOLUTIONS.contains(&extension.as_str()) {
                    found.solutions.push(entry.path());
                } else if PROJECTS.contains(&extension.as_str()) {
                    found.projects.push(entry.path());
                }
            }
        }
    }
    found
}

/// Whether the repository may restore .NET packages, possibly from private feeds: a
/// package configuration, or a solution or project file near the top. A walk that
/// reaches its bound counts as a possible match, so a large .NET repository still gets
/// the restore option, which is off until the person turns it on.
pub(in crate::app::cloud_panel::production::repository_setup) fn uses_nuget(repository: &Path) -> bool {
    let found = dotnet(repository);
    found.bounded || found.configs || !found.solutions.is_empty() || !found.projects.is_empty()
}

/// The most solutions or projects the host restore restores.
const RESTORE_LIMIT: usize = 10;

/// What `dotnet restore` restores in `tree`: its solutions, or without one its projects,
/// the shallowest first. Named one by one, since `dotnet restore` in a folder without
/// exactly one of them restores nothing.
fn restore_targets(tree: &Path) -> Vec<std::path::PathBuf> {
    let found = dotnet(tree);
    let mut targets = if found.solutions.is_empty() {
        found.projects
    } else {
        found.solutions
    };
    targets.sort_by_key(|path| (path.components().count(), path.clone()));
    targets.truncate(RESTORE_LIMIT);
    targets
}

impl super::Job {
    /// Restores a .NET repository's packages on this computer first, with its own feed
    /// credentials, so the container finds private packages in the read-only fallback
    /// folder and no credential ever enters it. It runs the repository's build files
    /// here with this computer's feed logins, so only when the person allowed it in the
    /// dialog, and from deployment's snapshot of the commit the proof tests, never from
    /// the live folder the agent can still change. Best effort: a failure here shows up
    /// in the CI step instead.
    pub(super) fn warm_packages(&self, log: &mut Option<std::fs::File>) {
        let Some(head) = self.head().filter(|_| self.host_restore) else {
            return;
        };
        let Ok(folder) = tempfile::Builder::new().prefix("horizon-proof-restore-").tempdir() else {
            return;
        };
        let runner = horizon_core::cloud_runtime::command::Runner {
            cancel: &self.cancel,
            emit: &|_| {},
            secrets: Vec::new(),
        };
        let tree =
            match horizon_core::cloud_runtime::repository::snapshot(&self.repository, head, folder.path(), &runner) {
                Ok(tree) => tree,
                Err(error) => {
                    self.record(log, format!("Horizon did not restore packages here: {error}"));
                    return;
                }
            };
        for target in restore_targets(&tree) {
            let mut restore = Command::new("dotnet");
            restore
                .current_dir(&tree)
                .arg("restore")
                .arg(&target)
                .args(["--nologo", "-v", "quiet"])
                .env("DOTNET_CLI_TELEMETRY_OPTOUT", "1")
                .env("DOTNET_NOLOGO", "1");
            let _ = self.stream(restore, log);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_restore_names_the_solutions_or_else_the_projects() {
        let tree = tempfile::tempdir().unwrap();
        let file = |path: &str| {
            let path = tree.path().join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "").unwrap();
        };
        file("src/App/App.csproj");
        file("tests/App.Tests/App.Tests.csproj");
        file("node_modules/pkg/Skipped.csproj");
        let names = |targets: Vec<std::path::PathBuf>| {
            targets
                .iter()
                .map(|target| target.strip_prefix(tree.path()).unwrap().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            names(restore_targets(tree.path())),
            ["src/App/App.csproj", "tests/App.Tests/App.Tests.csproj"],
            "without a solution, each project below the top"
        );
        file("App.sln");
        assert_eq!(
            names(restore_targets(tree.path())),
            ["App.sln"],
            "a solution restores its projects"
        );
    }
}
