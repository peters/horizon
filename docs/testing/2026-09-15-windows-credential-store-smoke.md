# Windows credential store smoke (2026-09-15)

Part of [#628](https://github.com/peters/horizon/issues/628): the Windows half
of the OS credential store, exercised against Windows Credential Manager on a
rented Azure VM. Host: `Standard_D4ds_v6` in `northeurope` running Windows
Server 2022 Datacenter Azure Edition (10.0.20348), Visual Studio 2022 Build
Tools (C++ workload) and Rust 1.98.1 installed for the run, the source taken
from `main` as of 04:56 UTC (9b07fea7, which includes #662). The VM had no
public IP; every step ran through `az vm run-command` and a scheduled task, and
the VM and its network resources were deleted afterwards.

## What ran

```
cargo test -p horizon-core remote_browser_credential
cargo test -p horizon-core os_store_round_trip_smoke -- --ignored --nocapture
```

The first command is every unit test of the credential module compiled for
Windows (the Windows adapter's single-target attribute probe included). The
second is the opt-in round trip: `KeyringCredentialStore::open`, a presence
probe under a unique `remote-browser/smoke/<pid>` slot, `put`, probe and
readiness `Present`, read-back through the secret sink, `delete`, and a probe
that finds nothing.

## Result

| Step | Outcome |
| --- | --- |
| Credential module unit tests on Windows | 15 passed, 0 failed, 1 ignored (the opt-in smoke) |
| `KeyringCredentialStore::open()` | store available |
| Presence probe before the write | absent |
| `put` of the smoke value | stored |
| Presence probe and readiness | `Present` |
| Read back through the secret sink | matches |
| `delete`, then `cmdkey /list` | no Horizon entry left |

Both commands finished in about six minutes of build time; the round trip
itself took 0.10 s.

## Account context

The run executed as `NT AUTHORITY\SYSTEM`. The VM's run-command channel could
not register a scheduled task under the administrator account (Windows rejected
the credential for task registration even though the same credential validated
against the local account database), so the build and the test ran as the
system account, whose Credential Manager vault is its own. The API path is the
same for any account (`CredWrite`, `CredRead`, `CredDelete` on generic
credentials), and no interaction is involved on Windows, so this is the store
behaviour a signed-in user gets; it is not a test of a roaming or
domain-managed vault.

## Not covered

- Starting a remote device session from Windows (the remainder of the
  per-platform acceptance item) was not exercised; the evidence tooling is
  Linux-only.
