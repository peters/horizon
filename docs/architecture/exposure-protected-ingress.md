# Protected exposure ingress facts

This default-off Protocol prerequisite defines physical port geometry and a
bounded passive receipt. It configures no host, opens no socket, changes no
existing target validation and enables no UI, CLI or MCP action. The existing
`producer_backend_port` helper keeps its previous meaning. Later deployment and
worker changes must explicitly adopt the new geometry together.

## Geometry

| Purpose | Logical/external port | Physical container port | Published |
| --- | --- | --- | --- |
| Existing management SSH | 22 | 22 | Yes |
| Public Gate ingress | 20000–20031 | 600–631, same offset | Yes |
| Tailnet Serve virtual service | 20000–20031 | Separate owned Serve mapping | No Docker publication |
| Private Serve backend | Paired with virtual service | 632–663, same offset | Never |

Protecting only the public listener leaves private backends vulnerable after a
Gate closes while an uncertain Serve mapping still exists. Both physical pools
are below 1024. A later runtime must verify an effective network-namespace
`ip_unprivileged_port_start` of at least 1024, root ownership and effective
`CAP_NET_BIND_SERVICE`; agent processes must retain no capabilities. A claimed
sysctl value in JSON is not that verification. No high-port fallback is safe.

Linux defines this sysctl per network namespace; zero disables privileged
ports. `ip_local_reserved_ports` does not restrict explicit binds. See the
[kernel IP sysctl documentation](https://kernel.org/doc/html/latest/networking/ip-sysctl.html).
Docker supports namespaced `net.*` sysctls, and normally retains
`NET_BIND_SERVICE`; the later worker must check effective state rather than
infer it from defaults. See [Docker run](https://docs.docker.com/engine/containers/run/)
and [namespaced sysctls](https://docs.docker.com/reference/cli/docker/container/run/#sysctl).

## Receipt version 1

`Receipt::from_json` accepts at most 16 KiB. All fields are mandatory. Unknown
and duplicate fields are refused at every struct boundary. The validated wrapper
exposes immutable comparison facts and has no deserializer or runtime factory.

- `version`: 1.
- `allocation`: the existing validated worker allocation operation ID.
- `generation`: a fresh non-nil UUID for this exact container start.
- `container.id`: full lowercase 64-hex Docker CID.
- `container.requested_image`: exact immutable image reference with a lowercase
  SHA-256 digest; not a tag.
- `container.image_id`: actual Docker image configuration ID, `sha256:` plus
  64 lowercase hex digits. This differs from the requested manifest digest.
- `container.init_start_ticks`: nonzero original container-init start time.
- `container.netns`: actual network namespace `device` and nonzero `inode`.
- `unprivileged_port_start`: claimed observed effective boundary, at least 1024.
- `published`: entries with `protocol: "tcp"`, `host_ip`, `host_port` and
  `container_port`. Each represented family has exactly the 33 mappings in the
  table: SSH22 and all 32 Gate ports. Only wildcard IPv4 and IPv6 addresses are
  accepted. One complete family or both complete families are allowed. Extra,
  missing, duplicate, backend, UDP and partial-family mappings are refused.

Single-family facts do not imply another family is available. Publication facts
do not prove original source preservation. Docker can map host IPv6 to container
IPv4 through its userland proxy; real accepted-source qualification must govern
which paths a future controller admits. See
[Docker port publishing](https://docs.docker.com/engine/network/port-publishing/).

## Required later producer and consumers

The approved next provisioning change is opt-in for new Hetzner allocations.
An absent/false Bootstrap flag must preserve the current host script and SSH22
behavior. The host must start the exact CID, inspect actual published bindings,
requested and actual image identities, and its original init process. It must
observe init start time and network namespace, then repeat the CID/process
binding checks before atomically writing a bounded root-owned receipt into a
host-private directory mounted read-only into that container. Missing or
inconsistent readback must not publish a success receipt. No Docker socket goes
into the worker. No existing worker is recreated or upgraded by decoding facts.

The protected worker must compare the fixed root read-only receipt with its
actual current allocation, original PID1 start time, actual network namespace,
effective sysctl and root capability. Filesystem ownership, immutable original
generation and repeated observations belong to that runtime boundary; this
module performs no file or kernel reads. A caller-supplied receipt, environment
override or saved record cannot establish it.

Core must acquire and retain that readback through the same original pinned SSH
connection and cloud custody before provider ingress or endpoint display. It
must refuse missing/changed bindings, cancel physically on drift and never
reconnect to a successor. Saved profile intent must not manufacture
`Worker.portMappings`. Provider firewall success alone does not prove host
publication. Existing logical wire ports, original accepted-stream producer
proofs and per-service closure guarantees must survive this later composition.

These are required remaining implementation and real-input acceptance steps,
not capabilities supplied by this passive prerequisite.
