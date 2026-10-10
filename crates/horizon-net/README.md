# Horizon Net

`horizon-net` connects named TCP services in a private workspace network. It
needs no root access, network interface or provider SDK. Iroh authenticates
endpoint keys and encrypts every connection with QUIC and TLS 1.3.

The crate exposes one library and the `horizon-net agent` binary. The relay is
the upstream `iroh-relay` binary. Configure your own relay URLs explicitly:
there is no public relay or discovery fallback. `relay_only: true` removes all
UDP transports for hosts that can only make outbound TCP connections.

```rust
use horizon_net::{Controller, Topology};

let controller = Controller::new(Topology::empty("example-workspace"))?;
let mut proposed = controller.topology();
proposed.revision += 1;
let plan = controller.plan(proposed)?; // No sockets, files or policy changes.
// The embedding application confirms this exact plan before calling apply.
let result = controller.apply(&plan)?;
# Ok::<(), horizon_net::Error>(())
```

The host owns topology confirmation and audit records. The library applies a
trusted local plan; it does not impersonate a person's confirmation. A plan
contains the complete old and new topology, exact changed objects and a
conservative widening indicator. A modified or stale plan fails. Repeating an
already applied exact plan changes nothing.

Each service names its host node and loopback TCP port. Each grant names source
nodes, one service, and an absolute Unix expiry. A source public key comes from
the authenticated iroh connection. The source cannot choose an arbitrary host
or port. Undeclared services, wrong host nodes, foreign network names, revoked
identities and expired grants fail closed. Revocation shuts down the local TCP
socket and QUIC connection immediately; expiry runs independently of the host.

## Agent configuration

Save a JSON document in a private file. On Unix, the file must be owned by the
agent user with mode `0600`; its state directory uses mode `0700`.

```json
{
  "node": "worker-a",
  "secret_key": "<64 hexadecimal characters from iroh SecretKey::generate()>",
  "authority_key": "<authority endpoint public key>",
  "relay_urls": ["https://relay.example.com"],
  "relay_only": false,
  "topology": {
    "schema_version": 1,
    "network": "example-workspace",
    "revision": 1,
    "nodes": {
      "worker-a": {"key": "<worker-a endpoint public key>"},
      "desktop": {"key": "<desktop endpoint public key>"}
    },
    "services": {
      "desktop/ssh": {"node": "desktop", "port": 22}
    },
    "grants": {
      "worker-ssh": {
        "from": ["worker-a"],
        "to": "desktop/ssh",
        "expires_at": 2000000000
      }
    }
  }
}
```

Run `horizon-net agent --config /absolute/path/config.json`. The agent prints
its public key after it connects to the configured relay. It never prints its
secret key. Generate identities with upstream `SecretKey::generate()`; use
`SecretKey::to_bytes()` to write the hexadecimal secret to the private file.

Persistent agents require Unix directory durability. On Windows and other
non-Unix platforms, `agent`, `Agent::bind_persistent` and
`Agent::bind_with_store` return an I/O `Unsupported` error before creating or
locking state. `Agent::bind` and `Controller::new` remain available for trusted
in-memory transport and policy; this form denies remote topology updates.

The binary stores immutable topology revisions in the sibling `config.state`
directory. Keep that directory when an agent restarts. The newest valid
published revision overrides the original configuration topology. A visible
snapshot cannot prove that a previous durability barrier or acknowledgement
succeeded. Every persistent restart therefore starts with service access denied.
Status reports `policy_state: awaiting_confirmation` and inactive grants until
the pinned authority sends the exact current topology or an accepted newer one.
That update repeats the publication barriers before service access becomes active.
The retained identity can still receive authority updates while access is denied.
Corrupt or conflicting state blocks startup. One exclusive file lock prevents two agents from owning
the same state. Snapshots bind the network, authority key and initial node key;
changing one requires an explicit new enrollment, not an automatic reset.

For an embedded host, use `Agent::bind(config, controller.clone())`. This form
shares its trusted local controller and denies remote topology updates. For a
remote agent, use `Agent::bind_persistent(config_path)` or
`Agent::bind_with_store(config, state_directory)`.

`Controller::new` creates an in-memory controller. The embedded host must save
its policy before it calls `apply` or `revoke`. Use a persistent agent's
controller for restart storage.

