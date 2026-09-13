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
  `install-observer-key`, `journal-group`, `off`, `return`, `remove-observer-key`,
  `cleanup` (`--groups-before` and `--resources-before`, the group names and ARM
  resource IDs recorded before the run), `verdict`, driven by a frozen manifest that
  carries a `run_id` drawn when it was frozen
  (`head -c 16 /dev/urandom | od -An -tx1 | tr -d ' \n'`); `--dry-run` never issues
  a mutating call and returns the journaled plan instead of waiting for a state it
  never caused.
- `record-client-build.sh`: run in a fully clean checkout; builds the client from that
  tree for x86-64 Linux and records HEAD with the digest of the binary it produced,
  the provenance the provisioning step verifies.
- The tests run in CI (`Azure harness tests` job) next to the workspace preflight
  suite.
- `provision-client.sh` (a following slice): creates client VM A in the manifest's
  run-named group with the reaper tags and the run identity, and copies the exact
  Horizon build after checking its provenance.
- `tests/`: deterministic coverage of the manifest gates, the verdict logic, the
  cleanup authorization, the `az` client, the observer channel and the mutation
  phases, run with `python3 -B -m unittest discover -s scripts/azure-client-off/tests -v`
  and in CI (`Azure harness tests` job).

No `az login`, provider registration or extension install happens anywhere here.
