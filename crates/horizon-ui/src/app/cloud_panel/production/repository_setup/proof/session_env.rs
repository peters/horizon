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

/// The `-e` arguments that give a container of `image` its session environment; the
/// image is created under the name `container` for the copy and removed again.
pub(super) fn arguments(image: &str, container: &str) -> Result<Vec<String>, String> {
    let folder = tempfile::Builder::new()
        .prefix("horizon-proof-env-")
        .tempdir()
        .map_err(|error| error.to_string())?;
    let created = Command::new("docker")
        .args(["create", "--name", container, image])
        .output()
        .map_err(|error| format!("cannot start docker: {error}"))?;
    if !created.status.success() {
        return Err(format!(
            "cannot read the image's session environment: {}",
            String::from_utf8_lossy(&created.stderr).trim()
        ));
    }
    // An image without the folder has no session environment; that is no failure.
    let _ = Command::new("docker")
        .arg("cp")
        .arg(format!("{container}:{FOLDER}"))
        .arg(folder.path())
        .output();
    let _ = Command::new("docker").args(["rm", "--force", container]).output();
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
    Ok(apply(&image_path(image), &texts))
}

/// The image's own PATH, as `docker image inspect` reports it.
fn image_path(image: &str) -> String {
    Command::new("docker")
        .args(["image", "inspect", "--format", "{{json .Config.Env}}", image])
        .output()
        .ok()
        .and_then(|output| serde_json::from_slice::<Vec<String>>(&output.stdout).ok())
        .and_then(|env| {
            env.into_iter()
                .find_map(|entry| entry.strip_prefix("PATH=").map(str::to_owned))
        })
        .unwrap_or_else(|| DEFAULT_PATH.to_owned())
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
