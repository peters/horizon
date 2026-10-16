//! The localci programs of a proof. The CI runs the localci a worker session of the image
//! finds first, as the agents on the worker do, so every image the setup builds must have
//! one, with mode 755. Horizon's own localci checks the plan before the CI: the plan must
//! keep a step that runs in every job. Horizon fetches its pinned release, checks it
//! against the checksum this Horizon was built with and copies it into the proof's
//! container, owned by root so the CI's user cannot change it; the release is a static binary, so nothing the image preloads runs
//! inside it. Horizon's localci runs the CI only for a published image without a localci
//! of its own.
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

/// Where the trusted localci is in the proof's container, copied in before it starts.
pub(super) const IN_CONTAINER: &str = "/opt/horizon-proof/localci";

/// Where the setup agent is told to install localci for the agents on the worker.
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
        "The localci for step 4 is {VERSION}: {releases}. Install the localci binary from that archive on the PATH of a worker session, for example at {IN_IMAGE}, with mode 755. Horizon runs the CI with the localci a worker session finds first, as the agents on the worker do; a wrapper there that starts what the CI needs and then runs localci works too."
    )
}

impl super::Job {
    /// The localci a worker session of `image` runs: the first `localci` on the session's
    /// PATH, read with `docker cp` from a created container, following a link as the
    /// session would, never by running the image; none when the session finds none. A
    /// wrapper that starts what the CI needs and then runs localci is one too. It must
    /// have mode 755, so no session user can change it and it runs with no more rights
    /// than the user who starts it. `number` names the containers this look creates.
    pub(super) fn session_localci(&self, image: &str, number: usize) -> Result<Option<String>, String> {
        let run = |command| self.capture(command, super::STEP_TIMEOUT);
        let path = super::session_env::session_path_of(image, &format!("{}-env{number}", self.container), &run)?;
        let container = format!("{}-localci{number}", self.container);
        let mut create = Command::new("docker");
        create.args(["create", "--name", &container, image]);
        run(create).map_err(|error| format!("cannot look at the image's localci: {error}"))?;
        let mut found = Ok(None);
        for folder in path.split(':').filter(|folder| folder.starts_with('/')) {
            let place = format!("{}/localci", folder.trim_end_matches('/'));
            match super::session_env::copied_following(&container, &place, &run) {
                Ok(Some(archive)) => {
                    found = Ok(Some((place, archive)));
                    break;
                }
                Ok(None) => {}
                Err(error) => {
                    found = Err(error);
                    break;
                }
            }
        }
        let mut remove = Command::new("docker");
        remove.args(["rm", "--force", "--volumes", &container]);
        let _ = run(remove);
        let Some((place, archive)) = found.map_err(|error| format!("cannot look at the image's localci: {error}"))?
        else {
            return Ok(None);
        };
        let mode = super::session_env::unpack_within(&archive, ARCHIVE_LIMIT, 1)
            .map_err(|error| format!("cannot read the image's localci at {place}: {error}"))?
            .into_iter()
            .find_map(|(_, entry)| match entry {
                super::session_env::Unpacked::File(_, mode) => Some(mode),
                _ => None,
            })
            .ok_or_else(|| format!("the localci a worker session finds, {place}, is not a file"))?;
        if mode & 0o7777 != 0o755 {
            return Err(format!(
                "the localci a worker session finds, {place}, has mode {:o}; install it with mode 755",
                mode & 0o7777
            ));
        }
        Ok(Some(place))
    }

    /// Horizon's own localci for the image's platform, fetched and checked once.
    pub(super) fn trusted_localci(&self) -> Result<PathBuf, String> {
        let platform = self
            .setup
            .as_ref()
            .ok()
            .and_then(|setup| setup.config().ok())
            .and_then(|config| {
                let build = config.profiles.get(&config.default)?.build.as_ref()?;
                Some(build.platform.clone())
            })
            .unwrap_or_else(|| "linux/amd64".to_owned());
        self.trusted_localci_for(&platform)
    }

