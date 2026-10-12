//! What the proof reads from the setup and the repository before it runs: the images
//! deployment would build, and whether the repository restores .NET packages.
use std::path::Path;

/// The images deployment would build: each profile's build with the build arguments
/// its capabilities give, the default profile's first, every other one that differs
/// from all before it. A profile without a build section runs a published image.
pub(super) fn distinct_builds(
    config: &horizon_core::cloud_panel::CloudConfig,
    arguments: impl Fn(&horizon_core::cloud_panel::Profile) -> Vec<String>,
) -> Result<Vec<(horizon_core::cloud_panel::Build, Vec<String>)>, String> {
    let default = config
        .profiles
        .get(&config.default)
        .ok_or("the default profile is missing")?;
    let build = default
        .build
        .clone()
        .ok_or("the default profile has no build section")?;
    let mut builds = vec![(build, arguments(default))];
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

/// Whether the repository restores .NET packages, possibly from private feeds: a package
/// configuration, or a solution or project file near the top. A bounded walk that never
/// follows links and skips hidden and build folders.
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
                return false;
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
