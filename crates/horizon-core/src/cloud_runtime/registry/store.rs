use super::{Binding, Config, Error, Result, Settings, State};
use serde::{Deserialize, Serialize};
use std::{
    fs::{File, OpenOptions},
    io::Write,
    path::{Component, Path, PathBuf},
};

pub(in crate::cloud_runtime) fn ensure_outside_source(root: &Path, source: &Path) -> Result<()> {
    let mut resolved = PathBuf::new();
    for component in root.components() {
        if component == Component::ParentDir {
            return Err(Error::Invalid("Registry state path must not contain parent traversal"));
        }
        resolved.push(component.as_os_str());
        match std::fs::symlink_metadata(&resolved) {
            Ok(_) => resolved = resolved.canonicalize()?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        if resolved.starts_with(source) {
            return Err(Error::Invalid(
                "Registry state must stay outside the source and build context",
            ));
        }
    }
    Ok(())
}

#[derive(Deserialize, Serialize)]
struct Record {
    repository: String,
    generation: String,
    account: String,
    fingerprint: Option<String>,
    state: State,
    #[serde(default)]
    validation: Option<super::Validation>,
}

pub(super) struct Journal {
    root: PathBuf,
    path: PathBuf,
    record: Record,
    lock: File,
}

impl Journal {
    pub fn open(config: &Config, binding: &Binding, settings: &Settings, fingerprint: Option<&str>) -> Result<Self> {
        let mut journal = Self::open_generation(config, binding, &binding.generation, settings)?;
        journal.check_fingerprint(config, fingerprint)?;
        Ok(journal)
    }

    pub fn open_host(config: &Config, binding: &Binding, fingerprint: &str) -> Result<Self> {
        let mut journal = Self::open_host_generation(config, binding, &binding.generation)?;
        journal.check_fingerprint(config, Some(fingerprint))?;
        Ok(journal)
    }

    pub fn open_host_generation(config: &Config, binding: &Binding, generation: &str) -> Result<Self> {
        let journal = Self::open_scoped(config, binding, generation, "direct-host", &config.root.join("hosts"))?;
        if !matches!(journal.state(), State::Prepared) {
            return Err(Error::Invalid(
                "A direct host pull journal cannot contain provider registry state",
            ));
        }
        Ok(journal)
    }

    fn check_fingerprint(&mut self, config: &Config, fingerprint: Option<&str>) -> Result<()> {
        if let Some(fingerprint) = fingerprint {
            // Provider and direct-host operations retain separate long-lived locks, but
            // pinning material is one generation-wide transaction across both journals.
            let _identity_lock = if self.record.fingerprint.is_none() {
                let lock = OpenOptions::new()
                    .create(true)
                    .truncate(false)
                    .read(true)
                    .write(true)
                    .open(config.root.join(format!("{}.identity.lock", self.record.generation)))?;
                lock.lock()?;
                Some(lock)
            } else {
                None
            };
            let other_root = if self.root == config.root {
                config.root.join("hosts")
            } else {
                config.root.clone()
            };
            match std::fs::read(other_root.join(format!("{}.json", self.record.generation))) {
                Ok(bytes) => {
                    let other: Record = serde_json::from_slice(&bytes).map_err(|_| Error::Json)?;
                    if other.repository != self.record.repository || other.generation != self.record.generation {
                        return Err(Error::Invalid(
                            "Registry journals disagree about their repository or generation",
                        ));
                    }
                    if other.fingerprint.as_deref().is_some_and(|saved| saved != fingerprint) {
                        return Err(Error::Invalid(
                            "Pull credential changed within a generation; rotate the binding instead",
                        ));
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
            if self
                .record
                .fingerprint
                .as_deref()
                .is_some_and(|saved| saved != fingerprint)
            {
                return Err(Error::Invalid(
                    "Pull credential changed within a generation; rotate the binding instead",
                ));
            }
            if self.record.fingerprint.is_none() {
                self.record.fingerprint = Some(fingerprint.into());
                self.write()?;
            }
        }
        Ok(())
    }

    pub fn open_generation(config: &Config, binding: &Binding, generation: &str, settings: &Settings) -> Result<Self> {
        config.validate()?;
        if generation != binding.generation && !binding.retired.iter().any(|value| value == generation) {
            return Err(Error::Invalid("Unknown registry generation"));
        }
        // RunPod registry bindings are per RunPod account; a Hetzner-only machine has none.
        if !settings.runpod_configured() {
            return Err(Error::Invalid(super::super::settings::RUNPOD_KEY_MISSING));
        }
        super::super::settings::validate_private_key_file(&settings.runpod_key_file)?;
        let key = zeroize::Zeroizing::new(std::fs::read_to_string(&settings.runpod_key_file)?);
        let account = super::credentials::fingerprint(key.trim().as_bytes());
        Self::open_scoped(config, binding, generation, &account, &config.root)
    }

    fn open_scoped(config: &Config, binding: &Binding, generation: &str, account: &str, root: &Path) -> Result<Self> {
        config.validate()?;
        if generation != binding.generation && !binding.retired.iter().any(|value| value == generation) {
            return Err(Error::Invalid("Unknown registry generation"));
        }
        crate::session_store::require_directory_durability()?;
        super::directory::ensure_private_root(&config.root)?;
        super::directory::ensure_private_root(root)?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(root.join(format!("{generation}.lock")))?;
        lock.try_lock().map_err(|_| Error::Busy)?;
        let path = root.join(format!("{generation}.json"));
        let record = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice::<Record>(&bytes).map_err(|_| Error::Json)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Record {
                repository: binding.repository.clone(),
                generation: generation.into(),
                account: account.to_owned(),
                fingerprint: None,
                state: State::Prepared,
                validation: None,
            },
            Err(error) => return Err(error.into()),
        };
        if record.repository != binding.repository || record.generation != generation || record.account != account {
            return Err(Error::Invalid(
                "Registry journal belongs to a different repository, generation or compute credential; reconcile with the original binding",
            ));
        }
        Ok(Self {
            root: root.to_owned(),
            path,
            record,
            lock,
        })
    }

    pub fn state(&self) -> &State {
        &self.record.state
    }

    pub fn validation(&self) -> Option<super::Validation> {
        self.record.validation.clone()
    }

    pub fn record_validation(&mut self, validation: super::Validation) -> Result<()> {
        self.record.validation = Some(validation);
        self.write()
    }

    pub fn save(&mut self, next: &State) -> std::result::Result<(), horizon_cloud::CloudError> {
        let previous = std::mem::replace(&mut self.record.state, next.clone());
        if self.write().is_err() {
            self.record.state = previous;
            return Err(horizon_cloud::CloudError::Persistence);
        }
        Ok(())
    }

    fn write(&self) -> Result<()> {
        let mut file = tempfile::NamedTempFile::new_in(&self.root)?;
        serde_json::to_writer(&mut file, &self.record).map_err(|_| Error::Json)?;
        file.flush()?;
        file.as_file().sync_all()?;
        file.persist(&self.path).map_err(|error| error.error)?;
        #[cfg(unix)]
        File::open(&self.root)?.sync_all()?;
        Ok(())
    }
}

impl Drop for Journal {
    fn drop(&mut self) {
        let _ = self.lock.unlock();
    }
}
