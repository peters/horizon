//! Lexical transfer policy only; callers must separately validate real filesystem nodes.

use super::OverlayPlanError as Error;
use std::path::Path;

pub(super) const MAX_PATH_BYTES: usize = 4096;

pub(super) fn validate(path: &str) -> Result<(), Error> {
    if !supported_text(path) || path.split('/').any(|part| !valid_component(part)) {
        return Err(Error::InvalidPath);
    }
    if path.split('/').any(excluded) {
        return Err(Error::ExcludedPath);
    }
    Ok(())
}

pub(super) fn validate_link(path: &str, target: &str) -> Result<(), Error> {
    if !supported_text(target) || target.starts_with('/') {
        return Err(Error::InvalidLink);
    }
    let mut resolved: Vec<_> = path.split('/').collect();
    resolved.pop();
    for part in target.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                if resolved.pop().is_none() {
                    return Err(Error::InvalidLink);
                }
            }
            _ if !valid_component(part) || excluded(part) => return Err(Error::InvalidLink),
            _ => resolved.push(part),
        }
    }
    Ok(())
}

fn supported_text(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_PATH_BYTES
        && !value
            .chars()
            .any(|character| character.is_control() || matches!(character, '\\' | ':'))
}

fn valid_component(part: &str) -> bool {
    !matches!(part, "" | "." | "..") && !part.ends_with(['.', ' '])
}

fn excluded(part: &str) -> bool {
    let lower = part.to_ascii_lowercase();
    lower == ".env"
        || lower.starts_with(".env.")
        || Path::new(part)
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("env"))
        || lower == ".envrc"
        || matches!(
            lower.as_str(),
            ".git"
                | ".hg"
                | ".svn"
                | ".ssh"
                | ".gnupg"
                | ".aws"
                | ".azure"
                | ".kube"
                | ".docker"
                | ".config"
                | ".codex"
                | ".claude"
                | ".claude.json"
                | ".netrc"
                | ".npmrc"
                | ".pypirc"
                | ".git-credentials"
                | "credentials.json"
                | "auth.json"
                | "id_rsa"
                | "id_ed25519"
                | ".cache"
                | "__pycache__"
                | "node_modules"
                | "target"
                | ".venv"
                | "venv"
                | ".pytest_cache"
                | ".mypy_cache"
        )
}