The controller returned by a persistent agent writes every successful `apply`
and `revoke` before changing live policy. Its clones share the state writer;
drop all controller and agent owners before reopening that state directory.
A failed write leaves live policy and existing sessions unchanged. Publication
can be uncertain: an error does not prove that a new snapshot is absent. Restart
keeps its topology for authority confirmation and denies service access. Once a
valid snapshot exists, editing the enrollment file cannot override its topology,
even with a higher revision. Invalid secret keys, authority keys, local identity
bindings and relay configuration fail before the state directory is created.

For a trusted embedded controller with persistent state, confirm the exact
retained topology with `apply(controller.plan(controller.topology())?)` after
restart. An identical plan still reports `changed: false`; when confirmation is
pending it repeats durability barriers and enables policy only after success.
A failed confirmation keeps service access denied. A new state directory may
activate its validated enrollment only after its first publication succeeds.

Only the configured authority key can call `Agent::push_topology`. The receiver
commits state and closes invalid sockets before it acknowledges the update.
An unreachable receiver cannot acknowledge revocation; its existing absolute
lease still expires locally. The host must report pending delivery honestly.

Use `Agent::forward(destination, service, local_port)` to create a loopback TCP
listener for SSH, VNC, CDP or a dev server. Keep its `Forwarder` alive for the
listener lifetime. Dropping it stops the listener and its owned connections.
`Agent::shutdown()` closes all agent listeners and connections, including
forwarders held by other code. `Agent::probe()` checks authenticated network
membership. Status marks a node reachable only after an actual response or a
confirmed active session; a successful probe stays fresh for 30 seconds.

## Relay configuration

`RelayConfiguration::to_toml()` produces upstream iroh-relay 1.3 configuration
with TLS on TCP port 443, ACME certificates and an endpoint allowlist. The HTTP
listener binds loopback. UDP address discovery and public metrics are disabled.
Run `horizon-net relay-config --config relay.json` to print this TOML. Install
and operate upstream `iroh-relay` separately. Port 443 can require an operator
service or platform binding capability; workspace agents remain unprivileged.

Destination addresses passed to `probe`, `push_topology` or `forward` can name
only configured relays. An unconfigured relay fails before transport or a
forwarding listener starts. Direct IP addresses remain available when IP
transports are enabled; `relay_only` removes those transports entirely.

The relay allowlist limits infrastructure use. The endpoint grants enforce
workspace access. The relay holds no workspace topology or decryption key.
Allowlist changes require the operator to update the upstream relay; they do
not replace endpoint revocation.

A durable agent whose identity was withdrawn can restart with its original
private enrollment file. It stays denied and can receive only a newer update
from its pinned authority. The original file cannot restore membership. A
changed local public key still requires a new explicit enrollment; it cannot
reuse the previous identity's state directory.

## Dependencies and validation

The runtime uses iroh, tokio and serde. `serde_json` provides bounded wire and
configuration serialization. `thiserror` provides typed errors. Unix-only
`rustix` checks effective user ownership without unsafe code. These small API
and filesystem dependencies are deliberate exceptions to the original three
dependency target. Test-only `iroh-relay` starts the upstream relay;
`tempfile` owns disposable state. There are no Horizon UI, core or provider
dependencies, and no custom cryptography.

Unix validates private owner and mode checks and requires a successful directory
flush before a persistent update changes live policy. Windows persistent state
is unsupported because the crate has no Windows publication durability barrier.
Windows in-memory APIs remain usable; caller-managed configuration must use
user-only ACLs. The crate checks regular files, size and corruption when reading
configuration, but does not validate Windows ACLs. Cross-platform CI and real
provider smoke evidence remain separate gates.

Run `cargo test -p horizon-net` for the deterministic policy and transport
tests. The upstream relay test removes UDP transports on every endpoint and
passes actual TCP bytes through a disposable loopback relay. It tests denial,
revocation, expiry, key rotation and restart. It does not qualify TLS on port
443 or provider connectivity. Use the [data-plane procedure](../../docs/testing/procedures/horizon-net-data-plane.md)
for those lanes. The optional `tailnet-bridge` feature remains off by default.

Primary references: [iroh 1.3 API](https://docs.rs/iroh/1.3.0/iroh/),
[upstream relay configuration](https://github.com/n0-computer/iroh/blob/v1.3.0/iroh-relay/src/main.rs).
