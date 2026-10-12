//! What the proof reads from the setup and the repository before it runs: the images
//! deployment would build, and whether the repository restores .NET packages, which
//! this computer restores first when the person allows it. The packages that restore
//! used are copied into the proof's volume for the worker's account, so the CI reads
//! only those and runs their tools as their owner.
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

/// The most packages the proof copies into its volume.
const PACKAGE_LIMIT: usize = 5000;

/// The largest restore output Horizon reads.
const ASSETS_LIMIT: u64 = 64 * 1024 * 1024;

/// Where the packages the host restored are in the proof's volume.
const PACKAGES: &str = "/workspace/nuget-packages";

/// Copies the packages named after the script, as `id/version` folders of this computer's
/// package folder, into the volume, owned by the worker's account so their tools run.
const COPY_PACKAGES: &str = "set -e; rm -rf /workspace/nuget-packages; mkdir /workspace/nuget-packages; \
for package; do if [ -d \"/host-nuget/$package\" ]; then mkdir -p \"/workspace/nuget-packages/$package\"; \
cp -R \"/host-nuget/$package/.\" \"/workspace/nuget-packages/$package/\"; fi; done; \
chown -R 10001:10001 /workspace/nuget-packages";

/// A package folder a restore output names: `id/version` in lower case, each a plain
/// name, so it never leaves the package folder.
fn package_folder(path: &str) -> bool {
    let plain = |part: &str| {
        !part.is_empty()
            && !part.starts_with('.')
            && part.bytes().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'-' | b'_' | b'+')
            })
    };
    let mut parts = path.split('/');
    matches!((parts.next(), parts.next(), parts.next()), (Some(id), Some(version), None) if plain(id) && plain(version))
}

/// The package folders the restores in `tree` used, from each project's
/// `obj/project.assets.json`, sorted and once each.
fn restored_packages(tree: &Path) -> Vec<String> {
    let mut packages = Vec::new();
    let mut pending = vec![(tree.to_path_buf(), 0)];
    while let Some((folder, depth)) = pending.pop() {
        for entry in std::fs::read_dir(&folder).into_iter().flatten().flatten() {
            let Ok(kind) = entry.file_type() else { continue };
            let path = entry.path();
            if kind.is_dir() && depth < 6 && entry.file_name() != "node_modules" {
                pending.push((path, depth + 1));
            } else if kind.is_file()
                && entry.file_name() == "project.assets.json"
                && folder.file_name().is_some_and(|name| name == "obj")
            {
                packages.extend(packages_in(&path));
            }
        }
    }
    packages.sort();
    packages.dedup();
    packages.truncate(PACKAGE_LIMIT);
    packages
}

/// The package folders one restore output names.
fn packages_in(assets: &Path) -> Vec<String> {
    use std::io::Read as _;
    let mut text = Vec::new();
    let read = std::fs::File::open(assets).and_then(|file| file.take(ASSETS_LIMIT).read_to_end(&mut text));
    let Ok(assets) = read
        .map_err(|_| ())
        .and_then(|_| serde_json::from_slice::<serde_json::Value>(&text).map_err(|_| ()))
    else {
        return Vec::new();
    };
    assets["libraries"]
        .as_object()
        .into_iter()
        .flat_map(|libraries| libraries.values())
        .filter(|library| library["type"] == "package")
        .filter_map(|library| library["path"].as_str())
        .filter(|path| package_folder(path))
        .map(str::to_owned)
        .collect()
}

impl super::Job {
    /// Restores a .NET repository's packages on this computer first, with its own feed
    /// credentials, then copies the packages that restore used into the proof's volume,
    /// where the container finds private ones as a fallback folder and no credential ever
    /// enters it. It runs the repository's build files here with this computer's feed
    /// logins, so only when the person allowed it in the dialog, and from deployment's
    /// snapshot of the commit the proof tests, never from the live folder the agent can
    /// still change. A restore the person asked for that fails fails the proof.
    pub(super) fn warm_packages(&self, log: &mut Option<std::fs::File>) -> Result<(), String> {
        let Some(head) = self.head().filter(|_| self.host_restore) else {
            return Ok(());
        };
        let folder = tempfile::Builder::new()
            .prefix("horizon-proof-restore-")
            .tempdir()
            .map_err(|error| format!("cannot make a private folder for the restore on this computer: {error}"))?;
        let runner = horizon_core::cloud_runtime::command::Runner {
            cancel: &self.cancel,
            emit: &|_| {},
            secrets: Vec::new(),
        };
        let tree =
            match horizon_core::cloud_runtime::repository::snapshot(&self.repository, head, folder.path(), &runner) {
                Ok(tree) => tree,
                Err(error) => return Err(format!("Horizon cannot restore packages on this computer: {error}")),
            };
        let targets = restore_targets(&tree);
        *self.restored.borrow_mut() = targets
            .iter()
            .filter_map(|target| target.strip_prefix(&tree).ok())
            .map(|target| target.to_string_lossy().into_owned())
            .collect();
        for target in &targets {
            let mut restore = Command::new("dotnet");
            restore
                .current_dir(&tree)
                .arg("restore")
                .arg(target)
                .args(["--nologo", "-v", "quiet"])
                .env("DOTNET_CLI_TELEMETRY_OPTOUT", "1")
                .env("DOTNET_NOLOGO", "1");
            self.stream(restore, log).map_err(|error| {
                let name = target.strip_prefix(&tree).unwrap_or(target).display();
                format!(
                    "dotnet restore of {name} on this computer failed ({error}); fix it here or turn off the restore on this computer"
                )
            })?;
        }
        self.copy_packages(&restored_packages(&tree), log)
    }

