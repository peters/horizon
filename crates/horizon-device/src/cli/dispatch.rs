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
pub(super) struct Response {
    pub value: Value,
    _lock: Option<File>,
    observation: Option<PathBuf>,
}
impl Response {
    pub(super) fn new(value: Value, lock: Option<File>, observation: Option<PathBuf>) -> Self {
        Self {
            value,
            _lock: lock,
            observation,
        }
    }
    pub(super) fn complete_observation(&mut self) -> crate::Result<()> {
        if let Some(path) = &self.observation {
            remove_file(path)?;
        }
        self.observation = None;
        Ok(())
    }
}
impl Dispatcher {
    pub fn call(&self, command: Command) -> Response {
        let result = self.lock().and_then(|lock| {
            let observation = matches!(command, Command::Screenshot(_)).then(|| self.marker("resize-observe"));
            let value = self.execute(command)?;
            Ok(Response::new(
                json!({"ok":true,"result":value}),
                Some(lock),
                observation,
            ))
        });
        result.unwrap_or_else(|error| {
            Response::new(
                json!({"ok":false,"error":{"code":error.code(),"message":error.to_string(),"resize_uncertain":error.resize_uncertain()}}),
                None,
                None,
            )
        })
    }
    fn execute(&self, command: Command) -> crate::Result<Value> {
        let pending = self.marker("resize-pending").symlink_metadata().is_ok();
        if pending && matches!(command, Command::Resize(_)) {
            return Err(DeviceError::ResizeUncertain(
                "previous resize requires owner reconciliation".into(),
            ));
        }
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
        let mut device = Device::connect(&target).map_err(|error| {
            if pending {
                DeviceError::ResizeUncertain(error.to_string())
            } else {
                error
            }
        })?;
        device.resize.uncertain = pending;
        if device.backend.supports_resize_revisions()
            && matches!(command, Command::Doctor | Command::Resize(_))
            && target.desktop_resize.vnc_address.is_some()
            && let Some(factory) = self.resize_factory
        {
            let backend = factory(&target).map_err(|error| {
                if pending {
                    DeviceError::ResizeUncertain(error.to_string())
                } else {
                    error
                }
            })?;
            device = device.with_resize_backend(backend);
        }
        match command {
            Command::Doctor => {
                let readiness = device.doctor().map_err(|error| {
                    if pending {
                        DeviceError::ResizeUncertain(error.to_string())
                    } else {
                        error
                    }
                })?;
                serde_json::to_value(readiness).map_err(io_error)
            }
            Command::Screenshot(options) => {
                let observation = device.screenshot_with(&options)?;
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
        let mut path = self.target_file.as_os_str().to_os_string();
        path.push(format!(".{extension}"));
        path.into()
    }

    fn remove_marker(&self, extension: &str) -> crate::Result<()> {
        remove_file(&self.marker(extension))
    }

    fn prepare_pending(&self, initialize: impl FnOnce(&File) -> crate::Result<()>) -> crate::Result<()> {
        let options = marker_options();
        let file = options.open(self.marker("resize-pending")).map_err(|error| {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                DeviceError::ResizeUncertain("previous resize requires owner reconciliation".into())
            } else {
                io_error(error)
            }
        })?;
        let result = initialize(&file);
        drop(file);
        if result.is_err() {
            self.remove_marker("resize-pending")?;
        }
        result
    }

    fn resize(&self, device: &mut Device, request: &crate::ResizeRequest) -> crate::Result<Value> {
        // Persist uncertainty before dispatch, including process termination.
        // Only the owner may remove an unresolved journal after reconciliation.
        self.prepare_pending(|file| {
            serde_json::to_writer(file, request).map_err(io_error)?;
            file.sync_all().map_err(io_error)
        })?;
        let options = marker_options();
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
        if self
            .target_file
            .extension()
            .is_some_and(|extension| matches!(extension.to_str(), Some("lock" | "resize-pending" | "resize-observe")))
        {
            return Err(DeviceError::Invalid(
                "target filename uses a reserved control suffix".into(),
            ));
        }
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
fn marker_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
}
fn remove_file(path: &std::path::Path) -> crate::Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(io_error(error)),
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
    #[test]
    fn failed_journal_initialization_removes_only_the_new_journal() -> crate::Result<()> {
        let directory = tempfile::tempdir().map_err(io_error)?;
        let dispatcher = Dispatcher {
            target_file: directory.path().join("target.json"),
            resize_factory: None,
        };
        let failed = dispatcher.prepare_pending(|mut file| {
            use std::io::Write;
            file.write_all(b"partial").map_err(io_error)?;
            Err(DeviceError::Unavailable("sync failed".into()))
        });
        assert!(failed.is_err());
        assert!(!dispatcher.marker("resize-pending").exists());
        std::fs::write(dispatcher.marker("resize-pending"), b"original").map_err(io_error)?;
        assert!(matches!(
            dispatcher.prepare_pending(|_| Ok(())),
            Err(DeviceError::ResizeUncertain(_))
        ));
        assert_eq!(
            std::fs::read(dispatcher.marker("resize-pending")).map_err(io_error)?,
            b"original"
        );
        Ok(())
    }

    #[test]
    fn pending_resize_is_reported_before_missing_target_or_transport() -> crate::Result<()> {
        let directory = tempfile::tempdir().map_err(io_error)?;
        let dispatcher = Dispatcher {
            target_file: directory.path().join("target.json"),
            resize_factory: None,
        };
        std::fs::write(dispatcher.marker("resize-pending"), b"pending").map_err(io_error)?;
        let response = dispatcher.call(Command::Resize(crate::ResizeRequest {
            width: 1920,
            height: 1080,
        }));
        assert_eq!(response.value["error"]["code"], "resize_uncertain");
        assert_eq!(response.value["error"]["resize_uncertain"], true);
        std::fs::write(
            &dispatcher.target_file,
            br#"{"id":"fixture","endpoint":{"kind":"local_x11","display":":54321"}}"#,
        )
        .map_err(io_error)?;
        let response = dispatcher.call(Command::Doctor);
        assert_eq!(response.value["error"]["code"], "resize_uncertain");
        Ok(())
    }
    #[test]
    #[ignore = "requires HORIZON_DEVICE_TEST_TARGET pointing to an owned X11 desktop with RandR"]
    fn pending_doctor_reports_capability_or_preserves_transport_uncertainty()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        fn supported(_: &Target) -> crate::Result<Box<dyn crate::ResizeBackend>> {
            fixture(true, Outcome::Confirm)
                .0
                .resize
                .backend
                .ok_or_else(|| DeviceError::Unavailable("missing fixture".into()))
        }
        fn unavailable(_: &Target) -> crate::Result<Box<dyn crate::ResizeBackend>> {
            Err(DeviceError::Unavailable("disconnected".into()))
        }
        let directory = tempfile::tempdir()?;
        let mut target: Target = serde_json::from_slice(&std::fs::read(std::env::var("HORIZON_DEVICE_TEST_TARGET")?)?)?;
        target.desktop_resize.policy.enabled = true;
        target.desktop_resize.vnc_address = Some("127.0.0.1:1".parse()?);
        let mut dispatcher = Dispatcher {
            target_file: directory.path().join("target.json"),
            resize_factory: Some(supported),
        };
        std::fs::write(&dispatcher.target_file, serde_json::to_vec(&target)?)?;
        std::fs::write(dispatcher.marker("resize-pending"), b"pending")?;
        let response = dispatcher.call(Command::Doctor);
        assert_eq!(response.value["result"]["desktop_resize"]["supported"], true);
        assert_eq!(response.value["result"]["desktop_resize"]["uncertain"], true);
        drop(response);
        dispatcher.resize_factory = Some(unavailable);
        let response = dispatcher.call(Command::Doctor);
        assert_eq!(response.value["error"]["code"], "resize_uncertain");
        Ok(())
    }
    #[test]
    fn target_names_never_alias_or_get_deleted_as_markers() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        for name in [
            "session",
            "session.json",
            "session.yaml",
            "session.resize-observe",
            "session.resize-pending",
            "session.lock",
        ] {
            let dispatcher = Dispatcher {
                target_file: directory.path().join(name),
                resize_factory: None,
            };
            std::fs::write(&dispatcher.target_file, b"original")?;
            assert_eq!(
                dispatcher.marker("resize-observe"),
                directory.path().join(format!("{name}.resize-observe"))
            );
            dispatcher.remove_marker("resize-observe")?;
            assert_eq!(std::fs::read(&dispatcher.target_file)?, b"original");
            if name.ends_with(".lock") || name.contains(".resize-") {
                assert!(matches!(dispatcher.lock(), Err(DeviceError::Invalid(_))));
            }
        }
        Ok(())
    }
}
