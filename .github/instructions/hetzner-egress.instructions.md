---
applyTo: "crates/horizon-cloud/src/egress.rs,crates/horizon-cloud/src/egress/**,crates/horizon-cloud/src/host.rs,crates/horizon-cloud/src/host/**,docs/cloud-hetzner.md"
---

# Trust boundary of the outbound allowlist

A profile with `egress` makes the Hetzner host limit where its worker container can
connect. The user-facing description is in `docs/cloud-hetzner.md`, section
"Outbound allowlist", and in `docs/upkeep.md`, section "Limits on a misled agent".
Upkeep clouds use it because their agent reads untrusted pull request text and runs
without approvals.

## Untrusted: everything that comes from the container

Root and agents in the container, and code that they run, choose every byte that
reaches the host: TLS records, the name in a ClientHello, DNS queries, the address a
connection goes to, the timing, and the folders on the workspace volume. Check that:

- `host/egress.py` reads at most `HELLO_LIMIT` bytes before it decides, and every
  parse of a ClientHello or a DNS query refuses a malformed or truncated message.
  Nothing it reads can raise out of a connection's thread or stop the service.
- The proxy connects only to the name in the ClientHello when `allowed` accepts it,
  resolves that name itself, ignores the address that the container chose, and never
  connects to an address that is not global (loopback, private, link-local, the
  bridge of the host, the metadata service).
- The resolver forwards only a query with one question for an allowed name, and
  answers NXDOMAIN for every other name.
- `allowed` matches an exact name, or a name under a `*.` entry, with ASCII case and
  a last dot ignored. `evilexample.com` never matches `example.com`.
- The log of refused names is rate-limited and written only into a folder that root
  owns and that no other account can write, without following a link.
- The number of threads is bounded, so a flood from the container cannot exhaust
  the host.

## Fail closed

- The container network has no address translation
  (`com.docker.network.bridge.enable_ip_masquerade=false`), so a packet that a rule
  misses cannot reach the internet.
- The guard script runs before each container start with `set -eu` and installs every
  rule; a failure must stop the start.
- When the proxy or the resolver stops, the container loses that way out and never
  gains another. No rule may accept forwarded traffic from the container other than
  replies (`ESTABLISHED,RELATED`).
- No file that the allowlist depends on is on the volume or reachable from the
  container.

## Trusted: the person's profile

The extra names in `egress.allow` come from the person. `Egress::validate` refuses
names that are not lowercase host names and wildcards over fewer than two labels. All
values reach the host as files written by cloud-init, never interpolated into a
script.

## Not covered

The host cannot tell agents from the services of the worker, so data sent to an
allowed name with another account's credential is not stopped. The docs say so; do
not claim otherwise.
