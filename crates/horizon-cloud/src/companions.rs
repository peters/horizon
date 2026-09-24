//! Passive companion declarations and stable selection. No lifecycle or access side effects.
use crate::{ProfileError, valid_id};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Declaration {
    /// GitHub repository identity, `owner/repository`, without credentials or local paths.
    pub repository: String,
    pub profile: String,
}

/// Host-resolved ownership, never authority supplied by a remote caller.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Scope {
    pub session_id: String,
    pub workspace_id: String,
}

/// A target from the owning host's current inventory, not proof of SSH readiness.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Target {
    pub scope: Scope,
    pub cloud_id: String,
    pub declaration: Declaration,
}

/// A local selection pins both ends and the declaration. Repository edits cannot rebind it.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Selection {
    scope: Scope,
    source_cloud_id: String,
    alias: String,
    declaration: Declaration,
    target_cloud_id: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SelectionError {
    #[error("Invalid companion identity")]
    Invalid,
    #[error("Companion selection belongs to another cloud or workspace")]
    ScopeMismatch,
    #[error("Companion declaration changed; select the target again")]
    DeclarationChanged,
    #[error("Selected companion cloud is missing; it was not replaced")]
    Missing,
    #[error("Multiple companion clouds match; select an explicit target")]
    Ambiguous,
}

impl Declaration {
    /// # Errors
    /// Accepts repository identities, not URLs, paths, credential bindings or shell text.
    pub fn validate(&self) -> Result<(), ProfileError> {
        let Some((owner, repository)) = self.repository.split_once('/') else {
            return Err(ProfileError::Invalid("Companion repository must be owner/repository"));
        };
        let component = |value: &str| {
            !value.is_empty()
                && !matches!(value, "." | "..")
                && value.len() <= 100
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
        };
        if !component(owner)
            || !owner.bytes().next().is_some_and(|byte| byte.is_ascii_alphanumeric())
            || !component(repository)
            || !valid_id(&self.profile)
        {
            return Err(ProfileError::Invalid("Invalid companion repository or profile"));
        }
        Ok(())
    }

    /// GitHub repository names are case-insensitive; local profile names are not.
    #[must_use]
    pub fn matches(&self, other: &Self) -> bool {
        self.repository.eq_ignore_ascii_case(&other.repository) && self.profile == other.profile
    }
}

/// # Errors
/// Rejects invalid aliases and excessive declarations before any allocation.
pub fn validate_declarations(declarations: &BTreeMap<String, Declaration>) -> Result<(), ProfileError> {
    if declarations.len() > 64 {
        return Err(ProfileError::Invalid("At most 64 companion repositories are supported"));
    }
    for (alias, declaration) in declarations {
        if !valid_alias(alias) {
            return Err(ProfileError::Invalid("Invalid companion alias"));
        }
        declaration.validate()?;
    }
    Ok(())
}

fn valid_alias(alias: &str) -> bool {
    !alias.is_empty()
        && alias.len() <= 64
        && alias.bytes().next().is_some_and(|byte| byte.is_ascii_lowercase())
        && alias
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"-_".contains(&byte))
}

impl Selection {
    /// Explicitly select one host-resolved target. This neither connects nor starts it.
    /// # Errors
    /// Rejects malformed identities, self-selection and targets outside the source scope.
    pub fn new(source: &Target, alias: &str, target: &Target) -> Result<Self, SelectionError> {
        if !valid_alias(alias)
            || !valid_id(&source.cloud_id)
            || !valid_id(&target.cloud_id)
            || source.cloud_id == target.cloud_id
            || target.declaration.validate().is_err()
            || !valid_id(&source.scope.session_id)
            || !valid_id(&source.scope.workspace_id)
        {
            return Err(SelectionError::Invalid);
        }
        if source.scope != target.scope {
            return Err(SelectionError::ScopeMismatch);
        }
        Ok(Self {
            scope: source.scope.clone(),
            source_cloud_id: source.cloud_id.clone(),
            alias: alias.into(),
            declaration: target.declaration.clone(),
            target_cloud_id: target.cloud_id.clone(),
        })
    }

    /// Resolve a saved selection against trusted inventory and the current declaration.
    /// # Errors
    /// Never falls back to a different cloud when the pinned target disappears or changes.
    pub fn resolve<'a>(
        &self,
        source: &Target,
        alias: &str,
        declaration: &Declaration,
        inventory: &'a [Target],
    ) -> Result<&'a Target, SelectionError> {
        if self.scope != source.scope || self.source_cloud_id != source.cloud_id {
            return Err(SelectionError::ScopeMismatch);
        }
        if self.alias != alias || !self.declaration.matches(declaration) {
            return Err(SelectionError::DeclarationChanged);
        }
        let mut matches = inventory
            .iter()
            .filter(|target| target.cloud_id == self.target_cloud_id && target.scope == self.scope);
        let target = matches.next().ok_or(SelectionError::Missing)?;
        if matches.next().is_some() {
            return Err(SelectionError::Ambiguous);
        }
        Self::new(source, alias, target)?;
        if !target.declaration.matches(&self.declaration) {
            return Err(SelectionError::DeclarationChanged);
        }
        Ok(target)
    }
}

/// Discover possible bindings, excluding the source and other workspaces.
/// A caller must make an explicit selection; discovery does not authorize access.
#[must_use]
pub fn candidates<'a>(source: &Target, declaration: &Declaration, inventory: &'a [Target]) -> Vec<&'a Target> {
    inventory
        .iter()
        .filter(|target| {
            target.cloud_id != source.cloud_id
                && target.scope == source.scope
                && target.declaration.matches(declaration)
        })
        .collect()
}

#[cfg(test)]
mod tests;
