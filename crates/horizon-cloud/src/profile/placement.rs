//! Where a new workspace for a repository runs.
use serde::{Deserialize, Serialize};

/// The optional `placement` of `.horizon/cloud.yml`: a new workspace for the repository runs
/// in the cloud, or on This PC with `placement: local`.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum WorkspacePlacement {
    #[default]
    Cloud,
    Local,
}

impl WorkspacePlacement {
    #[must_use]
    pub fn is_cloud(&self) -> bool {
        *self == Self::Cloud
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_repository_runs_in_the_cloud_unless_it_says_local() {
        #[derive(Deserialize)]
        struct File {
            #[serde(default)]
            placement: WorkspacePlacement,
        }
        let read = |text: &str| serde_yaml::from_str::<File>(text).map(|file| file.placement);
        assert_eq!(read("{}").unwrap(), WorkspacePlacement::Cloud);
        assert_eq!(read("placement: local").unwrap(), WorkspacePlacement::Local);
        assert_eq!(read("placement: cloud").unwrap(), WorkspacePlacement::Cloud);
        assert!(read("placement: moon").is_err());
    }
}
