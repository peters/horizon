//! What a worker learns about a deployment's siblings: the manifest that lays out their
//! checkouts beside the primary's, the session worktree that follows from it, and the
//! checkouts its Git grants may target. Local paths never reach the worker.
use super::{Error, Result, Set};
use crate::cloud_runtime::{git_auth, state::Deployment};
use serde::Serialize;

pub const SHARED_CHECKOUT_ROOT: &str = "/workspace/checkout";

/// The input of `horizon-worker-siblings set`.
#[derive(Serialize)]
struct Manifest<'a> {
    version: u32,
    primary: &'a str,
    siblings: Vec<Entry<'a>>,
}

#[derive(Serialize)]
struct Entry<'a> {
    alias: &'a str,
    directory: &'a str,
    revision: &'a str,
}

impl Set {
    /// The worker manifest of this set, on one line of JSON punctuation, letters, digits,
    /// `.`, `_` and `-`, so it can travel as shell text. Built before allocation, so a set
    /// the worker could not record is refused before a worker is paid for.
    /// # Errors
    /// A recorded field holds anything else.
    pub fn manifest(&self) -> Result<String> {
        let manifest = serde_json::to_string(&Manifest {
            version: 1,
            primary: &self.primary_directory,
            siblings: self
                .members
                .iter()
                .map(|sibling| Entry {
                    alias: &sibling.alias,
                    directory: &sibling.directory,
                    revision: &sibling.revision,
                })
                .collect(),
        })
        .map_err(|_| Error::Json)?;
        if !manifest
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"{}[]\":,._-".contains(&byte))
        {
            return Err(Error::Invalid("Invalid same-worker sibling manifest"));
        }
        Ok(manifest)
    }

    /// The pinned checkouts, as targets of per-repository Git grants.
    /// # Errors
    /// A checkout was moved or removed.
    pub fn grant_siblings(&self) -> Result<Vec<git_auth::Sibling<'_>>> {
        self.members
            .iter()
            .map(|sibling| {
                Ok(git_auth::Sibling {
                    alias: &sibling.alias,
                    local_repository: sibling.checkout()?,
                })
            })
            .collect()
    }
}

/// The Git grants of a deployment: its primary's alone, or with same-worker siblings a
/// version 2 set chosen by the same selection as the image contract check.
/// # Errors
/// A sibling checkout was moved, or a binding or token cannot be used.
pub fn git_grants(credentials: &[git_auth::Binding], state: &Deployment) -> Result<Option<git_auth::Prepared>> {
    let siblings = match &state.siblings {
        Some(set) => set.grant_siblings()?,
        None => Vec::new(),
    };
    git_auth::Prepared::for_repositories(credentials, &state.repository, &siblings)
}

/// The cloud's shared primary checkout, beside its selected siblings when present.
#[must_use]
pub fn shared_worktree(siblings: Option<&Set>) -> String {
    match siblings {
        Some(set) => format!("{SHARED_CHECKOUT_ROOT}/{}", set.primary_directory),
        None => SHARED_CHECKOUT_ROOT.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cloud_runtime::siblings::Sibling;

    fn set() -> Set {
        Set {
            primary_directory: "app".into(),
            members: vec![Sibling {
                alias: "native".into(),
                repository: "example/native-lib".into(),
                directory: "native-lib".into(),
                revision: "b".repeat(40),
                image_revision: None,
                local_repository: "/synthetic/native-lib".into(),
                profile: "gpu".into(),
            }],
        }
    }

    #[test]
    fn the_manifest_names_checkout_directories_and_revisions_but_no_local_path() {
        let manifest = set().manifest().unwrap();
        assert_eq!(
            manifest,
            format!(
                r#"{{"version":1,"primary":"app","siblings":[{{"alias":"native","directory":"native-lib","revision":"{}"}}]}}"#,
                "b".repeat(40)
            )
        );
        let mut unsafe_text = set();
        unsafe_text.members[0].directory = "native'lib".into();
        assert!(unsafe_text.manifest().is_err());
    }

    #[test]
    fn a_session_with_siblings_works_in_the_primary_directory_of_its_root() {
        assert_eq!(shared_worktree(None), "/workspace/checkout");
        assert_eq!(shared_worktree(Some(&set())), "/workspace/checkout/app");
    }

    #[test]
    fn grants_target_only_checkouts_that_are_still_in_place() {
        let checkout = tempfile::tempdir().unwrap();
        let mut set = set();
        set.members[0].local_repository = checkout.path().to_path_buf();
        let targets = set.grant_siblings().unwrap();
        assert_eq!(
            (targets[0].alias, targets[0].local_repository),
            ("native", checkout.path())
        );
        set.members[0].local_repository = checkout.path().join("moved");
        assert!(set.grant_siblings().is_err());
    }

    #[test]
    fn a_sibling_only_credential_is_granted_once_the_record_has_its_sibling() {
        use std::io::Write;
        let primary = tempfile::tempdir().unwrap();
        let library = tempfile::tempdir().unwrap();
        let mut token = tempfile::NamedTempFile::new().unwrap();
        token.write_all(b"library-token").unwrap();
        let credentials = [git_auth::Binding {
            local_repository: library.path().into(),
            repository: "example/native-lib".into(),
            token_file: token.path().into(),
            author_name: "Library author".into(),
            author_email: "test@example.invalid".into(),
        }];
        let mut state: Deployment = serde_json::from_value(serde_json::json!({
            "version":1,"cloud_id":"grants","repository":primary.path(),"revision":"a".repeat(40),
            "profile":{"provider":"runpod","image":"registry.example/worker","cpu":4,"memory_gb":8},
            "stage":"Validate","operation":{"state":"prepared"},"spec":null,"worker":null,"sessions":[]
        }))
        .unwrap();
        assert!(
            git_grants(&credentials, &state).unwrap().is_none(),
            "the primary has no binding"
        );
        let mut siblings = set();
        siblings.members[0].local_repository = library.path().to_path_buf();
        state.siblings = Some(siblings);
        assert!(git_grants(&credentials, &state).unwrap().is_some());
    }
}
