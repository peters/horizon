//! The localci that runs a proof's CI. The candidate image is the agent's work, so a
//! localci inside it proves nothing: Horizon fetches its own pinned release, checks it
//! against the checksum this Horizon was built with and mounts it read-only into the
//! proof's container. The release is a static binary, so nothing the image preloads
//! runs inside it. Before the CI, its plan must keep a step that runs in every job.
use sha2::{Digest as _, Sha256};
use std::path::{Path, PathBuf};
use std::process::Command;

/// The localci release the proof runs.
const VERSION: &str = "v0.1.0";

/// The release asset for each platform a proof's image can have, and its SHA-256.
const RELEASES: [(&str, &str, &str); 2] = [
    (
        "linux/amd64",
        "localci-x86_64-unknown-linux-musl.tar.gz",
        "215096651a2af9b1a8d64825d6453b7e090049078ac30525074e9e1f43fdc893",
    ),
    (
        "linux/arm64",
        "localci-aarch64-unknown-linux-musl.tar.gz",
        "c1706804fe8359db98bb8ce594cd20cb5020e217bdc5b0126ec636c4de650365",
    ),
];

/// The largest release archive Horizon reads.
const ARCHIVE_LIMIT: u64 = 64 * 1024 * 1024;

/// Where the trusted localci is in the proof's container.
pub(super) const MOUNTED: &str = "/opt/horizon-proof/localci";

/// Where the image keeps localci for the agents on the worker.
const IN_IMAGE: &str = "/usr/local/bin/localci";

/// The setup agent's instruction for installing the localci the proof checks for.
pub(super) fn instruction() -> String {
    let releases = RELEASES
        .iter()
        .map(|(platform, asset, sum)| {
            format!(
                "https://github.com/peters/localci/releases/download/{VERSION}/{asset} (SHA-256 {sum}) for {platform}"
            )
        })
        .collect::<Vec<_>>()
        .join(", or ");
    format!(
        "The localci for step 4 is {VERSION}: {releases}. Install the localci binary from that archive at {IN_IMAGE} with mode 755; Horizon checks that the image holds exactly this binary, executable for every user."
    )
}

/// Checks that `image` keeps exactly the trusted localci at its place for the worker's
/// agents, read with `docker cp` from a created container, never by running the image.
pub(super) fn check_installed(
    image: &str,
    container: &str,
    trusted: &Path,
    run: super::session_env::Run<'_>,
) -> Result<(), String> {
    let mut create = Command::new("docker");
    create.args(["create", "--name", container, image]);
    run(create).map_err(|error| format!("cannot look at the image's localci: {error}"))?;
    let copied = super::session_env::copied(container, IN_IMAGE, run);
    let mut remove = Command::new("docker");
    remove.args(["rm", "--force", container]);
    let _ = run(remove);
    let missing = || format!("the image has no localci {VERSION} at {IN_IMAGE}, which the agents on the worker use");
    let archive = copied
        .map_err(|error| format!("cannot look at the image's localci: {error}"))?
        .ok_or_else(missing)?;
    let (installed, mode) = super::session_env::unpack_within(&archive, ARCHIVE_LIMIT, 1)
        .map_err(|error| format!("cannot read the image's localci: {error}"))?
        .into_iter()
        .find_map(|(_, entry)| match entry {
            super::session_env::Unpacked::File(bytes, mode) => Some((bytes, mode)),
            _ => None,
        })
        .ok_or_else(missing)?;
    let trusted = std::fs::read(trusted).map_err(|error| error.to_string())?;
    if Sha256::digest(&installed) != Sha256::digest(&trusted) {
        return Err(format!(
            "the localci at {IN_IMAGE} in the image is not localci {VERSION}; install the release the setup names"
        ));
    }
    // The worker's agents run it as any user the image's sessions have; none of them
    // may change it, and it runs with no more rights than the user who starts it.
    if mode & 0o7777 != 0o755 {
        return Err(format!(
            "the localci at {IN_IMAGE} in the image has mode {:o}; install it with mode 755",
            mode & 0o7777
        ));
    }
    Ok(())
}

/// The trusted localci for an image of `platform`, extracted from the verified release
/// into a new private folder, which goes when the returned value is dropped.
pub(super) fn trusted(platform: &str) -> Result<(tempfile::TempDir, PathBuf), String> {
    let (_, asset, sum) = RELEASES
        .iter()
        .find(|(name, ..)| *name == platform)
        .ok_or_else(|| format!("Horizon has no trusted localci for {platform}"))?;
    let cache = horizon_core::HorizonHome::resolve().root().join("cloud").join("tools");
    std::fs::create_dir_all(&cache).map_err(|error| format!("cannot keep localci: {error}"))?;
    let archive = cache.join(format!("localci-{VERSION}-{asset}"));
    // The archive is read once and checked as read, so no file swapped in after the
    // check is what runs.
    let bytes = if let Some(bytes) = read_verified(&archive, sum) {
        bytes
    } else {
        download(asset, &archive)?;
        read_verified(&archive, sum)
            .ok_or_else(|| format!("the downloaded localci {VERSION} does not match its checksum"))?
    };
    let folder = tempfile::Builder::new()
        .prefix("horizon-proof-localci-")
        .tempdir()
        .map_err(|error| error.to_string())?;
    let copy = folder.path().join("release.tar.gz");
    std::fs::write(&copy, bytes).map_err(|error| error.to_string())?;
    let status = Command::new("tar")
        .arg("-xzf")
        .arg(&copy)
        .arg("-C")
        .arg(folder.path())
        .arg("localci")
        .status()
        .map_err(|error| format!("cannot unpack localci: {error}"))?;
    let _ = std::fs::remove_file(&copy);
    let binary = folder.path().join("localci");
    if !status.success() || !binary.is_file() {
        return Err(format!("cannot unpack localci {VERSION}"));
    }
    // The proof's container runs it as the image's user, whatever mode tar gave it.
    #[cfg(unix)]
    std::fs::set_permissions(&binary, std::os::unix::fs::PermissionsExt::from_mode(0o755))
        .map_err(|error| format!("cannot make localci executable: {error}"))?;
    Ok((folder, binary))
}