    /// Horizon's own localci for images of `platform`, fetched and checked once.
    pub(super) fn trusted_localci_for(&self, platform: &str) -> Result<PathBuf, String> {
        let mut known = self.localci.borrow_mut();
        let index = known.iter().position(|(name, _)| name == platform).unwrap_or_else(|| {
            let run = |command, limit| self.capture(command, limit);
            known.push((platform.to_owned(), trusted(platform, &run)));
            known.len() - 1
        });
        known[index]
            .1
            .as_ref()
            .map(|(_, binary)| binary.clone())
            .map_err(Clone::clone)
    }
}

/// A command run within the proof's cancellation and the time limit given, returning
/// what it wrote to standard output.
pub(super) type Bounded<'a> = &'a dyn Fn(Command, std::time::Duration) -> Result<Vec<u8>, String>;

/// How long the download of the release may take.
const DOWNLOAD_TIMEOUT: std::time::Duration = std::time::Duration::from_mins(5);

/// How long unpacking the release may take.
const UNPACK_TIMEOUT: std::time::Duration = std::time::Duration::from_mins(1);

/// The trusted localci for an image of `platform`, extracted from the verified release
/// into a new private folder, which goes when the returned value is dropped. The
/// download and the unpacking run through `run`, so Stop ends them at once.
pub(super) fn trusted(platform: &str, run: Bounded<'_>) -> Result<(tempfile::TempDir, PathBuf), String> {
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
        download(asset, &archive, run)?;
        read_verified(&archive, sum)
            .ok_or_else(|| format!("the downloaded localci {VERSION} does not match its checksum"))?
    };
    let folder = tempfile::Builder::new()
        .prefix("horizon-proof-localci-")
        .tempdir()
        .map_err(|error| error.to_string())?;
    let copy = folder.path().join("release.tar.gz");
    std::fs::write(&copy, bytes).map_err(|error| error.to_string())?;
    let mut unpack = Command::new("tar");
    unpack
        .arg("-xzf")
        .arg(&copy)
        .arg("-C")
        .arg(folder.path())
        .arg("localci");
    let unpacked = run(unpack, UNPACK_TIMEOUT);
    let _ = std::fs::remove_file(&copy);
    let binary = folder.path().join("localci");
    if let Err(error) = unpacked {
        return Err(format!("cannot unpack localci {VERSION}: {error}"));
    }
    if !binary.is_file() {
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

/// Downloads the release `asset` to `target`, through a new file of its own beside it
/// that takes the target's name in one step: proofs downloading at once never write into
/// one file, and the last one in place wins with the same checked release.
fn download(asset: &str, target: &Path, run: Bounded<'_>) -> Result<(), String> {
    let url = format!("https://github.com/peters/localci/releases/download/{VERSION}/{asset}");
    let folder = target.parent().ok_or("localci has no cache folder")?;
    let partial = tempfile::Builder::new()
        .prefix(".localci-download-")
        .tempfile_in(folder)
        .map_err(|error| format!("cannot download localci: {error}"))?;
    let mut curl = Command::new("curl");
    curl.args(["-fsSL", "--proto", "=https", "--max-time", "300", "--max-filesize"])
        .arg(ARCHIVE_LIMIT.to_string())
        .arg("-o")
        .arg(partial.path())
        .arg(&url);
    run(curl, DOWNLOAD_TIMEOUT).map_err(|error| format!("cannot download localci {VERSION} from {url}: {error}"))?;
    partial
        .persist(target)
        .map(|_| ())
        .map_err(|error| format!("cannot keep localci: {error}"))
}

/// Where localci keeps its step records in the CI container, from the session's
/// `XDG_CACHE_HOME` or else HOME's `.cache`, when that folder outlives the container: on
/// the volume, and not in the checkout, which every proof makes again. The session's
/// `-e` arguments come from the image, so the path is taken apart here, never by a shell
/// that runs with them.
pub(super) fn records(session: &[String]) -> Option<String> {
    let cache = session
        .chunks(2)
        .filter_map(|pair| match pair {
            [flag, value] if flag == "-e" => value.strip_prefix("XDG_CACHE_HOME="),
            _ => None,
        })
        .next_back()
        .unwrap_or("/workspace/home/.cache");
    let mut parts: Vec<&str> = Vec::new();
    if !cache.starts_with('/') {
        return None;
    }
    for part in cache.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            part => parts.push(part),
        }
    }
    let kept = matches!(parts.as_slice(), ["workspace", rest, ..] if *rest != "checkout");
    kept.then(|| format!("/{}/localci", parts.join("/")))
}

