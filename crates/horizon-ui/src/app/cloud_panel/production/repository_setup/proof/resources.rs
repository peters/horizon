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

/// Removes a finished run's containers, image and volume once `after`, the run's
/// stopped proof, has ended and can create nothing more: in the background, or before
/// returning with `wait`, as when Horizon exits.
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
        let containers = Command::new("docker")
            .args(["ps", "--all", "--quiet", "--filter"])
            .arg(format!("name=^{volume}-"))
            .output()
            .map(|output| {
                String::from_utf8_lossy(&output.stdout)
                    .split_whitespace()
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if !containers.is_empty() {
            let _ = Command::new("docker")
                .args(["rm", "--force"])
                .args(&containers)
                .output();
        }
        let images = Command::new("docker")
            .args(["images", "--format", "{{.Repository}}:{{.Tag}}", "--filter"])
            .arg(format!("reference={volume}-build*"))
            .output()
            .map(|output| {
                String::from_utf8_lossy(&output.stdout)
                    .split_whitespace()
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let _ = Command::new("docker")
            .args(["rmi", "--force", &image])
            .args(&images)
            .output();
        let _ = Command::new("docker")
            .args(["volume", "rm", "--force", &volume])
            .output();
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
    let hash = repository
        .to_string_lossy()
        .bytes()
        .fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
        });
    format!("{}-{:06x}", folder_slug(repository), hash & 0xff_ffff)
}

fn folder_slug(repository: &Path) -> String {
    let name = repository
        .file_name()
        .map(|name| name.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let slug: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let slug = slug.trim_matches('-');
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
