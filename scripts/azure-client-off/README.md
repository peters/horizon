# Azure client-off harness

Harness for the Azure lane of the disposable-client test (#475) under #474. See
`docs/testing/azure-client-off-acceptance.md` for the runbook, manifest and
labelling rules.

- `clientoff/`: the harness modules. `manifest.py` (schema, constants, the deadline
  arithmetic that keeps the reaper away from a live run), `verdict.py` (pure
  evaluation of recorded observer samples; the offline verdict also checks that the
  journal's baseline names an Azure worker in the manifest's group), `az.py` (bounded
  `az` calls without a shell; every mutation names the exact resource and `--dry-run`
  journals instead of issuing), `observer.py` (worker descriptor gates, the forced
  reader behind the restricted observer key and the pinned read), `phases.py`
  (attestation of the exact A and B from ARM, off, observer-key install and removal,
  return) and `cleanup.py` (deletion of exactly the journaled groups, re-proven owned
  immediately before the delete and only when name and tags bind them to this run:
  A's group is `horizon-client-<run_id>` carrying the manifest's `run_id`, B's group
  is the adapter's `horizon-ws-<workflow>-<job>` carrying those identities; ARM
  offers no conditional delete for resource groups, so this rests on names no other
  run can reuse rather than on a compare-and-delete; untouched peers are proven from
  a resource inventory recorded before the run, and the whole phase runs under one
  20-minute bound).
- `client_off.py`: the command line: `validate`, `observer-key-line`,
  `install-observer-key`, `journal-group`, `bind-worker` (binds the product-created
  worker group into a manifest frozen with `worker_group: "unbound"`: adapter tags,
  pre-run absence and the descriptor's manifest digest, then the journal entry, then
  the reaper tags on B's VM read back together with its adapter identity, and
  `worker_group` last, so no mutation happens before cleanup is authorized), `off`,
  `return`,
  `remove-observer-key` (bound to its own 35-minute wall clock),
  `cleanup` (`--groups-before` and `--resources-before`, the group names and ARM
  resource IDs recorded before the run; with an unbound manifest it deletes A and
  every journaled adapter worker group absent before the run), `verdict`, driven by
  a frozen manifest that carries a `run_id` drawn when it was frozen
  (`head -c 16 /dev/urandom | od -An -tx1 | tr -d ' \n'`); `--dry-run` never issues
  a mutating call and returns the journaled plan instead of waiting for a state it
  never caused.
- `record-client-build.sh`: run in a fully clean checkout; builds the client from that
  tree for x86-64 Linux and records HEAD with the digest of the binary it produced,
  the provenance the provisioning step verifies.
- The tests run in CI (`Azure harness tests` job) next to the workspace preflight
  suite.
- `provision-client.sh`: creates client VM A in the manifest's run-named group with
  the reaper tags and the run identity under one 30-minute bound, every local file
  reserved before the first cloud call and every create reconciled by reading the
  exact resource back, and copies the exact Horizon build after checking its
  provenance. `--with-azure-cli` installs the Azure CLI from Microsoft's repository
  during cloud-init (no login) and `--assign-identity` gives the exact VM a
  system-assigned identity (no role assignment); the descriptor records both and the
  digest of the unbound manifest that `bind-worker` later compares against.
- `tests/`: deterministic coverage of the manifest gates, the verdict logic, the
  cleanup authorization, the `az` client, the observer channel, the mutation phases,
  the unbound and bound manifest states with `bind-worker` and its crash-before-bind
  cleanup, and `provision-client.sh` against a scripted control plane, run with
  `python3 -B -m unittest discover -s scripts/azure-client-off/tests -v`
  and in CI (`Azure harness tests` job).

No `az login`, provider registration or extension install happens anywhere here.
