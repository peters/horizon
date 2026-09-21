//! Bounded asynchronous discovery; creating a session never performs a catalog HTTP request.
use super::{CatalogDevice, CatalogError, CatalogPage, CatalogQuery, target_provider};
use crate::remote::RemoteProviderProfile;
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc::{Receiver, TryRecvError, channel},
    },
    time::{Duration, Instant},
};

const FRESH: Duration = Duration::from_secs(300);
const RETRY: Duration = Duration::from_secs(15);
static ACTIVE_FETCHES: AtomicUsize = AtomicUsize::new(0);
struct Permit;
impl Permit {
    fn acquire() -> Option<Self> {
        ACTIVE_FETCHES
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| (n < 32).then_some(n + 1))
            .ok()
            .map(|_| Self)
    }
}
impl Drop for Permit {
    fn drop(&mut self) {
        ACTIVE_FETCHES.fetch_sub(1, Ordering::Release);
    }
}

struct Entry {
    profile: RemoteProviderProfile,
    started: Instant,
    pending: Option<Receiver<Result<Vec<CatalogDevice>, CatalogError>>>,
    result: Option<Result<Vec<CatalogDevice>, CatalogError>>,
}

#[derive(Default)]
pub struct CatalogCache {
    credential_generation: u64,
    entries: BTreeMap<String, Entry>,
}
impl CatalogCache {
    /// Discard completed and pending rows after an in-process credential change.
    pub fn invalidate_credentials(&mut self, generation: u64) {
        if self.credential_generation != generation {
            self.entries.clear();
            self.credential_generation = generation;
        }
    }

    /// Returns true when a new bounded fetch is needed. Changed account bindings invalidate cached rows.
    pub fn needs_refresh(&mut self, name: &str, profile: &RemoteProviderProfile) -> bool {
        self.poll();
        self.entries.get(name).is_none_or(|entry| {
            entry.profile != *profile
                || entry.started.elapsed()
                    >= if matches!(entry.result, Some(Ok(_))) {
                        FRESH
                    } else {
                        RETRY
                    }
        })
    }

    pub fn start(
        &mut self,
        name: &str,
        profile: &RemoteProviderProfile,
        fetch: impl FnOnce() -> Result<Vec<CatalogDevice>, CatalogError> + Send + 'static,
    ) {
        if !self.needs_refresh(name, profile) {
            return;
        }
        // Completed entries can be evicted; active workers stay bounded independently of the request queue.
        if self.entries.len() >= 32 && !self.entries.contains_key(name) {
            if let Some(old) = self
                .entries
                .iter()
                .find(|(_, e)| e.pending.is_none())
                .map(|(n, _)| n.clone())
            {
                self.entries.remove(&old);
            } else {
                return;
            }
        }
        let Some(permit) = Permit::acquire() else { return };
        let (tx, rx) = channel();
        let job = std::thread::Builder::new()
            .name("provider-catalog".into())
            .spawn(move || {
                let _permit = permit;
                let _ = tx.send(fetch());
            });
        self.entries.insert(
            name.into(),
            Entry {
                profile: profile.clone(),
                started: Instant::now(),
                pending: job.is_ok().then_some(rx),
                result: job.err().map(|_| Err(CatalogError::Unavailable)),
            },
        );
    }

    pub fn poll(&mut self) {
        for entry in self.entries.values_mut() {
            let Some(rx) = &entry.pending else { continue };
            let result = match rx.try_recv() {
                Ok(result) => result,
                Err(TryRecvError::Empty) => continue,
                Err(TryRecvError::Disconnected) => Err(CatalogError::Unavailable),
            };
            entry.pending = None;
            entry.result = Some(result);
        }
    }

    /// # Errors
    /// A failed fetch or a changed/expired provider binding. None means discovery is still running.
    pub fn page(
        &self,
        profile: &RemoteProviderProfile,
        query: &CatalogQuery,
    ) -> Result<Option<CatalogPage>, CatalogError> {
        self.rows(profile, &query.provider)
            .map(|rows| rows.map(|rows| CatalogPage::select(rows, query)))
    }

    /// # Errors
    /// Undiscovered, expired or unavailable combinations must be discovered again, never guessed.
    pub fn target(&self, profile: &RemoteProviderProfile, target: &str) -> Result<&CatalogDevice, CatalogError> {
        let provider = target_provider(target).ok_or(CatalogError::RefreshRequired)?;
        self.rows(profile, provider)?
            .and_then(|rows| rows.iter().find(|row| row.target == target))
            .ok_or(CatalogError::RefreshRequired)
    }

