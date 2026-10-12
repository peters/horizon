//! The candidate image's session environment, read without running the image: `docker
//! cp` takes its session-env.d folder out of a created container, and Horizon applies it
//! the way a worker session does. So the CI container starts Horizon's localci directly,
//! and no program of the image runs before it.
use std::process::Command;

/// Where a worker image keeps its session environment.
const FOLDER: &str = "/etc/horizon-worker/session-env.d";

/// The largest session environment file the worker reads, and the most files.
const FILE_LIMIT: u64 = 16 * 1024;
const FILES: usize = 64;

/// The PATH sshd gives a session on Ubuntu when /etc/environment names none.
const SSH_PATH: &str = "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin:/usr/games:/usr/local/games";

/// A docker command run within the proof's time limit and cancellation, returning what
/// it wrote to standard output.
pub(super) type Run<'a> = &'a dyn Fn(Command) -> Result<Vec<u8>, String>;

/// The `-e` arguments that give a container of `image` the environment a worker session
/// gets; the image is created under the name `container` for the copy and removed
/// again. Every docker command goes through `run`, so a daemon that hangs cannot hold
/// the proof.
pub(super) fn arguments(image: &str, container: &str, run: Run<'_>) -> Result<Vec<String>, String> {
    let folder = tempfile::Builder::new()
        .prefix("horizon-proof-env-")
        .tempdir()
        .map_err(|error| error.to_string())?;
    let mut create = Command::new("docker");
    create.args(["create", "--name", container, image]);
    run(create).map_err(|error| format!("cannot read the image's session environment: {error}"))?;
    let copy = |from: &str| {
        let mut copy = Command::new("docker");
        copy.arg("cp").arg(format!("{container}:{from}")).arg(folder.path());
        run(copy)
    };
    let copied = copy(FOLDER);
    let environment = copy("/etc/environment");
    let mut remove = Command::new("docker");
    remove.args(["rm", "--force", container]);
    let _ = run(remove);
    // A missing folder or file means none; any other failure is one.
    for result in [&copied, &environment] {
        if let Err(error) = result
            && !error.contains("Could not find the file")
            && !error.contains("No such")
        {
            return Err(format!("cannot read the image's session environment: {error}"));
        }
    }
    let base = base_path(&folder.path().join("environment"));
    let (variables, directives) =
        load(&folder.path().join("session-env.d")).map_err(|error| format!("session environment refused: {error}"))?;
    let mut arguments: Vec<String> = variables
        .into_iter()
        .flat_map(|(key, value)| ["-e".to_owned(), format!("{key}={value}")])
        .collect();
    arguments.extend(["-e".to_owned(), format!("PATH={}", session_path(&base, &directives))]);
    Ok(arguments)
}

/// The PATH an SSH session of the image starts from: the one its /etc/environment
/// names, which PAM applies, or sshd's own default. The image's `ENV` lines do not
/// reach a session, so they do not count here either.
fn base_path(environment: &std::path::Path) -> String {
    super::super::untrusted::read_regular(environment, FILE_LIMIT)
        .ok()
        .and_then(|bytes| {
            String::from_utf8_lossy(&bytes).lines().find_map(|line| {
                let value = line.trim().strip_prefix("PATH=")?;
                Some(value.trim_matches('"').to_owned())
            })
        })
        .unwrap_or_else(|| SSH_PATH.to_owned())
}

/// Session variables, in order, and PATH directives: whether each prepends, and its folder.
type Loaded = (Vec<(String, String)>, Vec<(bool, String)>);

/// The variables and PATH directives of the session-env.d folder at `folder`, read with
/// the worker's own rules (horizon-worker-session-env), so a file the worker would refuse
/// fails the proof instead of passing it with another environment.
fn load(folder: &std::path::Path) -> Result<Loaded, String> {
    let mut variables: Vec<(String, String)> = Vec::new();
    let mut directives = Vec::new();
    let Ok(metadata) = std::fs::symlink_metadata(folder) else {
        return Ok((variables, directives));
    };
    if !metadata.is_dir() {
        return Err(format!("{FOLDER}: not a directory"));
    }
    let mut entries: Vec<(String, std::path::PathBuf)> = std::fs::read_dir(folder)
        .map_err(|error| error.to_string())?
        .flatten()
        .map(|entry| (entry.file_name().to_string_lossy().into_owned(), entry.path()))
        .collect();
    entries.sort();
    if entries.len() > FILES {
        return Err(format!("{FOLDER}: more than {FILES} files"));
    }
    for (name, path) in entries {
        if !file_name(&name) {
            return Err(format!(
                "{name}: name must be at most 100 letters, digits, \".\", \"_\" or \"-\" ending in .env"
            ));
        }
        let bytes =
            super::super::untrusted::read_regular(&path, FILE_LIMIT).map_err(|error| format!("{name}: {error}"))?;
        let text = String::from_utf8(bytes).map_err(|_| format!("{name}: not UTF-8"))?;
        for (number, line) in text.split('\n').enumerate() {
            let at = format!("{name}:{}", number + 1);
            if line.chars().any(char::is_control) {
                return Err(format!("{at}: control character in line"));
            }
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                return Err(format!("{at}: expected KEY=VALUE, PATH_PREPEND=DIR or PATH_APPEND=DIR"));
            };
            if value.chars().count() > 4096 {
                return Err(format!("{at}: value longer than 4096 characters"));
            }
            if key == "PATH_PREPEND" || key == "PATH_APPEND" {
                if !value.starts_with('/') || value.contains(':') {
                    return Err(format!("{at}: {key} needs one absolute directory without \":\""));
                }
                directives.push((key == "PATH_PREPEND", value.to_owned()));
            } else if !variable_name(key) {
                return Err(format!("{at}: invalid variable name"));
            } else if reserved(key) {
                return Err(format!("{at}: {key} cannot be set by an image layer"));
            } else {
                variables.retain(|(existing, _)| existing != key);
                variables.push((key.to_owned(), value.to_owned()));
            }
        }
    }
    Ok((variables, directives))
}

