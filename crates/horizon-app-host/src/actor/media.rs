//! Verified provider references and bounded media exports.
use super::{Actor, Duration, Error, Result, Uuid};

impl Actor {
    pub(crate) fn retain_run_evidence(&self) -> Result<crate::observations::Run<'_>> {
        self.observations.begin_run()
    }

    /// # Errors
    /// Evidence reads use only the original verified driver ID, including after acknowledged closure.
    pub fn media(&self, id: Uuid, kind: horizon_app_provider::media::Kind) -> Result<Vec<u8>> {
        self.media_with_timeout(id, kind, Duration::from_secs(30))
    }
    pub(crate) fn media_with_timeout(
        &self,
        id: Uuid,
        kind: horizon_app_provider::media::Kind,
        timeout: Duration,
    ) -> Result<Vec<u8>> {
        self.export_media(id, kind, timeout, Ok)
    }
    pub(crate) fn export_media<T>(
        &self,
        id: Uuid,
        kind: horizon_app_provider::media::Kind,
        timeout: Duration,
        export: impl FnOnce(Vec<u8>) -> Result<T>,
    ) -> Result<T> {
        self.audit.execute(Some(id), "media", || {
            let _read = MediaRead::acquire(&self.media_reads)?;
            let reference = self.observations.reference(id)?;
            export(self.backend.media(&reference, kind, timeout)?)
        })
    }
    pub(crate) fn session_link(&self, id: Uuid, timeout: Duration) -> Result<String> {
        self.audit.execute(Some(id), "session_link", || {
            let reference = self.observations.reference(id)?;
            self.backend.session_link(&reference, timeout)
        })
    }
    /// # Errors
    /// Provider recording is selected at allocation by the project's evidence policy.
    pub fn video_enabled(&self, id: Uuid) -> Result<bool> {
        let _reference = self.observations.reference(id)?;
        Ok(self.contract.evidence.video)
    }
}
// Media reads never acquire lane locks or renew resources. Refuse bursts before allocating bodies.
struct MediaRead<'a>(&'a std::sync::atomic::AtomicUsize);
impl<'a> MediaRead<'a> {
    fn acquire(count: &'a std::sync::atomic::AtomicUsize) -> Result<Self> {
        use std::sync::atomic::Ordering;
        count
            .try_update(Ordering::AcqRel, Ordering::Acquire, |value| {
                (value < 2).then_some(value + 1)
            })
            .map_err(|_| Error::MediaBusy)?;
        Ok(Self(count))
    }
}
impl Drop for MediaRead<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
    }
}
#[cfg(test)]
mod media_admission_tests {
    use super::*;
    #[test]
    fn two_reads_are_admitted_and_errors_release_the_slot() {
        let count = std::sync::atomic::AtomicUsize::new(0);
        let first = MediaRead::acquire(&count).unwrap();
        let second = MediaRead::acquire(&count).unwrap();
        assert!(matches!(MediaRead::acquire(&count), Err(Error::MediaBusy)));
        drop(first);
        let replacement = MediaRead::acquire(&count).unwrap();
        assert!(matches!(MediaRead::acquire(&count), Err(Error::MediaBusy)));
        drop((second, replacement));
        assert_eq!(count.load(std::sync::atomic::Ordering::Acquire), 0);
    }
}
