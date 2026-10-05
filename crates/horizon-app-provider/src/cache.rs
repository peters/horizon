use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use horizon_app_testing::contract::Platform;
use serde::Serialize;
use uuid::Uuid;

use crate::api::{BrowserStack, UploadedApp};
use crate::artifact::Artifact;
use crate::{Error, Result};

// Shorter than provider retention. Last active lease release deletes the owned upload.
const MAX_AGE: Duration = Duration::from_hours(24);
const MAX_ASSETS: usize = 32;

pub trait Uploads {
    /// # Errors
    /// A host journal must record operation before this request; uncertain outcomes cannot be retried.
    fn upload(&self, artifact: &mut Artifact, operation: Uuid) -> Result<UploadedApp>;
    /// # Errors
    /// A failure retains the cache lease so the owner can reconcile it.
    fn delete(&self, app: &UploadedApp) -> Result<()>;
}

impl Uploads for BrowserStack {
    fn upload(&self, artifact: &mut Artifact, operation: Uuid) -> Result<UploadedApp> {
        self.upload(artifact, operation)
    }
    fn delete(&self, app: &UploadedApp) -> Result<()> {
        self.delete_app(app)
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct AppHandle {
    pub id: Uuid,
    pub platform: Platform,
    pub sha256: String,
    pub bytes: u64,
    /// Remaining lifetime at issue time, not a new retention period for a reused asset.
    pub remaining_seconds: u64,
}

struct Asset {
    app: UploadedApp,
    platform: Platform,
    sha256: String,
    acquired: Instant,
    deletion_uncertain: bool,
}

/// One host realm/workspace owns this cache. Callers must serialize access through the shared actor.
pub struct Cache {
    owner: Uuid,
    backend: Arc<dyn Uploads>,
    assets: BTreeMap<Uuid, Asset>,
    leases: BTreeMap<Uuid, Uuid>,
}

impl Cache {
    #[must_use]
    pub fn new(owner: Uuid, backend: Arc<dyn Uploads>) -> Self {
        Self {
            owner,
            backend,
            assets: BTreeMap::new(),
            leases: BTreeMap::new(),
        }
    }

    /// # Errors
    /// Only the owner may upload or reuse; the operation must already exist in its durable journal.
    pub fn upload(&mut self, owner: Uuid, artifact: &mut Artifact, operation: Uuid) -> Result<AppHandle> {
        if owner != self.owner {
            return Err(Error::OwnershipRefused);
        }
        if self.leases.len() >= 256 {
            return Err(Error::CacheFull);
        }
        if self.assets.values().any(|asset| {
            asset.deletion_uncertain && asset.platform == artifact.platform() && asset.sha256 == artifact.sha256()
        }) {
            return Err(Error::AppReleaseUncertain);
        }
        let found = self
            .assets
            .iter()
            .find(|(_, asset)| {
                asset.platform == artifact.platform()
                    && !asset.deletion_uncertain
                    && asset.sha256 == artifact.sha256()
                    && asset.acquired.elapsed() < MAX_AGE
            })
            .map(|(id, _)| *id);
        let asset_id = if let Some(id) = found {
            id
        } else {
            if self.assets.len() >= MAX_ASSETS {
                return Err(Error::CacheFull);
            }
            let app = self.backend.upload(artifact, operation)?;
            let id = Uuid::new_v4();
            self.assets.insert(
                id,
                Asset {
                    app,
                    platform: artifact.platform(),
                    sha256: artifact.sha256().to_owned(),
                    acquired: Instant::now(),
                    deletion_uncertain: false,
                },
            );
            id
        };
        let id = Uuid::new_v4();
        self.leases.insert(id, asset_id);
        Ok(AppHandle {
            id,
            platform: artifact.platform(),
            sha256: artifact.sha256().to_owned(),
            bytes: artifact.bytes(),
            remaining_seconds: MAX_AGE
                .saturating_sub(self.assets.get(&asset_id).ok_or(Error::AppExpired)?.acquired.elapsed())
                .as_secs(),
        })
    }

    /// # Errors
    /// Opaque handles are owner-bound and expire; private provider values reach only the host driver callback.
    pub fn use_for_driver<T>(&self, owner: Uuid, handle: Uuid, host: impl FnOnce(&str) -> T) -> Result<T> {
        if owner != self.owner {
            return Err(Error::OwnershipRefused);
        }
        let asset = self
            .leases
            .get(&handle)
            .and_then(|id| self.assets.get(id))
            .ok_or(Error::AppExpired)?;
        if asset.deletion_uncertain {
            return Err(Error::AppReleaseUncertain);
        }
        if asset.acquired.elapsed() >= MAX_AGE {
            return Err(Error::AppExpired);
        }
        Ok(asset.app.use_for_driver(host))
    }

    /// # Errors
    /// Shared uploads survive until their final lease closes; uncertain deletion keeps the exact owned lease.
    pub fn release(&mut self, owner: Uuid, handle: Uuid) -> Result<()> {
        if owner != self.owner {
            return Err(Error::OwnershipRefused);
        }
        let Some(asset_id) = self.leases.get(&handle).copied() else {
            return Ok(());
        };
        if self.leases.values().filter(|id| **id == asset_id).count() == 1 {
            let asset = self.assets.get_mut(&asset_id).ok_or(Error::AppExpired)?;
            asset.deletion_uncertain = true;
            self.backend.delete(&asset.app)?;
            self.assets.remove(&asset_id);
        }
        self.leases.remove(&handle);
        Ok(())
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use horizon_app_testing::contract::Contract;
    use serde_json::json;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    #[derive(Default)]
    struct Fake {
        uploads: AtomicUsize,
        deletes: AtomicUsize,
        fail_delete: AtomicBool,
    }
    impl Uploads for Fake {
        fn upload(&self, _artifact: &mut Artifact, _operation: Uuid) -> Result<UploadedApp> {
            self.uploads.fetch_add(1, Ordering::SeqCst);
            UploadedApp::from_response(&json!({"app_url":"bs://aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}))
        }
        fn delete(&self, _app: &UploadedApp) -> Result<()> {
            self.deletes.fetch_add(1, Ordering::SeqCst);
            if self.fail_delete.load(Ordering::SeqCst) {
                Err(Error::ProviderFailed)
            } else {
                Ok(())
            }
        }
    }

    #[cfg(unix)]
    fn artifact() -> Artifact {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("App.ipa"), b"PK\x03\x04data").unwrap();
        let contract = Contract::from_agents("```yaml\nremote-device-testing:\n  version: 1\n  provider: browserstack\n  apps:\n    ios:\n      build: [build]\n      artifact: App.ipa\n      bundle_id: com.example.app\n  matrix: [{platform: ios, form: phone}]\n  recipes: [recipe.md]\n```").unwrap();
        Artifact::capture(root.path(), &contract, Platform::Ios).unwrap()
    }

    #[cfg(unix)]
    #[test]
    fn unchanged_content_uploads_once_and_deletes_after_both_leases_close() {
        let owner = Uuid::new_v4();
        let backend = Arc::new(Fake::default());
        let mut cache = Cache::new(owner, backend.clone());
        let mut file = artifact();
        let first = cache.upload(owner, &mut file, Uuid::new_v4()).unwrap();
        let second = cache.upload(owner, &mut file, Uuid::new_v4()).unwrap();
        assert_ne!(first.id, second.id);
        assert_eq!(backend.uploads.load(Ordering::SeqCst), 1);
        assert!(!serde_json::to_string(&first).unwrap().contains("bs://"));
        assert_eq!(
            cache.use_for_driver(Uuid::new_v4(), first.id, str::len).err(),
            Some(Error::OwnershipRefused)
        );
        cache.release(owner, first.id).unwrap();
        assert_eq!(backend.deletes.load(Ordering::SeqCst), 0);
        cache.release(owner, second.id).unwrap();
        cache.release(owner, second.id).unwrap();
        assert_eq!(backend.deletes.load(Ordering::SeqCst), 1);
        assert_eq!(
            cache.use_for_driver(owner, second.id, str::len).err(),
            Some(Error::AppExpired)
        );
    }

    #[cfg(unix)]
    #[test]
    fn expiry_and_uncertain_release_cannot_drop_owned_resources() {
        let owner = Uuid::new_v4();
        let backend = Arc::new(Fake::default());
        let mut cache = Cache::new(owner, backend.clone());
        let handle = cache.upload(owner, &mut artifact(), Uuid::new_v4()).unwrap();
        cache.assets.values_mut().next().unwrap().acquired = Instant::now().checked_sub(MAX_AGE).unwrap();
        assert_eq!(
            cache.use_for_driver(owner, handle.id, str::len).err(),
            Some(Error::AppExpired)
        );
        backend.fail_delete.store(true, Ordering::SeqCst);
        assert_eq!(cache.release(owner, handle.id).err(), Some(Error::ProviderFailed));
        assert!(cache.leases.contains_key(&handle.id));
        backend.fail_delete.store(false, Ordering::SeqCst);
        cache.release(owner, handle.id).unwrap();
        assert!(cache.leases.is_empty());
    }
    #[test]
    fn same_workspace_keeps_different_credential_backends_separate() {
        let owner = Uuid::new_v4();
        let first = Arc::new(Fake::default());
        let second = Arc::new(Fake::default());
        let mut a = Cache::new(owner, first.clone());
        let mut b = Cache::new(owner, second.clone());
        let a_handle = a.upload(owner, &mut artifact(), Uuid::new_v4()).unwrap();
        let b_handle = b.upload(owner, &mut artifact(), Uuid::new_v4()).unwrap();
        assert_eq!(first.uploads.load(Ordering::SeqCst), 1);
        assert_eq!(second.uploads.load(Ordering::SeqCst), 1);
        assert_eq!(
            b.use_for_driver(owner, a_handle.id, str::len).err(),
            Some(Error::AppExpired)
        );
        a.release(owner, a_handle.id).unwrap();
        assert_eq!(first.deletes.load(Ordering::SeqCst), 1);
        assert_eq!(second.deletes.load(Ordering::SeqCst), 0);
        b.release(owner, b_handle.id).unwrap();
        assert_eq!(second.deletes.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn lost_deletion_reply_holds_reuse_and_driver_until_exact_cleanup_is_confirmed() {
        let owner = Uuid::new_v4();
        let backend = Arc::new(Fake::default());
        let mut cache = Cache::new(owner, backend.clone());
        let handle = cache.upload(owner, &mut artifact(), Uuid::new_v4()).unwrap();
        backend.fail_delete.store(true, Ordering::SeqCst);
        assert_eq!(cache.release(owner, handle.id), Err(Error::ProviderFailed));
        assert_eq!(
            cache.upload(owner, &mut artifact(), Uuid::new_v4()).err(),
            Some(Error::AppReleaseUncertain)
        );
        assert_eq!(
            cache.use_for_driver(owner, handle.id, str::len).err(),
            Some(Error::AppReleaseUncertain)
        );
        assert_eq!(backend.uploads.load(Ordering::SeqCst), 1);
        backend.fail_delete.store(false, Ordering::SeqCst);
        cache.release(owner, handle.id).unwrap();
        assert!(cache.assets.is_empty());
    }

    #[test]
    fn reused_handle_reports_the_assets_remaining_lifetime() {
        let owner = Uuid::new_v4();
        let backend = Arc::new(Fake::default());
        let mut cache = Cache::new(owner, backend.clone());
        cache.upload(owner, &mut artifact(), Uuid::new_v4()).unwrap();
        cache.assets.values_mut().next().unwrap().acquired = Instant::now()
            .checked_sub(MAX_AGE.checked_sub(Duration::from_secs(3)).unwrap())
            .unwrap();
        let reused = cache.upload(owner, &mut artifact(), Uuid::new_v4()).unwrap();
        assert!(reused.remaining_seconds <= 3);
        assert_eq!(backend.uploads.load(Ordering::SeqCst), 1);
    }
}
