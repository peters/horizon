//! How long a clone has left. When GitHub tells the repository's size, the whole clone is
//! judged from what has arrived and the speed Git reports; otherwise only the step under way
//! is, from how long its first part took.
use super::{Progress, Remote, Snapshot, Token, update};
use std::{
    path::Path,
    time::{Duration, Instant},
};

/// The phase in which Git receives the repository's data.
const RECEIVING: &str = "Receiving objects";

/// Takes one progress line of Git into `snapshot`: what this fetch has received, and when the
/// clone, or else the step, should end. `step_left` is the step's own estimate.
pub(super) fn take(snapshot: &mut Snapshot, phase: &str, detail: &str, step_left: Option<Duration>, now: Instant) {
    if phase == RECEIVING
        && let Some((received, speed)) = transfer(detail)
    {
        snapshot.receiving = received;
        let arrived = snapshot.received.saturating_add(received);
        if let Some(expected) = snapshot.expected.filter(|expected| *expected > arrived)
            && speed > 0
        {
            let left = Duration::from_millis((expected - arrived).saturating_mul(1000) / speed);
            let ends = now.checked_add(left);
            if ends.is_some() {
                snapshot.ends = ends;
                snapshot.whole = true;
                return;
            }
        }
    }
    // The whole clone's estimate goes on counting down through phases that receive nothing,
    // until it runs out; then the step's own is all there is.
    if snapshot.whole && snapshot.ends.is_some_and(|ends| ends > now) {
        return;
    }
    snapshot.whole = false;
    snapshot.ends = step_left.and_then(|left| now.checked_add(left));
}

/// What Git says it received and how fast, from `12.30 MiB | 4.50 MiB/s`: bytes and bytes
/// per second.
fn transfer(detail: &str) -> Option<(u64, u64)> {
    let (amount, speed) = detail.split_once('|')?;
    Some((size(amount.trim())?, size(speed.trim().strip_suffix("/s")?)?))
}

/// A size as Git writes it, with at most two decimals: `123 bytes`, `512.00 KiB`,
/// `12.30 MiB` or `1.20 GiB`.
fn size(text: &str) -> Option<u64> {
    let (number, unit) = text.split_once(' ')?;
    let scale: u64 = match unit {
        "byte" | "bytes" => 1,
        "KiB" => 1 << 10,
        "MiB" => 1 << 20,
        "GiB" => 1 << 30,
        _ => return None,
    };
    let (whole, fraction) = number.split_once('.').unwrap_or((number, ""));
    let digits = |part: &str| part.bytes().all(|byte| byte.is_ascii_digit());
    if whole.is_empty() || !digits(whole) || fraction.len() > 2 || !digits(fraction) {
        return None;
    }
    let hundredths = whole
        .parse::<u64>()
        .ok()?
        .checked_mul(100)?
        .checked_add(format!("{fraction:0<2}").parse::<u64>().ok()?)?;
    Some(hundredths.checked_mul(scale)? / 100)
}

/// Asks GitHub, off the clone's way, how much a full clone of `remote` receives, and puts the
/// answer into `progress`. Other hosts are not asked; nor is GitHub for a token of another host.
pub(super) fn look_up(remote: &Remote, token: Option<&Token>, progress: &Progress) {
    if remote.host != "github.com" {
        return;
    }
    let repository = format!("{}/{}", remote.owner, remote.name);
    let secret = token
        .filter(|token| token.scope == "https://github.com/")
        .map(|token| horizon_cloud::github::Secret::new(token.secret.to_string()));
    let progress = progress.clone();
    std::thread::spawn(move || {
        let size = horizon_cloud::github::Client::new().repository_size(&repository, secret.as_ref());
        if let Ok(size) = size.map_err(|error| tracing::debug!(%error, "no repository size for the clone's estimate"))
            && size > 0
        {
            update(&progress, |snapshot| snapshot.expected = Some(size));
        }
    });
}

/// What an earlier try at the clone in `folder` received: the size of its objects.
pub(super) fn received_before(folder: &Path) -> u64 {
    fn files(folder: &Path, depth: u8) -> u64 {
        let Ok(entries) = std::fs::read_dir(folder) else {
            return 0;
        };
        entries
            .flatten()
            .filter_map(|entry| Some((entry.path(), entry.file_type().ok()?)))
            .map(|(path, kind)| {
                if kind.is_file() {
                    std::fs::metadata(&path).map_or(0, |meta| meta.len())
                } else if kind.is_dir() && depth > 0 {
                    files(&path, depth - 1)
                } else {
                    0
                }
            })
            .sum()
    }
    // `objects/pack/*` and `objects/ab/*`: links are not followed.
    files(&folder.join(".git").join("objects"), 1)
}

#[cfg(test)]
mod tests;
