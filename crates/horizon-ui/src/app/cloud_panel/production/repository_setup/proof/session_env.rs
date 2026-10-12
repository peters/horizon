//! The candidate image's session environment, read without running the image: `docker
//! cp` takes its session-env.d folder out of a created container, and Horizon applies it
//! the way a worker session does. So the CI container starts Horizon's localci directly,
//! and no program of the image runs before it.
use std::process::Command;

/// Where a worker image keeps its session environment.
const FOLDER: &str = "/etc/horizon-worker/session-env.d";

/// The largest session environment file Horizon reads, and the most files.
const FILE_LIMIT: u64 = 64 * 1024;
const FILES: usize = 64;

/// The PATH a container gets when its image names none.
const DEFAULT_PATH: &str = "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin";

/// A docker command run within the proof's time limit and cancellation, returning what
/// it wrote to standard output.
pub(super) type Run<'a> = &'a dyn Fn(Command) -> Result<Vec<u8>, String>;

/// The `-e` arguments that give a container of `image` its session environment; the
/// image is created under the name `container` for the copy and removed again. Every
/// docker command goes through `run`, so a daemon that hangs cannot hold the proof.
pub(super) fn arguments(image: &str, container: &str, run: Run<'_>) -> Result<Vec<String>, String> {
    let folder = tempfile::Builder::new()
        .prefix("horizon-proof-env-")
        .tempdir()
        .map_err(|error| error.to_string())?;
    let mut create = Command::new("docker");
    create.args(["create", "--name", container, image]);
    run(create).map_err(|error| format!("cannot read the image's session environment: {error}"))?;
    let mut copy = Command::new("docker");
    copy.arg("cp").arg(format!("{container}:{FOLDER}")).arg(folder.path());
    let copied = run(copy);
    let mut remove = Command::new("docker");
    remove.args(["rm", "--force", container]);
    let _ = run(remove);
    // An image without the folder has no session environment; any other failure is one.
    if let Err(error) = copied
        && !error.contains("Could not find the file")
        && !error.contains("No such")
    {
        return Err(format!("cannot read the image's session environment: {error}"));
    }
    let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(folder.path().join("session-env.d"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "env"))
        .collect();
    files.sort();
    let texts: Vec<String> = files
        .iter()
        .take(FILES)
        .filter_map(|path| super::super::untrusted::read_regular(path, FILE_LIMIT).ok())
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        .collect();
    Ok(apply(&image_path(image, run)?, &texts))
}

/// The image's own PATH, as `docker image inspect` reports it.
fn image_path(image: &str, run: Run<'_>) -> Result<String, String> {
    let mut inspect = Command::new("docker");
    inspect.args(["image", "inspect", "--format", "{{json .Config.Env}}", image]);
    let output = run(inspect).map_err(|error| format!("cannot inspect the image: {error}"))?;
    let env: Vec<String> =
        serde_json::from_slice(&output).map_err(|error| format!("cannot read the image's environment: {error}"))?;
    Ok(env
        .into_iter()
        .find_map(|entry| entry.strip_prefix("PATH=").map(str::to_owned))
        .unwrap_or_else(|| DEFAULT_PATH.to_owned()))
}

/// The `-e` arguments for session environment files, in order, over `path`: lines of
/// `KEY=VALUE`, `PATH_PREPEND=...` and `PATH_APPEND=...`; blank and `#` lines are skipped.
fn apply(path: &str, texts: &[String]) -> Vec<String> {
    let mut path = path.to_owned();
    let mut arguments = Vec::new();
    for line in texts.iter().flat_map(|text| text.lines()) {
        let line = line.trim_end_matches('\r');
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(prefix) = line.strip_prefix("PATH_PREPEND=") {
            path = format!("{prefix}:{path}");
        } else if let Some(suffix) = line.strip_prefix("PATH_APPEND=") {
            path = format!("{path}:{suffix}");
        } else if let Some((key, _)) = line.split_once('=')
            && key != "PATH"
            && !key.is_empty()
            && !key.starts_with(|c: char| c.is_ascii_digit())
            && key.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        {
            arguments.extend(["-e".to_owned(), line.to_owned()]);
        }
    }
    arguments.extend(["-e".to_owned(), format!("PATH={path}")]);
    arguments
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_files_set_variables_and_extend_the_path_in_order() {
        let texts = [
            "# tools\nDOTNET_ROOT=/opt/dotnet\nPATH_PREPEND=/opt/dotnet\n".to_owned(),
            "PATH_APPEND=/opt/extra\n\nbad key=1\n1X=2\nPATH=/ignored\r\n".to_owned(),
        ];
        assert_eq!(
            apply("/usr/bin", &texts),
            [
                "-e",
                "DOTNET_ROOT=/opt/dotnet",
                "-e",
                "PATH=/opt/dotnet:/usr/bin:/opt/extra"
            ]
        );
    }
}
