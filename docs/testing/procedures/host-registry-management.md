---
procedure: host-registry-management
feature: check private worker pull access without a compute account
platforms: [linux, macos]
cost: none
destructive: no
secrets: synthetic-only
owner: peters
---

# Host-only registry management

## 1. Purpose

Check the shared host-only pull API and its UI, CLI and stdio MCP entry points.

## 2. Applicability

Durable registry journals require a supported Unix host. These fixtures neither
publish images nor allocate compute. Synthetic results prove routing, persistence
and display, not live issuer permission.

## 3. Safety

> **CAUTION:** ENTER ONLY SYNTHETIC CREDENTIALS IN THE OWNED FIXTURE.
> Do not open the user's settings or change installed credentials.

Keep the fixture's state, input target and resources separate from active applications.
No step publishes an image, rents a worker or revokes an installed host credential.

## 4. Equipment and preconditions

- Candidate source and the supported Rust toolchain.
- An owned native fixture and public Device panel for the UI lane.
- Private settings containing synthetic registry metadata. No compute key or real token.

## 5. Setup

1. Follow the native fixture runbook and retain candidate executable/source hashes.

   Result: The user's settings and Horizon processes stay unchanged.

## 6. Tasks

### 6.1 HR-01 — Scope and identity

1. Run `cargo test -p horizon-core cloud_runtime::registry::management --lib`.

   Result: Host status needs no credential files or compute account. Host verification
   records validation without creating a provider binding. Mutable tags and cancellation
   fail before Docker or provider use. The external manifest response is stubbed.

2. Run `cargo test -p horizon-core --lib cloud_runtime::registry::tests::direct_host`.

   Result: Provider and host journals retain the same generation fingerprint while
   keeping operation locks and provider IDs separate.

### 6.2 HR-02 — Actual headless interfaces

1. Run `cargo test -p horizon-core --example cloud_deploy registry_mcp`.

   Result: The real stdio tool router answers `pull_status` with `scope: host`, no
   credentials or provider IDs, and no compute key. The discovery schema remains valid.

2. From the candidate checkout, run this disposable CLI fixture:

   ```sh
   cargo build -p horizon-core --example cloud_deploy
   python3 - <<'PYTHON'
   import json
   import os
   from pathlib import Path
   import subprocess
   import tempfile

   with tempfile.TemporaryDirectory(prefix="host-registry-cli-") as directory:
       root = Path(directory)
       settings = root / "settings.json"
       settings.write_text(json.dumps({
           "runpod_key_file": str(root / "absent-compute"),
           "ssh_identity_file": str(root / "absent-identity"),
           "docker_config": str(root / "docker"),
           "registry_pull_auth_id": None, "cpu_flavors": [], "gpu_types": [],
           "registries": {"root": str(root / "registry"), "bindings": [{
               "repository": "registry.example/team/worker",
               "generation": "synthetic-generation", "publish": None,
               "pull": {"username": "reader", "secret_file": str(root / "absent-pull"),
                        "expires_at": None},
               "read_only_confirmed": True, "retired": []
           }]}
       }))
       os.chmod(settings, 0o600)
       action = root / "action.json"
       action.write_text(json.dumps({"operation": "pull_status",
           "repository": "registry.example/team/worker",
           "generation": "synthetic-generation"}))
       binary = Path("target/debug/examples/cloud_deploy").resolve()
       status = json.loads(subprocess.check_output(
           [str(binary), "registry", str(settings), str(action)], text=True))
       assert status["scope"] == "host", status
       assert status["repository"] == "registry.example/team/worker", status
       assert status["generation"] == "synthetic-generation", status
       assert status["validation"] is None, status
       assert not (root / "registry/synthetic-generation.json").exists()
       print(json.dumps(status, indent=2))
   PYTHON
   ```

   Result: JSON names the host scope, exact repository and generation. No provider
   journal is created. Missing token files do not prevent reading historical status.
   The temporary settings and journal are removed when the command finishes.

### 6.3 HR-03 — Settings behavior

1. Run `cargo test -p horizon-ui --bin horizon cloud_panel::production::setup::tests`.

   Result: An unsaved compute key does not block a host status request. Provider
   revocation remains blocked until that key is saved or cleared. Read failures
   cannot replace unknown host state with another scope's success. A failed check
   stays unverified after reading historical status, closing and reopening the
   settings form or switching board sessions during first-cloud setup, and clears
   only after fresh verification. Malformed digests and images from another saved
   repository invalidate the selected card before any background check. A stale
   request cannot clear proof for a newer grant. A worker refusal is retained
   even when settings loading fails or its receiver is replaced before polling. A fresh check displays the image that action verified. Provider
   status keeps an active provider proof when no host check exists. It does not
   save provider history as a host check. Locked and corrupt host journals keep the
   hint unknown even with an active provider proof. A later readable host status
   restores its history unless a live verification was refused. Managing a retired
   generation leaves the current generation's hint unchanged.

2. In the owned native fixture, open the container registry card without a compute account.

   Result: Validate pull access and Last pull check are available for saved bindings.
   Provider controls are in a separate section and require their compute account.

3. Request status, then validate a mutable tag and `repository@sha256:0`.
   Close and reopen settings after each refusal, and request history again.

   Result: Status says Host pull access and shows saved history or Not checked.
   Both invalid images are refused with an immutable-image action before a
   registry/provider call. The old verified hint stays absent after reopen and
   history. A new successful immutable-image check restores it.
   No real authorization is claimed by this lane.

4. Repeat in both themes and at minimum window width; record viewer video.

   Result: The actions and scope labels remain readable and reachable.

## 7. Pass criteria

All applicable tasks pass on the candidate's executable inputs. Scope, historical
validation and live issuer permission remain distinct.

## 8. Cleanup

1. Finalize and inspect video, then close the owned application and viewer.

   Result: Owned children exit and the target expires. No live worker changes.

## 9. Record of results

Record exact commit, commands, logs, frozen executable hashes and native evidence
in the PR. Report actual live checks separately from synthetic fixtures.