/// Checks localci's JSON plan of the proof: every selected job keeps at least one step
/// that runs a command, so leaving steps out never makes a job pass without CI.
/// Returns the keys of the selected legs that run here, which the run's report must
/// account for, and the legs left out, each named with why it proves nothing here.
pub(super) fn check_plan(plan: &[u8]) -> Result<(Vec<String>, Vec<String>), String> {
    let plan: serde_json::Value =
        serde_json::from_slice(plan).map_err(|error| format!("localci's plan does not read: {error}"))?;
    let legs: Vec<&serde_json::Value> = plan["legs"]
        .as_array()
        .map(|legs| legs.iter().filter(|leg| leg["selected"] == true).collect())
        .unwrap_or_default();
    if legs.is_empty() {
        return Err("the proof selects no job that runs on Linux".to_owned());
    }
    let (mut keys, mut left_out) = (Vec::new(), Vec::new());
    for leg in legs {
        let job = leg["key"].as_str().or_else(|| leg["job"].as_str()).unwrap_or("a job");
        // localci skips a whole job whose `if:` is false here and calls that a pass, and
        // a job whose every command step is skipped passes without CI: such a job proves
        // nothing, so it is left out of what the report must show and named instead.
        let runs = leg["steps"].as_array().is_some_and(|steps| {
            steps
                .iter()
                .any(|step| step["kind"] != "uses" && step["leave_out"].is_null())
        });
        if leg["if_locally"] == false {
            left_out.push(format!("{job} (its if: is false here)"));
        } else if !runs {
            left_out.push(format!("{job} (every step that runs a command is skipped)"));
        } else {
            keys.push(job.to_owned());
        }
    }
    if keys.is_empty() {
        return Err(format!(
            "no job the proof selects runs a command here, so it would prove nothing: {}",
            left_out.join(", ")
        ));
    }
    Ok((keys, left_out))
}

/// localci's JSON report in what the session's localci wrote on standard output: the
/// last document there that starts a line and reads to the end, since a wrapper may
/// print first, as a database it starts does.
fn report_of(output: &[u8]) -> Result<serde_json::Value, String> {
    let starts: Vec<usize> = std::iter::once(0)
        .chain(
            output
                .windows(2)
                .enumerate()
                .filter(|(_, pair)| *pair == b"\n{")
                .map(|(index, _)| index + 1),
        )
        .collect();
    starts
        .into_iter()
        .rev()
        .find_map(|start| {
            serde_json::from_slice::<serde_json::Value>(&output[start..])
                .ok()
                .filter(|report| report.get("legs").is_some())
        })
        .ok_or_else(|| "localci's report does not read: its output ends with no JSON report".to_owned())
}