/// A session-env.d file name the worker accepts.
fn file_name(name: &str) -> bool {
    let allowed = |byte: u8| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-');
    name.len() <= 100
        && std::path::Path::new(name)
            .extension()
            .is_some_and(|extension| extension == "env")
        && name.as_bytes().first().is_some_and(u8::is_ascii_alphanumeric)
        && name.bytes().all(allowed)
}

/// A variable name the worker accepts.
fn variable_name(key: &str) -> bool {
    key.len() <= 128
        && key.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
        && key.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

/// The variables the worker sets or clears for every session.
fn reserved(key: &str) -> bool {
    matches!(
        key,
        "PATH"
            | "HOME"
            | "DISPLAY"
            | "HORIZON"
            | "DISABLE_AUTOUPDATER"
            | "ANTHROPIC_API_KEY"
            | "ANTHROPIC_WORKSPACE_ID"
            | "ANTHROPIC_CUSTOM_HEADERS"
    ) || key.starts_with("HORIZON_")
}

/// The session's PATH: each `PATH_PREPEND` first and each `PATH_APPEND` last, in order, and a
/// directory already on it, from the base or an earlier line, staying where it is.
fn session_path(base: &str, directives: &[(bool, String)]) -> String {
    let mut parts: Vec<String> = Vec::new();
    for part in base.split(':') {
        if !parts.iter().any(|existing| existing == part) {
            parts.push(part.to_owned());
        }
    }
    for (prepend, directory) in directives {
        if parts.contains(directory) {
            continue;
        }
        if *prepend {
            parts.insert(0, directory.clone());
        } else {
            parts.push(directory.clone());
        }
    }
    parts.join(":")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(files: &[(&str, &str)]) -> tempfile::TempDir {
        let folder = tempfile::tempdir().unwrap();
        for (name, text) in files {
            std::fs::write(folder.path().join(name), text).unwrap();
        }
        folder
    }

    #[test]
    fn session_files_apply_the_way_the_worker_applies_them() {
        let files = folder(&[
            (
                "20-tools.env",
                "# tools\nDOTNET_ROOT=/opt/dotnet\nPATH_PREPEND=/opt/dotnet\n",
            ),
            (
                "30-more.env",
                "PATH_APPEND=/opt/extra\nPATH_APPEND=/usr/bin\nDOTNET_ROOT=/opt/other\n",
            ),
        ]);
        let (variables, directives) = load(files.path()).unwrap();
        assert_eq!(
            variables,
            [("DOTNET_ROOT".to_owned(), "/opt/other".to_owned())],
            "a later value replaces"
        );
        assert_eq!(
            session_path("/usr/bin:/bin", &directives),
            "/opt/dotnet:/usr/bin:/bin:/opt/extra"
        );
        assert!(
            load(&files.path().join("missing")).unwrap().0.is_empty(),
            "no folder, no environment"
        );
    }

    #[test]
    fn a_file_the_worker_refuses_fails_the_proof() {
        for (name, text) in [
            ("bad name.env", "A=1\n"),
            ("20-x.env", "PATH=/opt\n"),
            ("20-x.env", "HORIZON_TOKEN=1\n"),
            ("20-x.env", "PATH_PREPEND=relative\n"),
            ("20-x.env", "1X=2\n"),
            ("20-x.env", "A=1\tB\n"),
            ("20-x.env", "no separator\n"),
        ] {
            assert!(load(folder(&[(name, text)]).path()).is_err(), "{name}: {text:?}");
        }
    }

    #[test]
    fn the_base_path_is_the_one_ssh_gives_a_session() {
        let files = folder(&[("environment", "PATH=\"/usr/local/bin:/usr/bin\"\nLANG=C\n")]);
        assert_eq!(base_path(&files.path().join("environment")), "/usr/local/bin:/usr/bin");
        assert_eq!(base_path(&files.path().join("missing")), SSH_PATH);
    }
}
