//! Shared setup input for the Cloud form and command-line clients.
use super::{Auth, Binding, Error, Result};
use std::path::PathBuf;
use zeroize::Zeroizing;

/// Secret input cannot be serialized or printed.
#[derive(Clone, Default)]
pub struct Draft {
    pub repository: String,
    pub publish_username: String,
    pub publish_secret: Zeroizing<String>,
    pub publish_expiry: String,
    pub pull_username: String,
    pub pull_secret: Zeroizing<String>,
    pub pull_expiry: String,
    pub read_only_confirmed: bool,
    pub original: Option<Binding>,
    pub validation_image: String,
}

impl Draft {
    #[must_use]
    pub fn is_saved(&self) -> bool {
        self.original.as_ref().is_some_and(|original| {
            self.repository == original.repository
                && self.pull_username == original.pull.username
                && self.pull_expiry == original.pull.expires_at.as_deref().unwrap_or_default()
                && self.publish_username == original.publish.as_ref().map_or("", |auth| auth.username.as_str())
                && self.publish_expiry
                    == original
                        .publish
                        .as_ref()
                        .and_then(|auth| auth.expires_at.as_deref())
                        .unwrap_or_default()
                && self.read_only_confirmed == original.read_only_confirmed
                && self.pull_secret.is_empty()
                && self.publish_secret.is_empty()
        })
    }
    #[must_use]
    pub fn from_binding(binding: &Binding) -> Self {
        Self {
            repository: binding.repository.clone(),
            publish_username: binding
                .publish
                .as_ref()
                .map_or_else(String::new, |auth| auth.username.clone()),
            publish_expiry: binding
                .publish
                .as_ref()
                .and_then(|auth| auth.expires_at.clone())
                .unwrap_or_default(),
            pull_username: binding.pull.username.clone(),
            pull_expiry: binding.pull.expires_at.clone().unwrap_or_default(),
            read_only_confirmed: binding.read_only_confirmed,
            original: Some(binding.clone()),
            ..Self::default()
        }
    }

    /// # Errors
    /// Rejects incomplete setup without changing local files or provider bindings.
    pub fn validate(&self) -> Result<()> {
        if super::repository(&self.repository)? != self.repository {
            return Err(Error::Invalid("Registry scope must omit tags and digests"));
        }
        if self
            .original
            .as_ref()
            .is_some_and(|original| original.repository != self.repository)
        {
            return Err(Error::Invalid("Add a separate binding to change the repository scope"));
        }
        if !self.read_only_confirmed {
            return Err(Error::Invalid(
                "Confirm a dedicated repository-scoped read-only pull grant",
            ));
        }
        if self.pull_username.chars().count() > horizon_cloud::runpod::registry::MAX_USERNAME_LENGTH {
            return Err(Error::Invalid("Registry pull username exceeds the provider limit"));
        }
        validate_input(
            &self.pull_username,
            &self.pull_secret,
            &self.pull_expiry,
            self.original.as_ref().map(|binding| &binding.pull),
        )?;
        if !self.publish_username.is_empty() {
            super::validate_publish_username(&self.publish_username)?;
            validate_input(
                &self.publish_username,
                &self.publish_secret,
                &self.publish_expiry,
                self.original.as_ref().and_then(|binding| binding.publish.as_ref()),
            )?;
        } else if !self.publish_secret.is_empty() {
            return Err(Error::Invalid("Publishing credential needs a username"));
        }
        if !self.publish_username.is_empty()
            && secret_fingerprint(&self.pull_secret, self.original.as_ref().map(|binding| &binding.pull))?
                == secret_fingerprint(
                    &self.publish_secret,
                    self.original.as_ref().and_then(|binding| binding.publish.as_ref()),
                )?
        {
            return Err(Error::Invalid("Use different push and pull credentials"));
        }
        Ok(())
    }

    /// # Errors
    /// Writes immutable private files through the caller's settings transaction.
    /// Rotating pull material creates a fresh generation and retains old revocation handles.
    pub fn save(&self, mut secret: impl FnMut(&str, &str) -> Result<PathBuf>) -> Result<Binding> {
        self.validate()?;
        let pull = save_auth(
            &self.pull_username,
            &self.pull_secret,
            &self.pull_expiry,
            self.original.as_ref().map(|binding| &binding.pull),
            "registry-pull",
            &mut secret,
        )?;
        let publish = if self.publish_username.is_empty() {
            None
        } else {
            Some(save_auth(
                &self.publish_username,
                &self.publish_secret,
                &self.publish_expiry,
                self.original.as_ref().and_then(|binding| binding.publish.as_ref()),
                "registry-push",
                &mut secret,
            )?)
        };
        let rotate = self.original.as_ref().is_none_or(|original| {
            original.pull.username != pull.username
                || original.pull.expires_at != pull.expires_at
                || !self.pull_secret.is_empty()
        });
        let mut retired = self
            .original
            .as_ref()
            .map_or_else(Vec::new, |original| original.retired.clone());
        if rotate && let Some(original) = &self.original {
            retired.push(original.generation.clone());
        }
        let generation = if rotate {
            super::super::new_id()
        } else {
            self.original
                .as_ref()
                .ok_or(Error::Invalid("Registry setup lost its original binding"))?
                .generation
                .clone()
        };
        Ok(Binding {
            repository: self.repository.clone(),
            publish,
            pull,
            read_only_confirmed: true,
            generation,
            retired,
        })
    }
}

fn validate_input(username: &str, value: &str, expiry: &str, saved: Option<&Auth>) -> Result<()> {
    if username.is_empty()
        || username
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || c == ':')
    {
        return Err(Error::Invalid("Enter a valid registry username"));
    }
    if !expiry.is_empty() {
        super::parse_expiry(expiry)?;
    }
    if value.is_empty() {
        let saved = saved.ok_or(Error::Invalid("Enter a registry credential"))?;
        super::super::settings::validate_private_key_file(&saved.secret_file)?;
    } else if value.len() > 4096 || value.bytes().any(|byte| byte <= 32 || byte >= 127) {
        return Err(Error::Invalid("Enter a nonempty registry credential on one line"));
    }
    Ok(())
}

fn save_auth(
    username: &str,
    value: &str,
    expiry: &str,
    saved: Option<&Auth>,
    name: &str,
    secret: &mut impl FnMut(&str, &str) -> Result<PathBuf>,
) -> Result<Auth> {
    let secret_file = if value.is_empty() {
        saved
            .ok_or(Error::Invalid("No saved registry credential"))?
            .secret_file
            .clone()
    } else {
        secret(name, value)?
    };
    Ok(Auth {
        username: username.into(),
        secret_file,
        expires_at: (!expiry.is_empty()).then(|| expiry.into()),
    })
}

fn secret_fingerprint(value: &str, saved: Option<&Auth>) -> Result<String> {
    let value = if value.is_empty() {
        let saved = saved.ok_or(Error::Invalid("No saved registry credential"))?;
        super::super::settings::validate_private_key_file(&saved.secret_file)?;
        Zeroizing::new(std::fs::read_to_string(&saved.secret_file)?)
    } else {
        Zeroizing::new(value.into())
    };
    Ok(super::credentials::fingerprint(value.trim().as_bytes()))
}