/// Checks localci's run report against the legs the plan selected: each one passed and
/// ran at least one step that runs a command. A condition the plan cannot decide, such
/// as one on `needs` or `steps`, can still skip a job or its steps while localci calls the
/// run a pass, so the plan alone proves nothing. A step localci reused, because it passed
/// before on the same inputs in this image, counts as run.
pub(super) fn check_report(report: &[u8], planned: &[String]) -> Result<(), String> {
    let report = report_of(report)?;
    let legs = report["legs"].as_array().map(Vec::as_slice).unwrap_or_default();
    for key in planned {
        let leg = legs
            .iter()
            .find(|leg| leg["key"].as_str() == Some(key))
            .ok_or_else(|| format!("{key} did not run: localci reports no result for it"))?;
        let result = leg["result"].as_str().unwrap_or("no result");
        if result != "success" {
            return Err(format!("{key} did not pass: localci reports {result}"));
        }
        let ran = leg["steps"]
            .as_array()
            .is_some_and(|steps| steps.iter().any(|step| step["outcome"] == "success"));
        if !ran {
            return Err(format!("{key} ran no step that runs a command, so it proves nothing"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_records_to_forget_are_on_the_volume_and_never_in_the_checkout() {
        let session = |cache: &str| vec!["-e".to_owned(), format!("XDG_CACHE_HOME={cache}")];
        assert_eq!(records(&[]).as_deref(), Some("/workspace/home/.cache/localci"));
        assert_eq!(
            records(&session("/workspace/cache")).as_deref(),
            Some("/workspace/cache/localci")
        );
        assert_eq!(
            records(&session("/workspace/checkout")),
            None,
            "the checkout is the tested tree"
        );
        assert_eq!(records(&session("/workspace/home/../checkout/x")), None);
        assert_eq!(records(&session("/tmp/cache")), None, "gone with the container");
        assert_eq!(records(&session("relative")), None);
        assert_eq!(records(&session("/workspace")), None, "never the whole volume");
    }

    #[test]
    fn the_report_is_read_after_what_a_wrapper_printed() {
        let output = b"about to fork child process, waiting until server is ready for connections.\nforked process: 42\n{\n  \"legs\": [{\"key\": \"build\", \"result\": \"success\", \"steps\": [{\"outcome\": \"success\"}]}]\n}\n";
        assert!(check_report(output, &["build".to_owned()]).is_ok());
        assert!(check_report(b"no report at all\n", &["build".to_owned()]).is_err());
    }

    #[test]
    fn every_planned_job_must_have_passed_a_step_it_ran() {
        let report = |legs: serde_json::Value| serde_json::to_vec(&serde_json::json!({"legs": legs})).unwrap();
        let planned = ["test".to_owned()];
        let checkout = serde_json::json!({"outcome": "skipped", "reason": "uses: actions do not run locally"});
        let ran = serde_json::json!({"outcome": "success"});
        let reused = serde_json::json!({"outcome": "success", "reason": "cached: passed before on the same inputs"});
        let leg = |result: &str, steps: serde_json::Value| serde_json::json!([{"key": "test", "result": result, "steps": steps}]);
        assert!(check_report(&report(leg("success", serde_json::json!([checkout, ran]))), &planned).is_ok());
        assert!(check_report(&report(leg("success", serde_json::json!([checkout, reused]))), &planned).is_ok());
        assert!(
            check_report(&report(leg("success", serde_json::json!([checkout]))), &planned)
                .unwrap_err()
                .contains("ran no step")
        );
        assert!(
            check_report(&report(leg("skipped", serde_json::json!([]))), &planned)
                .unwrap_err()
                .contains("did not pass")
        );
        assert!(
            check_report(&report(serde_json::json!([])), &planned)
                .unwrap_err()
                .contains("did not run")
        );
    }

    #[test]
    fn a_job_without_a_running_step_is_left_out_and_a_plan_needs_one_that_runs() {
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
                .contains("test (every step that runs a command is skipped)"),
            "a plan whose only job runs nothing proves nothing"
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
        assert!(
            check_plan(&skipped_job)
                .unwrap_err()
                .contains("no job the proof selects runs a command here")
        );
        let with_publish_and_sync = serde_json::to_vec(&serde_json::json!({"legs": [
            {"key": "test", "selected": true, "steps": [{"kind": "run", "leave_out": null}]},
            {"key": "publish#1", "selected": true, "if_locally": false, "steps": [{"kind": "run", "leave_out": null}]},
            {"key": "vendor-sync", "selected": true, "steps": [checkout, tests(serde_json::json!("--skip"))]}
        ]}))
        .unwrap();
        assert_eq!(
            check_plan(&with_publish_and_sync).unwrap(),
            (
                vec!["test".to_owned()],
                vec![
                    "publish#1 (its if: is false here)".to_owned(),
                    "vendor-sync (every step that runs a command is skipped)".to_owned()
                ]
            ),
            "a job that runs nothing here is left out and named, not proved"
        );
    }
}