    /// Copies the package folders the host restore used into the proof's volume with
    /// Horizon's base image, owned by the worker's account: `NuGet` leaves a package's
    /// tools executable only for the user who restored it, and the CI reads only these.
    fn copy_packages(&self, packages: &[String], log: &mut Option<std::fs::File>) -> Result<(), String> {
        if packages.is_empty() {
            return Ok(());
        }
        let host = std::env::var_os("HOME")
            .map(|home| std::path::PathBuf::from(home).join(".nuget/packages"))
            .filter(|folder| folder.is_dir())
            .ok_or("the restore on this computer left no NuGet package folder")?;
        let mut copy = self.docker();
        copy.args(["run", "--rm", "--name", &self.container, "-v"])
            .arg(format!("{}:/workspace", self.volume))
            .arg("-v")
            .arg(format!("{}:/host-nuget:ro", host.display()))
            .args(["--entrypoint", "sh", super::TOOLS, "-c", COPY_PACKAGES, "sh"])
            .args(packages);
        self.stream(copy, log)
            .map_err(|error| format!("cannot copy the restored packages into the proof's volume: {error}"))?;
        self.packages_copied.set(true);
        Ok(())
    }

    /// The fallback folder the CI's container reads restored packages from, once the
    /// host restore copied them into the volume.
    pub(super) fn package_folder(&self) -> Option<&'static str> {
        self.packages_copied.get().then_some(PACKAGES)
    }

    /// Restores in the CI's container what the host restored, from the packages copied
    /// into the volume and without a credential, so the checkout has the restore's
    /// output that a CI step left out for its credentials would have made. A restore that
    /// fails here fails the proof.
    pub(super) fn restore_in_container(
        &self,
        session: &[String],
        log: &mut Option<std::fs::File>,
    ) -> Result<(), String> {
        let targets = self.restored.borrow().clone();
        if targets.is_empty() {
            return Ok(());
        }
        let mut command = self.ci_container()?;
        command
            .args(session)
            .args(["--entrypoint", "sh", &self.image, "-c"])
            .arg("for target; do dotnet restore \"$target\" --nologo -v quiet || exit 1; done")
            .arg("sh")
            .args(&targets);
        self.stream(command, log).map_err(|error| {
            format!("dotnet restore in the container failed after the restore on this computer ({error})")
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_copy_takes_the_package_folders_the_restores_used_and_nothing_outside() {
        let tree = tempfile::tempdir().unwrap();
        let assets = |project: &str, libraries: serde_json::Value| {
            let obj = tree.path().join(project).join("obj");
            std::fs::create_dir_all(&obj).unwrap();
            std::fs::write(
                obj.join("project.assets.json"),
                serde_json::to_vec(&serde_json::json!({"libraries": libraries})).unwrap(),
            )
            .unwrap();
        };
        assets(
            "src/App",
            serde_json::json!({
                "Newtonsoft.Json/13.0.3": {"type": "package", "path": "newtonsoft.json/13.0.3"},
                "runtime.linux-x64.Microsoft.DotNet.ILCompiler/11.0.0-rc.1": {"type": "package", "path": "runtime.linux-x64.microsoft.dotnet.ilcompiler/11.0.0-rc.1"},
                "App.Core/1.0.0": {"type": "project", "path": "../Core/App.Core.csproj"},
                "Evil/1.0.0": {"type": "package", "path": "../../.ssh/1.0.0"}
            }),
        );
        assets(
            "tests/App.Tests",
            serde_json::json!({"Newtonsoft.Json/13.0.3": {"type": "package", "path": "newtonsoft.json/13.0.3"}}),
        );
        std::fs::write(tree.path().join("project.assets.json"), "{}").unwrap();
        assert_eq!(
            restored_packages(tree.path()),
            vec![
                "newtonsoft.json/13.0.3".to_owned(),
                "runtime.linux-x64.microsoft.dotnet.ilcompiler/11.0.0-rc.1".to_owned()
            ],
            "packages once each, never a project or a path that leaves the package folder"
        );
        for path in ["a/b/c", "a", "../a/1", ".a/1", "a/..", "A/1", "a/1 2", ""] {
            assert!(!package_folder(path), "{path:?}");
        }
    }

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
