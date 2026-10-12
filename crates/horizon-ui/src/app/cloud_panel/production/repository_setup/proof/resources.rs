//! The names of one setup run's proof image and volume, and their removal.
use std::path::Path;
use std::process::Command;

/// The image and volume of one setup run's proofs: unique to the run, so two runs or
/// two Horizon processes on the same checkout never share or remove each other's.
pub(in crate::app::cloud_panel::production::repository_setup) fn run_names(
    repository: &Path,
    run: &str,
) -> (String, String) {
    let volume = format!("horizon-setup-{}-{run}", slug(repository));
    (format!("{volume}:candidate"), volume)
}

/// A short name for the setup run whose status file is `status`.
pub(in crate::app::cloud_panel::production::repository_setup) fn run_id(status: &Path) -> String {
    let folder = status.parent().unwrap_or(status).to_string_lossy();
    let hash = folder.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    });
    format!("{:08x}", hash & 0xffff_ffff)
}

/// The start of the worker-check operation names of the run whose volume is `volume`;
/// each check adds its profile's number.
pub(super) fn check_operations(volume: &str) -> String {
    let hash = volume.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    });
    format!("horizon-setup-check-{:08x}", hash & 0xffff_ffff)
}

/// The most one cleanup command may take before Horizon gives up on it and goes on with
/// the next, so a daemon that does not answer never holds an exit or a new setup.
const CLEANUP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// Runs a docker cleanup command; its standard output when it succeeded in time. One
/// that runs past [`CLEANUP_TIMEOUT`] is ended with everything it started.
fn docker(args: &[String]) -> Option<String> {
    use std::io::Read as _;
    let mut command = Command::new("docker");
    command
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut command, 0);
    let mut child = command.spawn().ok()?;
    let stdout = child.stdout.take();
    let reader = std::thread::spawn(move || {
        let mut output = String::new();
        if let Some(stdout) = stdout {
            let _ = stdout.take(1024 * 1024).read_to_string(&mut output);
        }
        output
    });
    let deadline = std::time::Instant::now() + CLEANUP_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return reader.join().ok().filter(|_| status.success()),
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            _ => {
                #[cfg(unix)]
                if let Some(group) = i32::try_from(child.id()).ok().and_then(rustix::process::Pid::from_raw) {
                    let _ = rustix::process::kill_process_group(group, rustix::process::Signal::KILL);
                }
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

/// `words` as the arguments of a docker command.
fn arguments<'a>(words: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    words.into_iter().map(str::to_owned).collect()
}

/// The containers whose names match `pattern`, a Docker name filter.
fn containers(pattern: &str) -> Vec<String> {
    docker(&arguments([
        "ps",
        "--all",
        "--quiet",
        "--filter",
        &format!("name={pattern}"),
    ]))
    .map(|output| output.split_whitespace().map(str::to_owned).collect())
    .unwrap_or_default()
}

/// Removes a finished run's containers, image and volume once `after`, the run's
/// stopped proof, has ended and can create nothing more: in the background, or before
/// returning with `wait`, as when Horizon exits. Each docker command has a time limit.
pub(in crate::app::cloud_panel::production::repository_setup) fn remove_run_resources(
    repository: &Path,
    run: &str,
    after: Option<std::thread::JoinHandle<()>>,
    wait: bool,
) {
    let (image, volume) = run_names(repository, run);
    let remove = move || {
        if let Some(after) = after {
            let _ = after.join();
        }
        // The proof's own containers and the worker-check containers of its profiles.
        // A check stopped mid-way can still have its container appear a moment later,
        // so the search runs once more after a pause.
        let checks = format!("^/horizon-contract-{}-", check_operations(&volume));
        for pass in 0..2 {
            if pass > 0 {
                std::thread::sleep(std::time::Duration::from_secs(2));
            }
            let found: Vec<String> = containers(&format!("^/{volume}-"))
                .into_iter()
                .chain(containers(&checks))
                .collect();
            if !found.is_empty() {
                let mut remove = arguments(["rm", "--force", "--volumes"]);
                remove.extend(found);
                let _ = docker(&remove);
            }
        }
        let filter = format!("reference={volume}-build*");
        let images: Vec<String> = docker(&arguments([
            "images",
            "--format",
            "{{.Repository}}:{{.Tag}}",
            "--filter",
            &filter,
        ]))
        .map(|output| output.split_whitespace().map(str::to_owned).collect())
        .unwrap_or_default();
        let mut remove_images = arguments(["rmi", "--force", &image]);
        remove_images.extend(images);
        let _ = docker(&remove_images);
        let _ = docker(&arguments(["volume", "rm", "--force", &volume]));
    };
    if wait {
        remove();
    } else {
        std::thread::spawn(remove);
    }
}

/// A name Docker accepts for the repository: its folder name and a short hash of its
/// path, so two checkouts with the same folder name never share an image or a volume.
fn slug(repository: &Path) -> String {
    // The path's own bytes, so two paths that only differ in bytes that are not UTF-8
    // never share a name.
    #[cfg(unix)]
    let bytes = std::os::unix::ffi::OsStrExt::as_bytes(repository.as_os_str()).to_vec();
    #[cfg(not(unix))]
    let bytes = repository.to_string_lossy().into_owned().into_bytes();
    let hash = bytes.iter().fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0100_0000_01b3)
    });
    format!("{}-{:06x}", folder_slug(repository), hash & 0xff_ffff)
}

/// The longest part of a name the folder gives; the path's hash tells folders apart,
/// and Docker names stay well within their limits.
const FOLDER_SLUG_LIMIT: usize = 32;

fn folder_slug(repository: &Path) -> String {
    let name = repository
        .file_name()
        .map(|name| name.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let slug: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let slug: String = slug.trim_matches('-').chars().take(FOLDER_SLUG_LIMIT).collect();
    let slug = slug.trim_end_matches('-');
    if slug.is_empty() {
        "repository".to_owned()
    } else {
        slug.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_image_and_volume_take_a_name_docker_accepts() {
        assert_eq!(folder_slug(Path::new("/home/me/YouPay v2")), "youpay-v2");
        assert_eq!(folder_slug(Path::new("/")), "repository");
        assert_eq!(
            folder_slug(Path::new(&format!("/home/me/{}", "a".repeat(255)))).len(),
            FOLDER_SLUG_LIMIT
        );
        let (a, b) = (slug(Path::new("/home/a/app")), slug(Path::new("/home/b/app")));
        assert!(a.starts_with("app-") && b.starts_with("app-"));
        assert_ne!(a, b, "two checkouts named app keep their own image and volume");
        assert_eq!(a, slug(Path::new("/home/a/app")), "the same path keeps its volume");
    }

    #[test]
    fn each_setup_run_names_its_own_image_and_volume() {
        let repository = Path::new("/home/me/app");
        let (first, second) = (
            run_id(Path::new("/tmp/horizon-cloud-setup-1-10/status.json")),
            run_id(Path::new("/tmp/horizon-cloud-setup-1-11/status.json")),
        );
        assert_ne!(first, second);
        let (image, volume) = run_names(repository, &first);
        assert_eq!(image, format!("{volume}:candidate"));
        assert_ne!(
            volume,
            run_names(repository, &second).1,
            "two runs never share a volume"
        );
    }
}
