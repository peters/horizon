//! How far a clone has come. The phase under way is judged from how long its first part took,
//! which holds well within one phase. The whole clone is told by what it received out of the
//! repository's size, when GitHub says it, and by how long it has run: measured on `git/git`,
//! receiving took 13 of 64 seconds, and the rest (GitHub counting and compressing objects,
//! Git resolving deltas, and the pauses between the clone's fetches) follows object counts
//! that nobody knows before the end, so a time left for the whole clone would be a guess.
use super::{Progress, Remote, Snapshot, Token, eta, update};
use std::{
    collections::VecDeque,
    path::Path,
    time::{Duration, Instant},
};

/// The phase in which Git receives the repository's data.
const RECEIVING: &str = "Receiving objects";
/// Phases that run on the host, whose progress jumps (compressing `git/git` took 5 s to reach
/// 30% and then under 1 s for the rest): no time left is told for them.
const ON_THE_HOST: [&str; 3] = ["Enumerating objects", "Counting objects", "Compressing objects"];
/// A phase that races through its start and then slows (resolving `git/git`'s deltas reached
/// 60% in 1 s and took 9 s more), judged by its pace over this last stretch.
const RESOLVING: &str = "Resolving deltas";
const WINDOW: Duration = Duration::from_secs(3);

/// What one phase's progress has been, for judging its time left.
pub(super) struct Seen {
    started: Instant,
    /// Each percent and when it was seen, the oldest at most one beyond [`WINDOW`].
    recent: VecDeque<(Instant, u8)>,
}

impl Seen {
    pub(super) fn new(started: Instant) -> Self {
        Self {
            started,
            recent: VecDeque::new(),
        }
    }

    /// Notes that `phase` reached `percent` at `now`, and tells how long it has left: from
    /// the pace since it began, from its pace over the last [`WINDOW`] for deltas, and not at
    /// all for a phase on the host.
    pub(super) fn left(&mut self, phase: &str, percent: u8, now: Instant) -> Option<Duration> {
        self.recent.push_back((now, percent));
        while self
            .recent
            .get(1)
            .is_some_and(|(at, _)| now.saturating_duration_since(*at) >= WINDOW)
        {
            self.recent.pop_front();
        }
        if ON_THE_HOST.contains(&phase) {
            return None;
        }
        if phase != RESOLVING {
            return eta(now.saturating_duration_since(self.started), percent);
        }
        let &(then, before) = self.recent.front()?;
        let span = now.saturating_duration_since(then);
        if !(1..100).contains(&percent) || percent <= before || span < Duration::from_secs(1) {
            return None;
        }
        Some(span.mul_f64(f64::from(100 - percent) / f64::from(percent - before)))
    }
}

/// Takes one progress line of Git into `snapshot`: what this fetch has received so far, and
/// when the phase should end, `phase_left` from `now`.
pub(super) fn take(snapshot: &mut Snapshot, phase: &str, detail: &str, phase_left: Option<Duration>, now: Instant) {
    if phase == RECEIVING
        && let Some(received) = received(detail)
    {
        snapshot.receiving = received;
    }
    snapshot.ends = phase_left.and_then(|left| now.checked_add(left));
}

/// What Git says it received, in bytes, from `12.30 MiB | 4.50 MiB/s` or `12.30 MiB`.
fn received(detail: &str) -> Option<u64> {
    size(detail.split('|').next()?.trim())
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