    fn rows(&self, profile: &RemoteProviderProfile, provider: &str) -> Result<Option<&[CatalogDevice]>, CatalogError> {
        let entry = self
            .entries
            .get(provider)
            .filter(|entry| entry.profile == *profile && entry.started.elapsed() < FRESH)
            .ok_or(CatalogError::RefreshRequired)?;
        match &entry.result {
            Some(Ok(rows)) => Ok(Some(rows)),
            Some(Err(error)) => Err(*error),
            None if entry.started.elapsed() < RETRY => Ok(None),
            None => Err(CatalogError::Unavailable),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn profile() -> RemoteProviderProfile {
        serde_json::from_str(r#"{"adapter":"browserstack","endpoint":"https://hub-cloud.browserstack.com/wd/hub"}"#)
            .unwrap()
    }
    #[test]
    fn credential_change_discards_ready_rows_and_late_results() {
        let profile = profile();
        let mut cache = CatalogCache::default();
        let rows = super::super::decode(
            "account",
            br#"[{"os":"ios","os_version":"18","browser":"iphone","device":"iPhone 16","real_mobile":true}]"#,
        )
        .unwrap();
        let target = rows[0].target.clone();
        cache.entries.insert(
            "account".into(),
            Entry {
                profile: profile.clone(),
                started: Instant::now(),
                pending: None,
                result: Some(Ok(rows.clone())),
            },
        );
        assert!(cache.target(&profile, &target).is_ok());
        cache.invalidate_credentials(1);
        assert!(cache.entries.is_empty());
        assert_eq!(
            cache.target(&profile, &target).unwrap_err(),
            CatalogError::RefreshRequired
        );
        let (old_result, old_receiver) = channel();
        cache.entries.insert(
            "account".into(),
            Entry {
                profile: profile.clone(),
                started: Instant::now(),
                pending: Some(old_receiver),
                result: None,
            },
        );
        cache.invalidate_credentials(2);
        assert!(
            old_result.send(Ok(rows)).is_err(),
            "an old credential fetch cannot repopulate the cache"
        );
        cache.poll();
        assert!(cache.needs_refresh("account", &profile));
        assert_eq!(
            cache
                .page(
                    &profile,
                    &CatalogQuery {
                        provider: "account".into(),
                        ..Default::default()
                    }
                )
                .unwrap_err(),
            CatalogError::RefreshRequired
        );
    }
    #[test]
    fn bounded_rebinding_recovers_while_old_fetch_waits_and_expiry_invalidates_targets() {
        let profile = profile();
        let mut cache = CatalogCache::default();
        let rows = super::super::decode(
            "account",
            br#"[{"os":"ios","os_version":"18","browser":"iphone","device":"iPhone 16","real_mobile":true}]"#,
        )
        .unwrap();
        let target = rows[0].target.clone();
        let (release, wait) = channel();
        cache.start("account", &profile, move || {
            wait.recv().unwrap();
            Ok(rows)
        });
        cache.entries.get_mut("account").unwrap().started -= RETRY * 2;
        let mut changed = profile.clone();
        changed.limits.max_session_seconds += 1;
        assert!(
            cache.needs_refresh("account", &changed),
            "rebinding may replace the consumer while the old worker retains its permit"
        );
        assert_eq!(
            cache.target(&changed, &target).unwrap_err(),
            CatalogError::RefreshRequired
        );
        let replacement = super::super::decode(
            "account",
            br#"[{"os":"ios","os_version":"18","browser":"iphone","device":"iPhone 16","real_mobile":true}]"#,
        )
        .unwrap();
        cache.start("account", &changed, move || Ok(replacement));
        release.send(()).unwrap();
        let end = Instant::now() + Duration::from_secs(2);
        while cache.entries["account"].pending.is_some() {
            cache.poll();
            assert!(Instant::now() < end);
            std::thread::yield_now();
        }
        assert_eq!(
            cache.target(&changed, &target).unwrap().device.as_deref(),
            Some("iPhone 16")
        );
        assert!(!cache.needs_refresh("account", &changed));
        cache.entries.get_mut("account").unwrap().started -= FRESH;
        assert_eq!(
            cache.target(&changed, &target).unwrap_err(),
            CatalogError::RefreshRequired
        );
    }
}
