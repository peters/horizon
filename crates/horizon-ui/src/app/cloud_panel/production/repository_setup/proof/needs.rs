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

/// Whether the repository may restore .NET packages, possibly from private feeds: a
/// package configuration, or a solution or project file near the top. A bounded walk
/// that never follows links and skips hidden and build folders. A walk that reaches its
/// bound counts as a possible match, so a large .NET repository still gets the restore
/// option, which is off until the person turns it on.
pub(in crate::app::cloud_panel::production::repository_setup) fn uses_nuget(repository: &Path) -> bool {
    const CONFIGS: [&str; 3] = ["nuget.config", "NuGet.Config", "Directory.Packages.props"];
    const PROJECTS: [&str; 5] = ["sln", "slnx", "csproj", "fsproj", "vbproj"];
    const SKIPPED: [&str; 5] = ["node_modules", "target", "bin", "obj", "vendor"];
    let mut pending = vec![(repository.to_path_buf(), 0)];
    let mut seen = 0;
    while let Some((folder, depth)) = pending.pop() {
        for entry in std::fs::read_dir(&folder).into_iter().flatten().flatten() {
            seen += 1;
            if seen > 5000 {
                return true;
            }
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let Ok(kind) = entry.file_type() else { continue };
            if kind.is_dir() {
                if depth < 3 && !name.starts_with('.') && !SKIPPED.contains(&name.as_ref()) {
                    pending.push((entry.path(), depth + 1));
                }
            } else if kind.is_file()
                && (CONFIGS.contains(&name.as_ref())
                    || Path::new(name.as_ref())
                        .extension()
                        .is_some_and(|extension| PROJECTS.iter().any(|project| extension == *project)))
            {
                return true;
            }
        }
    }
    false
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
        if !uses_nuget(&tree) {
            return;
        }
        let mut restore = Command::new("dotnet");
        restore
            .current_dir(&tree)
            .args(["restore", "--nologo", "-v", "quiet"])
            .env("DOTNET_CLI_TELEMETRY_OPTOUT", "1")
            .env("DOTNET_NOLOGO", "1");
        let _ = self.stream(restore, log);
    }
}
