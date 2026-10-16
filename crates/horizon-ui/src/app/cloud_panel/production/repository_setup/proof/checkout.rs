//! The proof's checkout: the tested commit in the run's volume, as `actions/checkout`
//! makes it. The first proof of a run sends the commit and its history; a later one
//! sends only what the run's git cache does not hold yet, and nothing when it holds it
//! all, so a Docker host or a rented builder gets new commits only.
//!
//! The cache is a volume of its own that only Horizon's base image mounts, never the
//! candidate image, its CI or a preview, so nothing the repository runs writes to it.
//! Git checks each object it receives against its name, and a cache that cannot serve
//! the commit is removed and the whole history sent instead.
use super::{STEP_TIMEOUT, TOOLS, launch, resources};
use std::time::Duration;

/// How long the read of the objects the cache keeps may take.
const KEPT_TIMEOUT: Duration = Duration::from_mins(2);

impl super::Job {
    /// Clones the committed repository into the proof volume, or brings the clone
    /// up to date, so build output and caches stay between runs.
    pub(super) fn prepare(&self, log: &mut Option<std::fs::File>) -> Result<(), String> {
        // The image the agent wrote gets the tested commit and its history only, as a
        // bundle made here, never this computer's git folder with its other branches.
        let head = self.head().ok_or("the repository has no commit")?;
        // Horizon's pinned base image, not the candidate, prepares the volume: what the
        // CI later runs on is the bundled commit, whatever the candidate's tools do.
        let mut own = self.docker();
        own.args(["run", "--rm", "--name", &self.container, "-v"])
            .arg(format!("{}:/workspace", self.volume))
            .args(["--entrypoint", "sh", TOOLS, "-c"]);
        let take_back = self.root_dirty.load(std::sync::atomic::Ordering::SeqCst);
        own.arg(if take_back {
            format!("{OWN}; {TAKE_BACK}")
        } else {
            OWN.to_owned()
        });
        self.stream(own, log)?;
        if take_back {
            self.root_dirty.store(false, std::sync::atomic::Ordering::SeqCst);
        }
        let kept = self.kept();
        let result = match self.check_out(head, &kept, log) {
            Err(error) if !kept.is_empty() && error != "stopped" => {
                self.record(
                    log,
                    format!("The run's git cache cannot serve this commit ({error}); sending the whole history."),
                );
                self.forget_cache();
                self.check_out(head, &[], log)
            }
            result => result,
        };
        result?;
        self.hydrate(head, log)
    }

    /// Fetches `head` into the volume's checkout, sending what the objects in `kept`
    /// do not reach.
    fn check_out(&self, head: &str, kept: &[String], log: &mut Option<std::fs::File>) -> Result<(), String> {
        // In a new private folder with a random name, never beside the agent's files.
        let folder = tempfile::Builder::new()
            .prefix("horizon-setup-bundle-")
            .tempdir()
            .map_err(|error| format!("cannot make a private folder for the bundle: {error}"))?;
        let bundled = self.bundle(folder.path(), head, kept, log)?;
        self.record(log, sent(bundled.bundle.as_deref(), kept.is_empty()));
        // As root, so the checkout can be handed to the worker's account after. The
        // bundle goes into the container as a copy, so any daemon can run it.
        let mut clone = self.docker();
        clone
            .args(["run", "--rm", "--name", &self.container, "-v"])
            .arg(format!("{}:/workspace", self.volume))
            .arg("-v")
            .arg(format!("{}:/cache", resources::git_cache(&self.volume)))
            .arg("-e")
            .arg(format!("HEAD_SHA={head}"))
            .arg("-e")
            .arg(format!("HEAD_BRANCH={}", bundled.branch.unwrap_or_default()))
            .args(["--entrypoint", "sh", TOOLS, "-c"])
            .arg(CLONE);
        let mut clone = launch::Launch::new(clone);
        clone.copies.push(launch::Copy::new(&bundled.names, "/names"));
        if let Some(bundle) = &bundled.bundle {
            clone.copies.push(launch::Copy::new(bundle, "/src.bundle"));
        }
        let result = self.stream_launch(clone, log, Some(STEP_TIMEOUT), false);
        drop(folder);
        result.map(|_| ())
    }

    /// The objects the run's git cache keeps; none when it has none yet or the read fails,
    /// and the proof then sends the whole history.
    fn kept(&self) -> Vec<String> {
        // A name of its own, so it never waits on or collides with the proof's container;
        // the run's cleanup removes it by the run's prefix all the same.
        let mut list = self.docker();
        let name = format!("{}-kept", self.container);
        list.args(["run", "--rm", "--name", &name, "-v"])
            .arg(format!("{}:/cache:ro", resources::git_cache(&self.volume)))
            .args(["--entrypoint", "sh", TOOLS, "-c", KEPT]);
        if let Ok(output) = self.capture(list, KEPT_TIMEOUT) {
            objects(&String::from_utf8_lossy(&output))
        } else {
            // A read ended by its limit or by Stop leaves its container running, as only
            // the client ends; it goes before the checkout uses the cache.
            let _ = resources::remove_now(&self.daemon, &["rm", "--force", &name]);
            Vec::new()
        }
    }

