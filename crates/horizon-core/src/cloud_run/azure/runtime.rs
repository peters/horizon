//! Explicit operator-owned policies for a newly created worker container.

use crate::cloud_run::ArtifactDigest;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};

/// Container security configuration frozen into the worker's placement binding.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AzureContainerRuntime {
    /// Preserve the provider's default container filters and capabilities.
    #[default]
    Default,
    /// Scoped filters qualified for nested filesystem sandboxes on Docker 29.1.3.
    WorkspaceSandboxV1,
}

impl AzureContainerRuntime {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::WorkspaceSandboxV1 => "workspace_sandbox_v1",
        }
    }

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Default => "Docker defaults",
            Self::WorkspaceSandboxV1 => "Workspace sandbox v1 (scoped filters)",
        }
    }

    pub(super) const fn packages(self) -> &'static str {
        match self {
            Self::Default => "docker.io, iptables-persistent",
            Self::WorkspaceSandboxV1 => "docker.io, iptables-persistent, apparmor",
        }
    }

    pub(super) fn files(self) -> String {
        match self {
            Self::Default => String::new(),
            Self::WorkspaceSandboxV1 => format!(
                "  - path: /etc/apparmor.d/horizon-workspace-sandbox-v1\n    permissions: '0644'\n    owner: root:root\n    encoding: b64\n    content: {}\n  - path: /etc/horizon/worker-seccomp-v1.json\n    permissions: '0644'\n    owner: root:root\n    encoding: b64\n    content: {}\n  - path: /usr/local/sbin/horizon-worker-runtime-preflight\n    permissions: '0700'\n    owner: root:root\n    encoding: b64\n    content: {}\n",
                STANDARD.encode(include_bytes!("runtime/workspace-v1.apparmor")),
                STANDARD.encode(include_bytes!("runtime/workspace-v1.seccomp.json")),
                STANDARD.encode(Self::preflight()),
            ),
        }
    }

    pub(super) const fn daemon_dependencies(self) -> &'static str {
        match self {
            Self::Default => "",
            Self::WorkspaceSandboxV1 => {
                "      Requires=apparmor.service\n      After=apparmor.service\n      [Service]\n      ExecStartPre=/usr/local/sbin/horizon-worker-runtime-preflight\n"
            }
        }
    }

    fn preflight() -> String {
        include_str!("runtime/preflight-workspace-v1.sh")
            .replace(
                "__APPARMOR_SHA__",
                ArtifactDigest::sha256(include_bytes!("runtime/workspace-v1.apparmor")).as_str(),
            )
            .replace(
                "__SECCOMP_SHA__",
                ArtifactDigest::sha256(include_bytes!("runtime/workspace-v1.seccomp.json")).as_str(),
            )
    }

    pub(super) const fn prepare(self) -> &'static str {
        match self {
            Self::Default => "",
            Self::WorkspaceSandboxV1 => include_str!("runtime/prepare-workspace-v1.sh"),
        }
    }

    pub(super) const fn flags(self) -> &'static str {
        match self {
            Self::Default => "",
            Self::WorkspaceSandboxV1 => {
                " --security-opt seccomp=/etc/horizon/worker-seccomp-v1.json --security-opt apparmor=horizon-workspace-sandbox-v1"
            }
        }
    }

    pub(super) fn qualify(self, image: &str) -> String {
        match self {
            Self::Default => String::new(),
            Self::WorkspaceSandboxV1 => include_str!("runtime/qualify-workspace-v1.sh")
                .replace("__RUNTIME_FLAGS__", self.flags())
                .replace("__WORKER_IMAGE__", image),
        }
    }
}

#[cfg(test)]
mod tests;
