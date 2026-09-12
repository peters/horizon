# Azure client-off harness

Tooling for the Azure client-off acceptance of #474 / #475: client VM A runs the
exact Horizon client, a separate worker B keeps a task running, and observer C (this
controller) proves that B keeps working while A is deallocated and that A
reconnects afterwards without a new create or a replay.

This slice ships the core; the mutation phases (off, observer install, return) and
the provisioning scripts follow in the next slice together with the runbook.

- `clientoff/`: the harness modules. `manifest.py` (schema, constants, the deadline
  arithmetic that keeps the reaper away from a live run), `verdict.py` (pure
  evaluation of recorded observer samples: cadence, A deallocated throughout, B's
  identity, image tag and endpoint constant, counter advancing, checkpoints judged
  apart), `az.py` (bounded `az` calls without a shell; every mutation names the exact
  resource and `--dry-run` journals instead of issuing) and `cleanup.py` (deletion
  of exactly the groups this run journaled, re-proven owned immediately before the
  delete, and only when their tags bind them to this run: A's group is named
  `horizon-client-<run_id>` and carries the manifest's `run_id`, B's group is the
  adapter's `horizon-ws-<workflow>-<job>` and carries those identities). ARM offers
  no conditional delete for resource groups, so this rests on names no other run can
  reuse rather than on a compare-and-delete.
- `client_off.py`: the command line: `validate`, `journal-group`, `cleanup`,
  `verdict`, driven by a frozen manifest that carries a `run_id` drawn when it was
  frozen (`head -c 16 /dev/urandom | od -An -tx1 | tr -d ' \n'`).
- The tests run in CI (`Azure harness tests` job) next to the workspace preflight
  suite.
- `tests/`: deterministic coverage of the manifest gates, the verdict logic, the
  cleanup authorization and the `az` client, run with
  `python3 -B -m unittest discover -s scripts/azure-client-off/tests -v`.

No `az login`, provider registration or extension install happens anywhere here.
