//! The build slots of a Docker daemon: the heavy part of a proof (the image build, the CI
//! and the fast rerun) runs for only a few setups at once on one daemon, so several
//! setups do not overload one computer. The others wait for a free slot in order.
use horizon_core::cloud_runtime::Cancellation;
use std::collections::BTreeMap;
use std::sync::{Condvar, Mutex, PoisonError};
use std::time::Duration;

/// How often a waiting proof looks whether it was stopped.
const CHECK_EVERY: Duration = Duration::from_millis(250);

/// For each daemon, the setups that hold its slots and those that wait, in order.
#[derive(Default)]
struct Queue {
    holders: Vec<String>,
    waiting: Vec<u64>,
}

static QUEUES: Mutex<BTreeMap<String, Queue>> = Mutex::new(BTreeMap::new());
static FREED: Condvar = Condvar::new();
static NEXT_TICKET: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// Names the number of build slots of each daemon in place of the default, from 1 to 16.
const SLOTS_VARIABLE: &str = "HORIZON_SETUP_BUILD_SLOTS";

/// How many proofs build and run their CI at once on one daemon: [`SLOTS_VARIABLE`] when
/// it names a number from 1 to 16, else one for each 16 processors of this computer, at
/// least one and at most four.
pub(super) fn limit() -> usize {
    limit_from(
        std::env::var(SLOTS_VARIABLE).ok().as_deref(),
        std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get),
    )
}

fn limit_from(variable: Option<&str>, processors: usize) -> usize {
    variable
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|slots| (1..=16).contains(slots))
        .unwrap_or_else(|| (processors / 16).clamp(1, 4))
}

/// A held build slot; it is free again once this is dropped.
pub(super) struct Slot {
    daemon: String,
    holder: String,
}

impl Drop for Slot {
    fn drop(&mut self) {
        let mut queues = QUEUES.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(queue) = queues.get_mut(&self.daemon)
            && let Some(index) = queue.holders.iter().position(|holder| *holder == self.holder)
        {
            queue.holders.remove(index);
        }
        FREED.notify_all();
    }
}

/// Takes one of `limit` slots of `daemon` for `holder`, the repository the proof sets up.
/// While all are held, `waiting` hears who holds them, each time that changes. `None`
/// when `cancel` stopped the proof first.
pub(super) fn take(
    daemon: &str,
    holder: &str,
    limit: usize,
    cancel: &Cancellation,
    waiting: &mut dyn FnMut(&[String]),
) -> Option<Slot> {
    let ticket = NEXT_TICKET.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut queues = QUEUES.lock().unwrap_or_else(PoisonError::into_inner);
    queues.entry(daemon.to_owned()).or_default().waiting.push(ticket);
    let mut told: Option<Vec<String>> = None;
    loop {
        let queue = queues.entry(daemon.to_owned()).or_default();
        if cancel.is_cancelled() {
            queue.waiting.retain(|waiting| *waiting != ticket);
            FREED.notify_all();
            return None;
        }
        // First come, first served: a slot goes to the proof that waited longest.
        if queue.holders.len() < limit.max(1) && queue.waiting.first() == Some(&ticket) {
            queue.waiting.remove(0);
            queue.holders.push(holder.to_owned());
            // The next in line may take a slot that is still free.
            FREED.notify_all();
            return Some(Slot {
                daemon: daemon.to_owned(),
                holder: holder.to_owned(),
            });
        }
        if told.as_ref() != Some(&queue.holders) {
            let holders = queue.holders.clone();
            drop(queues);
            waiting(&holders);
            told = Some(holders);
            queues = QUEUES.lock().unwrap_or_else(PoisonError::into_inner);
            continue;
        }
        queues = FREED
            .wait_timeout(queues, CHECK_EVERY)
            .unwrap_or_else(PoisonError::into_inner)
            .0;
    }
}

/// A slot of `daemon` for the proof of `repository`, as [`take`] gives one with the
/// default limit. `tell` hears who holds the slots while it waits, then an empty list.
pub(super) fn take_for(
    daemon: Option<&str>,
    repository: &std::path::Path,
    cancel: &Cancellation,
    tell: impl Fn(&[String]),
) -> Option<Slot> {
    let holder = repository.file_name().map_or_else(
        || repository.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    );
    let slot = take(daemon.unwrap_or_default(), &holder, limit(), cancel, &mut |holders| {
        tell(holders);
    })?;
    tell(&[]);
    Some(slot)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn the_slots_follow_the_processors_unless_the_variable_names_them() {
        assert_eq!(limit_from(None, 8), 1, "a laptop builds one setup at a time");
        assert_eq!(limit_from(None, 48), 3);
        assert_eq!(limit_from(None, 256), 4);
        assert_eq!(limit_from(Some("2"), 8), 2);
        assert_eq!(limit_from(Some("0"), 48), 3, "zero slots would build nothing");
        assert_eq!(limit_from(Some("many"), 48), 3);
        assert_eq!(limit_from(Some("17"), 48), 3);
    }

    #[test]
    fn a_full_daemon_makes_the_next_proof_wait_until_a_slot_is_free() {
        let cancel = Cancellation::default();
        let first = take("test-daemon-full", "first", 1, &cancel, &mut |_| {}).unwrap();
        let (told, heard) = std::sync::mpsc::channel();
        let waiter = std::thread::spawn(move || {
            let cancel = Cancellation::default();
            let slot = take("test-daemon-full", "second", 1, &cancel, &mut |holders| {
                let _ = told.send(holders.to_vec());
            });
            (slot.is_some(), Instant::now())
        });
        assert_eq!(
            heard.recv_timeout(Duration::from_secs(5)).unwrap(),
            ["first"],
            "the waiting proof hears who builds"
        );
        std::thread::sleep(Duration::from_millis(200));
        assert!(!waiter.is_finished(), "it waits while the slot is held");
        let freed = Instant::now();
        drop(first);
        let (took, at) = waiter.join().unwrap();
        assert!(took && at >= freed);
    }

    #[test]
    fn another_daemon_has_slots_of_its_own() {
        let cancel = Cancellation::default();
        let _held = take("test-daemon-a", "first", 1, &cancel, &mut |_| {}).unwrap();
        let other = take("test-daemon-b", "second", 1, &cancel, &mut |_| panic!("no wait"));
        assert!(other.is_some());
    }

    #[test]
    fn a_stopped_proof_leaves_the_queue_and_the_next_one_goes_on() {
        let held_cancel = Cancellation::default();
        let held = take("test-daemon-stop", "first", 1, &held_cancel, &mut |_| {}).unwrap();
        let stopped = Cancellation::default();
        let stop = stopped.clone();
        let waiter = std::thread::spawn(move || take("test-daemon-stop", "second", 1, &stopped, &mut |_| {}).is_none());
        let third = std::thread::spawn(|| {
            let cancel = Cancellation::default();
            take("test-daemon-stop", "third", 1, &cancel, &mut |_| {}).is_some()
        });
        std::thread::sleep(Duration::from_millis(300));
        stop.cancel();
        assert!(waiter.join().unwrap(), "a stopped proof takes no slot");
        drop(held);
        assert!(third.join().unwrap(), "the proof after it still gets the slot");
    }
}
