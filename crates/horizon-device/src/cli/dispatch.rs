use crate::{ActRequest, CaptureOptions, Device, DeviceError, Target};
use serde_json::{Value, json};
use std::{
    fs::{File, OpenOptions},
    io::Read,
    path::PathBuf,
};

pub enum Command {
    Doctor,
    Screenshot(CaptureOptions),
    Act(ActRequest),
    Resize(crate::ResizeRequest),
}

#[derive(Clone)]
pub struct Dispatcher {
    pub target_file: PathBuf,
    pub resize_factory: Option<super::ResizeFactory>,
}
impl Dispatcher {
    pub fn call(&self, command: Command) -> Value {
        match self.execute(command) {
            Ok(value) => json!({"ok":true,"result":value}),
            Err(error) => json!({"ok":false,"error":{"code":error.code(),"message":error.to_string()}}),
        }
    }
    fn execute(&self, command: Command) -> crate::Result<Value> {
        let _lock = self.lock()?;
        let mut bytes = Vec::new();
        File::open(&self.target_file)
            .map_err(io_error)?
            .take(4097)
            .read_to_end(&mut bytes)
            .map_err(io_error)?;
        if bytes.len() > 4096 {
            return Err(DeviceError::Invalid("target config too large".into()));
        }
        let target: Target =
            serde_json::from_slice(&bytes).map_err(|_| DeviceError::Invalid("invalid target config".into()))?;
        let mut device = Device::connect(&target)?;
        if matches!(command, Command::Doctor | Command::Resize(_))
            && target.desktop_resize.vnc_address.is_some()
            && let Some(factory) = self.resize_factory
        {
            device = device.with_resize_backend(factory(&target)?);
        }
        match command {
            Command::Doctor => {
                let mut readiness = device.doctor()?;
                readiness.desktop_resize.uncertain |= self.marker("resize-pending").exists();
                serde_json::to_value(readiness).map_err(io_error)
            }
            Command::Screenshot(options) => {
                let observation = device.screenshot_with(&options)?;
                self.remove_marker("resize-observe")?;
                serde_json::to_value(observation).map_err(io_error)
            }
            Command::Act(request) => {
                if self.marker("resize-observe").exists() {
                    return Err(DeviceError::StaleGeometry);
                }
                serde_json::to_value(device.act(&request)?).map_err(io_error)
            }
            Command::Resize(request) => self.resize(&mut device, &request),
        }
    }

    fn marker(&self, extension: &str) -> PathBuf {
        self.target_file.with_extension(extension)
    }

    fn remove_marker(&self, extension: &str) -> crate::Result<()> {
        match std::fs::remove_file(self.marker(extension)) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(io_error(error)),
        }
    }

    fn resize(&self, device: &mut Device, request: &crate::ResizeRequest) -> crate::Result<Value> {
        // Persist uncertainty before dispatch, including process termination.
        // Only the owner may remove an unresolved journal after reconciliation.
        let pending = self.marker("resize-pending");
        let mut options = OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(&pending).map_err(|error| {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                DeviceError::ResizeUncertain("previous resize requires owner reconciliation".into())
            } else {
                io_error(error)
            }
        })?;
        serde_json::to_writer(&file, request).map_err(io_error)?;
        file.sync_all().map_err(io_error)?;
        // A fresh process must also refuse input until a post-resize screenshot.
        let observed = self.marker("resize-observe");
        let observation_was_required = observed.symlink_metadata().is_ok();
        let observe_result = options
            .open(&observed)
            .and_then(|file| file.sync_all())
            .or_else(|error| {
                if error.kind() == std::io::ErrorKind::AlreadyExists
                    && observed.symlink_metadata().is_ok_and(|metadata| metadata.is_file())
                {
                    Ok(())
                } else {
                    Err(error)
                }
            });
        if let Err(error) = observe_result {
            self.remove_marker("resize-pending")?;
            return Err(io_error(error));
        }
        let result = device.resize_desktop(request);
        if !result.as_ref().is_err_and(DeviceError::resize_uncertain) {
            if result.is_err() && !observation_was_required {
                self.remove_marker("resize-observe")?;
            }
            self.remove_marker("resize-pending")
                .map_err(|error| DeviceError::ResizeUncertain(error.to_string()))?;
        }
        serde_json::to_value(result?).map_err(io_error)
    }

    fn lock(&self) -> crate::Result<File> {
        // The supplied config lives in a caller-owned private session directory.
        // Keep the inode after unlock so independent CLI/MCP processes cooperate.
        let path = self.target_file.with_extension("lock");
        if path.symlink_metadata().is_ok_and(|m| !m.is_file()) {
            return Err(DeviceError::Invalid("lock must be a regular file".into()));
        }
        let mut options = OpenOptions::new();
        options.create(true).read(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(path).map_err(io_error)?;
        file.try_lock().map_err(|error| match error {
            std::fs::TryLockError::WouldBlock => {
                DeviceError::Unavailable("device busy; another command is active".into())
            }
            std::fs::TryLockError::Error(error) => io_error(error),
        })?;
        Ok(file)
    }
}
fn io_error(e: impl std::fmt::Display) -> DeviceError {
    DeviceError::Unavailable(e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resize::tests::{Outcome, fixture};

    #[test]
    fn a_rejected_second_resize_preserves_the_first_observation_gate()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let dispatcher = Dispatcher {
            target_file: directory.path().join("target.json"),
            resize_factory: None,
        };
        let request = crate::ResizeRequest {
            width: 1920,
            height: 1080,
        };
        let (mut device, _) = fixture(true, Outcome::Confirm);
        dispatcher.resize(&mut device, &request)?;
        let (mut rejected, _) = fixture(true, Outcome::Denied);
        assert!(dispatcher.resize(&mut rejected, &request).is_err());
        assert!(dispatcher.marker("resize-observe").is_file());
        assert!(!dispatcher.marker("resize-pending").exists());

        Ok(())
    }

    #[test]
    fn journals_survive_uncertain_results_and_block_a_fresh_device()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let dispatcher = Dispatcher {
            target_file: directory.path().join("target.json"),
            resize_factory: None,
        };
        let (mut device, _) = fixture(true, Outcome::Timeout(true));
        let request = crate::ResizeRequest {
            width: 1920,
            height: 1080,
        };
        assert!(matches!(
            dispatcher.resize(&mut device, &request),
            Err(DeviceError::ResizeTimeout { uncertain: true })
        ));
        assert!(dispatcher.marker("resize-pending").is_file());
        assert!(dispatcher.marker("resize-observe").is_file());
        let (mut fresh, _) = fixture(true, Outcome::Confirm);
        assert!(matches!(
            dispatcher.resize(&mut fresh, &request),
            Err(DeviceError::ResizeUncertain(_))
        ));

        Ok(())
    }

    #[test]
    fn confirmed_resize_retains_observation_gate_and_denial_clears_journals()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        for outcome in [Outcome::Confirm, Outcome::Denied] {
            let directory = tempfile::tempdir()?;
            let dispatcher = Dispatcher {
                target_file: directory.path().join("target.json"),
                resize_factory: None,
            };
            let (mut device, _) = fixture(true, outcome);
            let result = dispatcher.resize(
                &mut device,
                &crate::ResizeRequest {
                    width: 1920,
                    height: 1080,
                },
            );
            assert!(!dispatcher.marker("resize-pending").exists());
            assert_eq!(dispatcher.marker("resize-observe").exists(), result.is_ok());
        }

        Ok(())
    }
}
