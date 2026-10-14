//! Image destinations selected by a host account, independently of Git hosting.
//! This policy performs no authentication, network requests or publication.
use ring::digest::{SHA256, digest};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// The non-secret account identity that a publication must retain across retries.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Publisher {
    /// The account of the host's package-registry sign-in, not its GitHub App sign-in.
    GitHub { login: String },
    /// The exact machine-local publication grant; an image name alone grants no access.
    Registry { binding: String },
}

/// A resolved repository and the account or grant allowed to publish there.
/// Hosts persist this selection before a build, and never put it in repository YAML.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Destination {
    pub repository: String,
    pub publisher: Publisher,
}

#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum DestinationError {
    #[error("Choose a valid image-publishing account")]
    Account,
    #[error("Choose an explicit registry repository without a tag, digest or credentials")]
    Repository,
    #[error("Restore the exact registry credential binding selected for this destination")]
    Binding,
    #[error("Choose a repository and a valid cloud profile before selecting an image destination")]
    Source,
}

impl Destination {
    /// A personal GHCR destination derived from an authenticated publishing account.
    /// `source` is a non-secret repository identity supplied by the host. It names the
    /// image and separates otherwise identical names; it cannot choose the account.
    /// # Errors
    /// Rejects missing source identity, malformed profile names or account identities.
    pub fn automatic_github(login: &str, source: &str, profile: &str) -> Result<Self, DestinationError> {
        if !valid_login(login) {
            return Err(DestinationError::Account);
        }
        if source.is_empty() || source.len() > 4096 || source.contains(char::is_control) || !crate::valid_id(profile) {
            return Err(DestinationError::Source);
        }
        let fingerprint = digest(&SHA256, format!("{source}\0{profile}").as_bytes());
        let name = source.rsplit('/').next().unwrap_or(source);
        let name = name.strip_suffix(".git").unwrap_or(name);
        let name = component(name, 48);
        let profile = component(profile, 32);
        let suffix: String = fingerprint.as_ref()[..6]
            .iter()
            .flat_map(|byte| {
                const HEX: &[u8; 16] = b"0123456789abcdef";
                [
                    char::from(HEX[usize::from(byte >> 4)]),
                    char::from(HEX[usize::from(byte & 15)]),
                ]
            })
            .collect();
        let login = login.to_ascii_lowercase();
        let destination = Self {
            repository: format!("ghcr.io/{login}/{name}-{profile}-worker-{suffix}"),
            publisher: Publisher::GitHub { login },
        };
        destination.validate()?;
        Ok(destination)
    }

    /// A destination explicitly authorized by one machine-local registry grant.
    /// This accepts any supported registry and does not depend on the source Git host.
    /// # Errors
    /// Rejects malformed repository names and grant identities.
    pub fn bound(repository: String, binding: String) -> Result<Self, DestinationError> {
        let destination = Self {
            repository,
            publisher: Publisher::Registry { binding },
        };
        destination.validate()?;
        Ok(destination)
    }

    /// # Errors
    /// Validates persisted or externally constructed selections before a host uses them.
    pub fn validate(&self) -> Result<(), DestinationError> {
        if !valid_repository(&self.repository) {
            return Err(DestinationError::Repository);
        }
        match &self.publisher {
            Publisher::GitHub { login } => {
                if !valid_login(login) || !self.repository.starts_with("ghcr.io/") {
                    return Err(DestinationError::Account);
                }
            }
            Publisher::Registry { binding } => {
                if !crate::valid_id(binding) {
                    return Err(DestinationError::Binding);
                }
            }
        }
        Ok(())
    }

    /// # Errors
    /// Refuses a different account or credential reference on an existing destination.
    pub fn check_publisher(&self, publisher: &Publisher) -> Result<(), DestinationError> {
        self.validate()?;
        if &self.publisher != publisher {
            return Err(match self.publisher {
                Publisher::GitHub { .. } => DestinationError::Account,
                Publisher::Registry { .. } => DestinationError::Binding,
            });
        }
        Ok(())
    }
}

/// An explicit registry repository, without credentials, tags or digests.
#[must_use]
pub fn valid_repository(repository: &str) -> bool {
    if !crate::valid_image(repository) || repository.len() > 255 {
        return false;
    }
    let Some((host, path)) = repository.split_once('/') else {
        return false;
    };
    let (hostname, port) = host
        .split_once(':')
        .map_or((host, None), |(name, port)| (name, Some(port)));
    let valid_host = (hostname.contains('.') || hostname == "localhost" || port.is_some())
        && hostname.len() <= 253
        && hostname.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
        && port.is_none_or(|port| {
            !port.is_empty()
                && port.bytes().all(|byte| byte.is_ascii_digit())
                && port.parse::<u16>().is_ok_and(|port| port != 0)
        });
    valid_host && path.split('/').all(valid_component)
}

fn valid_component(component: &str) -> bool {
    let mut bytes = component.bytes().peekable();
    let alphanumeric = |byte: &u8| byte.is_ascii_lowercase() || byte.is_ascii_digit();
    if !bytes.peek().is_some_and(alphanumeric) {
        return false;
    }
    loop {
        while bytes.peek().is_some_and(alphanumeric) {
            bytes.next();
        }
        match bytes.next() {
            None => return true,
            Some(b'.') => {}
            Some(b'_') => {
                if bytes.peek() == Some(&b'_') {
                    bytes.next();
                }
            }
            Some(b'-') => {
                while bytes.peek() == Some(&b'-') {
                    bytes.next();
                }
            }
            Some(_) => return false,
        }
        if !bytes.peek().is_some_and(alphanumeric) {
            return false;
        }
    }
}

fn valid_login(login: &str) -> bool {
    !login.is_empty()
        && login.len() <= 39
        && !login.starts_with('-')
        && !login.ends_with('-')
        && !login.contains("--")
        && login.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

fn component(input: &str, limit: usize) -> String {
    let mut component = String::new();
    for byte in input.bytes() {
        if component.len() == limit {
            break;
        }
        if byte.is_ascii_alphanumeric() {
            component.push(char::from(byte.to_ascii_lowercase()));
        } else if !component.is_empty() && !component.ends_with('-') {
            component.push('-');
        }
    }
    let component = component.trim_end_matches('-');
    if component.is_empty() {
        "worker".into()
    } else {
        component.to_owned()
    }
}

#[cfg(test)]
mod tests;
