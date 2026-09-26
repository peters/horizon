//! Per-repository Git grants for a primary repository and its same-worker siblings.
use super::{
    super::{Error, Result},
    Binding, Prepared, matching, read_token, validate_identity, validate_repository, validate_token,
};
use serde::{Deserialize, Serialize};
use std::{collections::HashSet, path::Path};
use zeroize::Zeroize;

/// Printed by `horizon-worker-check --git-auth` when the worker accepts version 2 grants.
pub const GRANTS_CONTRACT: &str = "horizon-git-auth-contract=2";
const MAX_GRANTS: usize = 16;
/// The worker's `MAX_BYTES`; the largest valid payload serializes well below it.
const MAX_PAYLOAD_BYTES: u64 = 128 * 1024;
const SIBLING_PREFIX: &str = "sibling:";

/// The bare repository on the worker that a grant configures.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(try_from = "String", into = "String")]
pub enum Target {
    Primary,
    Sibling(String),
}

impl TryFrom<String> for Target {
    type Error = &'static str;

    fn try_from(value: String) -> std::result::Result<Self, Self::Error> {
        if value == "primary" {
            return Ok(Self::Primary);
        }
        match value.strip_prefix(SIBLING_PREFIX) {
            Some(alias) if horizon_cloud::companions::valid_alias(alias) => Ok(Self::Sibling(alias.to_owned())),
            _ => Err("Invalid Git grant target"),
        }
    }
}

impl From<Target> for String {
    fn from(target: Target) -> Self {
        match target {
            Target::Primary => "primary".to_owned(),
            Target::Sibling(alias) => format!("{SIBLING_PREFIX}{alias}"),
        }
    }
}

/// A sibling checkout deployed next to the primary repository on one worker.
#[derive(Clone, Copy, Debug)]
pub struct Sibling<'a> {
    pub alias: &'a str,
    pub local_repository: &'a Path,
}

/// The binding chosen for one worker repository.
#[derive(Clone, Debug)]
pub struct Selected<'a> {
    pub target: Target,
    pub binding: &'a Binding,
}

/// Chooses at most one binding per repository; repositories without a binding receive no grant.
/// # Errors
/// Rejects any invalid binding, invalid or duplicate aliases, a missing checkout, two bindings
/// for one checkout, one GitHub repository granted to two targets, and more than 16 grants.
pub fn select<'a>(bindings: &'a [Binding], primary: &Path, siblings: &[Sibling<'_>]) -> Result<Vec<Selected<'a>>> {
    for binding in bindings {
        binding.validate()?;
    }
    let mut targets = vec![(Target::Primary, primary.canonicalize()?)];
    for sibling in siblings {
        let target = Target::try_from(format!("{SIBLING_PREFIX}{}", sibling.alias)).map_err(Error::Invalid)?;
        if targets.iter().any(|(existing, _)| existing == &target) {
            return Err(Error::Invalid("Duplicate sibling alias"));
        }
        targets.push((target, sibling.local_repository.canonicalize()?));
    }
    let mut selected = Vec::new();
    let mut repositories = HashSet::new();
    for (target, repository) in targets {
        if let Some(binding) = matching(bindings, &repository)? {
            if !repositories.insert(binding.repository.to_ascii_lowercase()) {
                return Err(Error::Invalid("One Git repository is bound to two worker repositories"));
            }
            selected.push(Selected { target, binding });
        }
    }
    if selected.len() > MAX_GRANTS {
        return Err(Error::Invalid("Too many Git credential grants for one worker"));
    }
    Ok(selected)
}

/// Never printed and wiped on drop.
#[derive(Deserialize, Serialize)]
#[serde(transparent)]
struct Token(String);

impl Drop for Token {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

impl std::fmt::Debug for Token {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("<redacted>")
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Grant {
    repository: String,
    token: Token,
    author_name: String,
    author_email: String,
    target: Target,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(try_from = "u8", into = "u8")]
struct Version;

impl TryFrom<u8> for Version {
    type Error = &'static str;

    fn try_from(value: u8) -> std::result::Result<Self, Self::Error> {
        if value == 2 {
            Ok(Self)
        } else {
            Err("Unsupported Git grant version")
        }
    }
}

impl From<Version> for u8 {
    fn from(_: Version) -> Self {
        2
    }
}

/// Mirrors the worker's version 2 credential file.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct GrantSet {
    version: Version,
    grants: Vec<Grant>,
}

impl GrantSet {
    /// The same limits the worker enforces, checked again before any token leaves this machine.
    fn validate(&self) -> Result<()> {
        if self.grants.is_empty() || self.grants.len() > MAX_GRANTS {
            return Err(Error::Invalid("Too many Git credential grants for one worker"));
        }
        let mut repositories = HashSet::new();
        let mut targets = HashSet::new();
        for grant in &self.grants {
            validate_repository(&grant.repository)?;
            validate_identity(&grant.author_name, &grant.author_email)?;
            validate_token(&grant.token.0)?;
            if !repositories.insert(grant.repository.to_ascii_lowercase()) || !targets.insert(&grant.target) {
                return Err(Error::Invalid("Duplicate Git credential grant"));
            }
        }
        Ok(())
    }
}

impl Prepared {
    /// Grants for a primary repository and its same-worker siblings. Without siblings this is
    /// exactly [`Prepared::for_repository`], so single-repository clouds keep the version 1 file.
    /// With siblings the payload is always version 2, even for a primary-only grant, because
    /// only version 2 keeps `gh` from sending the primary token from a sibling checkout.
    /// # Errors
    /// Fails before transfer on any selection error or unsafe token file.
    pub fn for_repositories(bindings: &[Binding], primary: &Path, siblings: &[Sibling<'_>]) -> Result<Option<Self>> {
        if siblings.is_empty() {
            return Self::for_repository(bindings, primary);
        }
        let selected = select(bindings, primary, siblings)?;
        if selected.is_empty() {
            return Ok(None);
        }
        let grants = selected
            .into_iter()
            .map(|Selected { target, binding }| {
                let mut token = read_token(binding)?;
                Ok(Grant {
                    repository: binding.repository.clone(),
                    token: Token(std::mem::take(&mut *token)),
                    author_name: binding.author_name.clone(),
                    author_email: binding.author_email.clone(),
                    target,
                })
            })
            .collect::<Result<_>>()?;
        let set = GrantSet {
            version: Version,
            grants,
        };
        set.validate()?;
        let prepared = Self::private(&set)?;
        if prepared.0.as_file().metadata()?.len() > MAX_PAYLOAD_BYTES {
            return Err(Error::Invalid("Git credential grants are too large for the worker"));
        }
        Ok(Some(prepared))
    }
}

#[cfg(test)]
mod tests;
