//! Capture provider startup identity before sshd clears the login environment.
use super::store::{anchor_directory, invalid, open_directory, regular};
use horizon_cloud_protocol::bootstrap::Startup;
use rustix::fs::{Mode, OFlags, mkdirat, openat};
use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    io::{self, Read, Write},
    os::unix::fs::MetadataExt,
    path::Path,
};

pub(super) const RUN_ROOT: &str = "/run/sshd/horizon-allocation";
const CAPTURE: &str = "runtime.json";
const LIMIT: u64 = 16 * 1024;

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub(super) enum Source {
    #[default]
    LegacyEnvironment,
    StartupCapture,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Runtime {
    pub version: u32,
    pub startup: Startup,
    pub worker_id: String,
    pub volume_id: String,
    pub data_center_id: String,
    pub worker_operation: String,
    #[serde(skip)]
    pub source: Source,
}

impl Runtime {
    fn environment() -> io::Result<Self> {
        let get = |name| std::env::var(name).map_err(|_| invalid());
        let metadata = get("HORIZON_WORKER_STARTUP")?;
        if metadata.len() > 8192 {
            return Err(invalid());
        }
        let runtime = Self {
            version: 1,
            startup: serde_json::from_str(&metadata).map_err(|_| invalid())?,
            worker_id: get("RUNPOD_POD_ID")?,
            volume_id: get("RUNPOD_VOLUME_ID")?,
            data_center_id: get("RUNPOD_DC_ID")?,
            worker_operation: get("HORIZON_CLOUD_OPERATION")?,
            source: Source::LegacyEnvironment,
        };
        runtime.validate()?;
        Ok(runtime)
    }

    pub fn validate_binding(&self) -> io::Result<()> {
        self.startup.validate().map_err(|_| invalid())?;
        if self.version != 1
            || !horizon_cloud::valid_id(&self.worker_id)
            || self.volume_id != self.startup.volume_id
            || self.data_center_id != self.startup.data_center_id
            || self.worker_operation != self.startup.worker_operation
        {
            return Err(invalid());
        }
        Ok(())
    }

    pub fn validate(&self) -> io::Result<()> {
        self.validate_binding()?;
        let mounts = std::fs::read_to_string("/proc/self/mountinfo")?;
        if !mounts
            .lines()
            .any(|line| line.split_whitespace().nth(4) == Some("/workspace"))
        {
            return Err(invalid());
        }
        Ok(())
    }

    /// A missing capture can never select legacy behavior for a v2 marker.
    pub fn load(bootstrap_version: u32) -> io::Result<Self> {
        match bootstrap_version {
            1 => Self::environment(),
            2 => Self::captured(),
            _ => Err(invalid()),
        }
    }

    pub fn captured() -> io::Result<Self> {
        let directory = open_directory(Path::new(RUN_ROOT))?;
        let mut runtime = read(&directory)?;
        runtime.validate()?;
        runtime.source = Source::StartupCapture;
        Ok(runtime)
    }

    /// Startup-only entry point, before SSH or any workspace writes.
    pub fn capture() -> io::Result<Self> {
        let runtime = Self::environment()?;
        let parent = anchor_directory(Path::new("/run/sshd"))?;
        let metadata = parent.metadata()?;
        if metadata.uid() != rustix::process::geteuid().as_raw() || metadata.mode() & 0o022 != 0 {
            return Err(invalid());
        }
        match mkdirat(&parent, "horizon-allocation", Mode::RUSR | Mode::WUSR | Mode::XUSR) {
            Ok(()) | Err(rustix::io::Errno::EXIST) => {}
            Err(error) => return Err(error.into()),
        }
        let directory = open_directory(Path::new(RUN_ROOT))?;
        match openat(
            &directory,
            CAPTURE,
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::RUSR | Mode::WUSR,
        ) {
            Ok(fd) => {
                let bytes = serde_json::to_vec(&runtime)?;
                if bytes.len() as u64 > LIMIT {
                    return Err(invalid());
                }
                let mut file = File::from(fd);
                file.write_all(&bytes)?;
                file.sync_all()?;
                directory.sync_all()?;
                parent.sync_all()?;
            }
            Err(rustix::io::Errno::EXIST) => {}
            Err(error) => return Err(error.into()),
        }
        if read(&directory)? != runtime {
            return Err(invalid());
        }
        regular(&directory, CAPTURE)?.sync_all()?;
        directory.sync_all()?;
        let mut expected = runtime;
        expected.source = Source::StartupCapture;
        let captured = Self::captured()?;
        if captured != expected {
            return Err(invalid());
        }
        Ok(captured)
    }
}

fn read(directory: &File) -> io::Result<Runtime> {
    let file = regular(directory, CAPTURE)?;
    let mut bytes = Vec::new();
    file.take(LIMIT + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > LIMIT {
        return Err(invalid());
    }
    serde_json::from_slice(&bytes).map_err(|_| invalid())
}
