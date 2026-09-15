The seccomp policy derives from Moby's Apache-2.0 licensed Docker 29.1.3 default:
https://raw.githubusercontent.com/moby/moby/docker-v29.1.3/vendor/github.com/moby/profiles/seccomp/default.json

It preserves that default policy and adds unconditional allow rules for clone,
unshare, setns, mount, umount2 and pivot_root. clone3 retains its ENOSYS fallback.
The default process/system denial rules and default container capability set
remain in force. These changes permit nested user/mount namespaces; the coding
agent still must apply its own filesystem policy inside the container.

The AppArmor policy derives from Moby's default container template:
https://github.com/moby/profiles/blob/main/apparmor/template.go

It names the worker profile, permits user namespaces and nested mount operations,
and retains the default process/system write denials. It does not replace the
host's default container profile or disable host user-namespace restrictions.
LICENSE-MOBY contains the upstream Apache-2.0 license.

Qualified baseline: Ubuntu 24.04 Azure worker with Docker 29.1.3, root and UID1000,
no extra capabilities. Native UI and Chromium/Firefox public MCP smoke passed
under these scoped filters. Automatic bootstrap fails closed on other Docker
versions until they are qualified. A runtime selection is operator configuration;
repository worker YAML cannot grant these settings.
