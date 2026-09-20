//! Optional host-visible attribution while the existing target lock is held.
use std::{
    io::Write,
    path::{Path, PathBuf},
};
pub(super) struct ControlOwner {
    path: PathBuf,
    actor: String,
    previous: Option<Vec<u8>>,
    completed: bool,
}
impl ControlOwner {
    pub(super) fn begin(target: &Path) -> crate::Result<Option<Self>> {
        let Ok(actor) = std::env::var("HORIZON_DEVICE_ACTOR") else {
            return Ok(None);
        };
        if actor.is_empty() || actor.len() > 128 || actor.chars().any(char::is_control) {
            return Err(crate::DeviceError::Invalid("Invalid device controller identity".into()));
        }
        let mut path = target.as_os_str().to_os_string();
        path.push(".controller.json");
        let path: PathBuf = path.into();
        let previous = match std::fs::read(&path) {
            Ok(value) => Some(value),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(crate::DeviceError::Unavailable(error.to_string())),
        };
        let owner = Self {
            path,
            actor,
            previous,
            completed: false,
        };
        owner
            .save(true)
            .map_err(|e| crate::DeviceError::Unavailable(e.to_string()))?;
        Ok(Some(owner))
    }
    pub(super) fn complete(&mut self) {
        self.completed = true;
    }
    fn save(&self, active: bool) -> std::io::Result<()> {
        let time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let previous = self
            .previous
            .as_deref()
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(bytes).ok());
        let last = if self.completed {
            Some(self.actor.as_str())
        } else {
            previous
                .as_ref()
                .and_then(|value| value["last_actor"].as_str().or_else(|| value["actor"].as_str()))
        };
        let value = serde_json::json!({"actor":self.actor,"last_actor":last,"active":active,"pid":std::process::id(),"updated":time});
        let temporary = self.path.with_extension(format!("{}.tmp", std::process::id()));
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary)?;
        serde_json::to_writer(&mut file, &value)?;
        file.flush()?;
        std::fs::rename(temporary, &self.path)
    }
}
impl Drop for ControlOwner {
    fn drop(&mut self) {
        if self.completed {
            let _ = self.save(false);
        } else if let Some(previous) = &self.previous {
            let _ = std::fs::write(&self.path, previous);
        } else {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}
