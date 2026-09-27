//! Explicit repository-bound Git authentication, kept outside portable provider code.
use super::{Error, Result, command::Runner, settings::validate_private_key_file, ssh::Connection};
use serde::{Deserialize, Serialize};
use std::{
    io::Write,
    path::{Path, PathBuf},
};

mod grants;
pub use grants::{GRANTS_CONTRACT, Selected, Sibling, Target, select};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    pub local_repository: PathBuf,
    pub repository: String,
    pub token_file: PathBuf,
    pub author_name: String,
    pub author_email: String,
}

impl Binding {
    /// # Errors
    /// Rejects relative bindings, malformed repository names and control characters.
    pub fn validate(&self) -> Result<()> {
        if !self.local_repository.is_absolute() || !self.token_file.is_absolute() {
            return Err(Error::Invalid(
                "Git credential bindings require absolute machine-local paths",
            ));
        }
        validate_repository(&self.repository)?;
        validate_identity(&self.author_name, &self.author_email)
    }
}

fn validate_repository(repository: &str) -> Result<()> {
    let parts: Vec<_> = repository.split('/').collect();
    if parts.len() != 2
        || parts.iter().any(|part| {
            part.is_empty()
                || part.len() > 100
                || *part == "."
                || *part == ".."
                || !part.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
        })
    {
        return Err(Error::Invalid("Git credential repository must be an owner/name pair"));
    }
    Ok(())
}

fn validate_identity(name: &str, email: &str) -> Result<()> {
    if [name, email]
        .iter()
        .any(|s| s.is_empty() || s.len() > 200 || s.chars().any(char::is_control))
    {
        return Err(Error::Invalid("Invalid Git author identity"));
    }
    Ok(())
}

fn validate_token(token: &str) -> Result<()> {
    if token.is_empty() || token.len() > 2048 || !token.bytes().all(|b| b.is_ascii_graphic()) {
        return Err(Error::Invalid("Invalid Git credential value"));
    }
    Ok(())
}

/// Reads and checks one binding's private token file.
fn read_token(binding: &Binding) -> Result<zeroize::Zeroizing<String>> {
    validate_private_key_file(&binding.token_file)?;
    let token = zeroize::Zeroizing::new(std::fs::read_to_string(&binding.token_file)?);
    validate_token(token.trim())?;
    Ok(zeroize::Zeroizing::new(token.trim().to_owned()))
}

/// The one binding whose checkout is `repository`; two matches are ambiguous.
fn matching<'a>(bindings: &'a [Binding], repository: &Path) -> Result<Option<&'a Binding>> {
    let mut matched = None;
    for binding in bindings {
        // An unrelated missing checkout does not block this deployment.
        if binding.local_repository.canonicalize().ok().as_deref() == Some(repository)
            && matched.replace(binding).is_some()
        {
            return Err(Error::Invalid("Multiple Git credential bindings match this repository"));
        }
    }
    Ok(matched)
}

#[derive(Serialize)]
struct Payload<'a> {
    repository: &'a str,
    token: &'a str,
    author_name: &'a str,
    author_email: &'a str,
}

/// A private temporary stdin payload; it is never part of deployment persistence.
pub struct Prepared(tempfile::NamedTempFile);

impl Prepared {
    /// # Errors
    /// Fails before image preparation/allocation for ambiguous or unsafe bindings.
    pub fn for_repository(bindings: &[Binding], repository: &Path) -> Result<Option<Self>> {
        let repository = repository.canonicalize()?;
        for binding in bindings {
            binding.validate()?;
        }
        matching(bindings, &repository)?.map(Self::new).transpose()
    }

    fn new(binding: &Binding) -> Result<Self> {
        let token = read_token(binding)?;
        Self::private(&Payload {
            repository: &binding.repository,
            token: &token,
            author_name: &binding.author_name,
            author_email: &binding.author_email,
        })
    }

    fn private(payload: &impl Serialize) -> Result<Self> {
        let mut file = tempfile::NamedTempFile::new()?;
        serde_json::to_writer(&mut file, payload).map_err(|_| Error::Json)?;
        file.flush()?;
        Ok(Self(file))
    }

    /// # Errors
    /// Uses encrypted stdin with both output streams suppressed; no token enters argv.
    pub fn install(&self, connection: &Connection, runner: &Runner<'_>) -> Result<()> {
        self.transfer(&mut connection.command("horizon-worker-git-auth install"), runner)
    }

    /// Up to 16 grants exceed the 4 KiB single-credential file bound, so the payload uses the
    /// bounded structured path, which keeps the private-mode check and suppresses both streams.
    fn transfer(&self, command: &mut std::process::Command, runner: &Runner<'_>) -> Result<()> {
        runner.private_payload(command, self.0.path())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn binding(root: &Path, token_file: PathBuf) -> Binding {
        Binding {
            local_repository: root.into(),
            repository: "example/project".into(),
            token_file,
            author_name: "Test User".into(),
            author_email: "test@example.invalid".into(),
        }
    }

    #[test]
    fn transfer_is_opt_in_and_only_matches_the_explicit_local_repository() {
        let selected = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let mut key = tempfile::NamedTempFile::new().unwrap();
        key.write_all(b"synthetic-git-token").unwrap();
        let binding = binding(selected.path(), key.path().into());
        assert!(Prepared::for_repository(&[], selected.path()).unwrap().is_none());
        assert!(
            Prepared::for_repository(std::slice::from_ref(&binding), other.path())
                .unwrap()
                .is_none()
        );
        let prepared = Prepared::for_repository(std::slice::from_ref(&binding), selected.path())
            .unwrap()
            .unwrap();
        let value: serde_json::Value = serde_json::from_reader(prepared.0.reopen().unwrap()).unwrap();
        assert_eq!(value["repository"], "example/project");
        assert_eq!(value["token"], "synthetic-git-token");
        assert!(!format!("{binding:?}").contains("synthetic-git-token"));
        assert!(Prepared::for_repository(&[binding.clone(), binding], selected.path()).is_err());
    }

    #[test]
    fn unsafe_metadata_and_private_token_fail_before_transfer() {
        let root = tempfile::tempdir().unwrap();
        let mut key = tempfile::NamedTempFile::new().unwrap();
        key.write_all(b"private\ninjected-value").unwrap();
        let mut value = binding(root.path(), key.path().into());
        let error = Prepared::new(&value).err().unwrap().to_string();
        assert_eq!(error, "Invalid Git credential value");
        for repository in [
            "example/project\n",
            "../project",
            "https://github.com/example/project",
            "example/a/b",
        ] {
            value.repository = repository.into();
            assert!(value.validate().is_err());
        }
        value.repository = "example/project".into();
        value.author_email = "test@example.invalid\nhelper=bad".into();
        assert!(value.validate().is_err());
    }
}