    /// Removes the run's git cache, so the next checkout starts it again.
    fn forget_cache(&self) {
        let mut remove = self.docker();
        remove
            .args(["volume", "rm", "--force"])
            .arg(resources::git_cache(&self.volume));
        let _ = self.capture(remove, KEPT_TIMEOUT);
    }
}

/// The log line that says what a checkout sends.
fn sent(bundle: Option<&std::path::Path>, whole: bool) -> String {
    let Some(bundle) = bundle else {
        return "The run's git cache holds this commit; nothing is sent for the checkout.".to_owned();
    };
    let size = std::fs::metadata(bundle).map_or(0, |metadata| metadata.len());
    let megabytes = size.div_ceil(1024 * 1024);
    if whole {
        format!("Sending the commit and its history for the checkout ({megabytes} MB).")
    } else {
        format!("Sending the commits the run's git cache does not hold ({megabytes} MB).")
    }
}

/// The object names in the cache's listing; anything else is left out.
fn objects(listing: &str) -> Vec<String> {
    listing
        .lines()
        .map(str::trim)
        .filter(|line| matches!(line.len(), 40 | 64) && line.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .map(str::to_owned)
        .collect()
}

/// Lists the objects the cache keeps, when it has a repository yet.
const KEPT: &str = "if [ -f /cache/HEAD ]; then GIT_CONFIG_GLOBAL=/dev/null git -c safe.directory='*' -C /cache \
for-each-ref --format='%(objectname)' refs/kept; fi";

/// Takes the bundle, when there is one, into the run's git cache, which keeps each
/// commit and tag the checkout names, then makes the checkout from the cache: it fetches
/// the tested commit first, so `FETCH_HEAD` names it as a fetch of the bundle did, then
/// what the cache keeps, never following tags, so a tag deleted since an earlier proof
/// stays gone, names the tested commit and the tags of this proof's list alone,
/// and checks the commit out on its branch, as `actions/checkout` does for a push, or
/// detached when it has no branch without doubt, then removes everything untracked. It
/// runs as root in Horizon's base image, so it starts from a fresh git folder with no
/// global settings and no hooks: an earlier proof's CI could have written to the
/// checkout's .git and to HOME, and none of that may run here. Git LFS files stay
/// pointers, as `actions/checkout` leaves them without `lfs: true`; the bundle has no LFS
/// objects, and `hydrate` brings them when the commit has any.
pub(super) const CLONE: &str = "set -e; export HOME=/tmp/horizon-clone GIT_CONFIG_GLOBAL=/dev/null GIT_LFS_SKIP_SMUDGE=1; mkdir -p \"$HOME\"; \
git config --system --add safe.directory '*'; G='git -c core.hooksPath=/dev/null -c core.fsmonitor=false'; \
if [ ! -f /cache/HEAD ]; then git init -q --bare /cache; fi; \
if [ -f /src.bundle ]; then $G -C /cache fetch -q --no-tags /src.bundle '+refs/*:refs/incoming/*'; fi; \
while read -r object name; do $G -C /cache update-ref \"refs/kept/$object\" \"$object\"; done < /names; \
$G -C /cache update-ref \"refs/kept/$HEAD_SHA\" \"$HEAD_SHA\"; \
$G -C /cache for-each-ref --format='delete %(refname)' refs/incoming | $G -C /cache update-ref --stdin; \
rm -rf /workspace/checkout; mkdir /workspace/checkout; cd /workspace/checkout; git init -q; \
$G fetch -q --no-tags /cache \"refs/kept/$HEAD_SHA\" '+refs/kept/*:refs/kept/*'; \
while read -r object name; do $G update-ref \"$name\" \"$object\"; done < /names; \
$G for-each-ref --format='delete %(refname)' refs/kept | $G update-ref --stdin; \
if [ -n \"$HEAD_BRANCH\" ]; then $G checkout -q --force -B \"$HEAD_BRANCH\" \"$HEAD_SHA\"; \
else $G checkout -q --force --detach \"$HEAD_SHA\"; fi; $G clean -q -ffdx; \
chown -R 10001:10001 /workspace/checkout";

/// Hands the volume to the worker's account, with the folders a worker session names:
/// HOME, the session's data folder and the GPU lock's folder. They stay between runs for
/// their caches, but a link an earlier run left in their place goes, so nothing is made
/// through it.
const OWN: &str = "for d in /workspace/home /workspace/session-data /workspace/session-data/proof /workspace/locks; do \
if [ -L \"$d\" ]; then rm -f \"$d\"; fi; mkdir -p \"$d\"; done; \
chown 10001:10001 /workspace /workspace/home /workspace/session-data /workspace/session-data/proof /workspace/locks";

/// Gives the worker's account back what a preview of the run that ran as root left in
/// those folders (links themselves, never followed). It runs only when Horizon knows
/// such a preview ran; nothing on the volume decides it.
const TAKE_BACK: &str =
    "find /workspace/home /workspace/session-data /workspace/locks ! -user 10001 -exec chown -h 10001:10001 {} +";

#[cfg(all(test, unix))]
mod tests;
