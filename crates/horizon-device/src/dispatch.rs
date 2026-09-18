use horizon_device::{ActRequest, Device, DeviceError, Target};
use serde_json::{Value, json};
use std::{
    fs::{File, OpenOptions},
    io::Read,
    path::PathBuf,
};

#[derive(Clone)]
pub struct Dispatcher {
    pub target_file: PathBuf,
}
impl Dispatcher {
    pub fn call(&self, command: &str, request: Option<ActRequest>) -> Value {
        match self.execute(command, request) {
            Ok(value) => json!({"ok":true,"result":value}),
            Err(error) => json!({"ok":false,"error":{"code":error.code(),"message":error.to_string()}}),
        }
    }
    fn execute(&self, command: &str, request: Option<ActRequest>) -> horizon_device::Result<Value> {
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
        match command {
            "doctor" => serde_json::to_value(device.doctor()?).map_err(io_error),
            "screenshot" => serde_json::to_value(device.screenshot()?).map_err(io_error),
            "act" => serde_json::to_value(
                device.act(&request.ok_or_else(|| DeviceError::Invalid("missing action".into()))?)?,
            )
            .map_err(io_error),
            _ => Err(DeviceError::Invalid("unknown command".into())),
        }
    }
    fn lock(&self) -> horizon_device::Result<File> {
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