/// The archive's bytes when they have the expected SHA-256.
fn read_verified(archive: &Path, sum: &str) -> Option<Vec<u8>> {
    let bytes = super::super::untrusted::read_regular(archive, ARCHIVE_LIMIT).ok()?;
    let digest = Sha256::digest(&bytes).iter().fold(String::new(), |mut digest, byte| {
        use std::fmt::Write as _;
        let _ = write!(digest, "{byte:02x}");
        digest
    });
    (digest == sum).then_some(bytes)
}

/// Downloads the release `asset` to `target`, through a partial file beside it.
fn download(asset: &str, target: &Path) -> Result<(), String> {
    let url = format!("https://github.com/peters/localci/releases/download/{VERSION}/{asset}");
    let partial = target.with_extension("partial");
    let status = Command::new("curl")
        .args(["-fsSL", "--proto", "=https", "--max-time", "300", "--max-filesize"])
        .arg(ARCHIVE_LIMIT.to_string())
        .arg("-o")
        .arg(&partial)
        .arg(&url)
        .status()
        .map_err(|error| format!("cannot download localci: {error}"))?;
    if !status.success() {
        let _ = std::fs::remove_file(&partial);
        return Err(format!("cannot download localci {VERSION} from {url}"));
    }
    std::fs::rename(&partial, target).map_err(|error| error.to_string())
}

/// Checks localci's JSON plan of the proof: every selected job keeps at least one step
/// that runs a command, so leaving steps out never makes a job pass without CI.
pub(super) fn check_plan(plan: &[u8]) -> Result<(), String> {
    let plan: serde_json::Value =
        serde_json::from_slice(plan).map_err(|error| format!("localci's plan does not read: {error}"))?;
    let legs: Vec<&serde_json::Value> = plan["legs"]
        .as_array()
        .map(|legs| legs.iter().filter(|leg| leg["selected"] == true).collect())
        .unwrap_or_default();
    if legs.is_empty() {
        return Err("the proof selects no job that runs on Linux".to_owned());
    }
    for leg in legs {
        let job = leg["key"].as_str().or_else(|| leg["job"].as_str()).unwrap_or("a job");
        // localci skips a whole job whose `if:` is false here, and calls that a pass.
        if leg["if_locally"] == false {
            return Err(format!(
                "{job} does not run here: its if: is false, so it would prove nothing"
            ));
        }
        let runs = leg["steps"].as_array().is_some_and(|steps| {
            steps
                .iter()
                .any(|step| step["kind"] != "uses" && step["leave_out"].is_null())
        });
        if !runs {
            return Err(format!(
                "the proof leaves out every step of {job} that runs a command, so it would prove nothing"
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plan_must_keep_a_running_step_in_every_selected_job() {
        let plan = |steps: serde_json::Value| {
            serde_json::to_vec(&serde_json::json!({"legs": [
                {"key": "test", "selected": true, "steps": steps},
                {"key": "windows", "selected": false, "steps": []}
            ]}))
            .unwrap()
        };
        let checkout = serde_json::json!({"kind": "uses", "leave_out": "uses: actions do not run locally"});
        let tests = |leave_out: serde_json::Value| serde_json::json!({"kind": "run", "leave_out": leave_out});
        assert!(check_plan(&plan(serde_json::json!([checkout, tests(serde_json::Value::Null)]))).is_ok());
        assert!(
            check_plan(&plan(serde_json::json!([checkout, tests(serde_json::json!("--skip"))])))
                .unwrap_err()
                .contains("leaves out every step of test")
        );
        let restore_and_test = serde_json::json!([{"kind": "setup", "leave_out": null}]);
        assert!(
            check_plan(&plan(restore_and_test)).is_ok(),
            "a step localci calls setup still runs commands"
        );
        assert!(check_plan(br#"{"legs": []}"#).is_err());
        let skipped_job = serde_json::to_vec(&serde_json::json!({"legs": [
            {"key": "test", "selected": true, "if_locally": false, "steps": [{"kind": "run", "leave_out": null}]}
        ]}))
        .unwrap();
        assert!(check_plan(&skipped_job).unwrap_err().contains("does not run here"));
    }
}
