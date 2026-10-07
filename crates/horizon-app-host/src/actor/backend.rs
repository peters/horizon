//! Provider adaptation for the ownership controller.
use super::{Artifact, Capacity, Device, Duration, Result, Uuid};
use horizon_app_provider::api::{BrowserStack, UploadedApp};
use horizon_browser::ClassicTransport;
use std::sync::Arc;

pub(crate) trait Backend: Send + Sync {
    fn capacity(&self, timeout: Duration) -> Result<Capacity>;
    fn driver(&self) -> Result<Arc<dyn ClassicTransport>>;
    fn verify(&self, id: &str, target: &Device, app: &UploadedApp, operation: Uuid, timeout: Duration) -> Result<()>;
    fn upload(&self, artifact: &mut Artifact, operation: Uuid, timeout: Duration) -> Result<UploadedApp>;
    fn delete(&self, app: &UploadedApp) -> Result<()>;
    fn confirmed_closed(&self, reference: &str) -> Result<bool>;
    fn session_link(&self, _reference: &str, _timeout: Duration) -> Result<String> {
        Err(horizon_app_provider::Error::MediaUnavailable.into())
    }
    fn media(&self, _reference: &str, _kind: horizon_app_provider::media::Kind, _timeout: Duration) -> Result<Vec<u8>> {
        Err(horizon_app_provider::Error::MediaUnavailable.into())
    }
}
impl Backend for BrowserStack {
    fn session_link(&self, reference: &str, timeout: Duration) -> Result<String> {
        Ok(self
            .session_with_timeout(reference, timeout.min(Duration::from_secs(10)))?
            .dashboard_link()?)
    }
    fn media(&self, reference: &str, kind: horizon_app_provider::media::Kind, timeout: Duration) -> Result<Vec<u8>> {
        Ok(BrowserStack::media_with_timeout(self, reference, kind, timeout)?)
    }
    fn capacity(&self, timeout: Duration) -> Result<Capacity> {
        let observation = self.native_capacity(timeout)?;
        Ok(Capacity::observed(observation.quota, observation.running)?)
    }
    fn driver(&self) -> Result<Arc<dyn ClassicTransport>> {
        Ok(BrowserStack::driver(self)?)
    }
    fn verify(&self, id: &str, target: &Device, app: &UploadedApp, operation: Uuid, timeout: Duration) -> Result<()> {
        let observed = self.session_with_timeout(id, timeout)?;
        observed.verify(target, app, operation)?;
        if !observed.active()? {
            return Err(horizon_app_provider::Error::DeviceUnverified.into());
        }
        Ok(())
    }
    fn confirmed_closed(&self, reference: &str) -> Result<bool> {
        Ok(!self
            .session_with_timeout(reference, Duration::from_secs(15))?
            .active()?)
    }
    fn upload(&self, artifact: &mut Artifact, operation: Uuid, timeout: Duration) -> Result<UploadedApp> {
        Ok(self.upload_with_timeout(artifact, operation, timeout)?)
    }
    fn delete(&self, app: &UploadedApp) -> Result<()> {
        Ok(self.delete_app(app)?)
    }
}
