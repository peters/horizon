# Saved tailnet catalog fence

This procedure tests saved network removal and allocation with synthetic state.
The tests do not use a provider, an auth key, or the OS credential store.

## Requirements

- Use the exact candidate checkout.
- Use a private test directory on persistent disk.
- Keep the Rust compiler slot free before the test run.

Cloud lifecycle ownership currently requires Unix directory durability.
Windows tests check refusal for existing clouds before state or credential mutation.
The cloud ownership tests declare an ignore reason on Windows.
The portable catalog lock tests run on Windows.

## Procedure

1. Set `TMPDIR` to the private test directory.
2. Run `cargo test -p horizon-cloud tailnet::bindings::tests`.
3. Run `cargo test -p horizon-core cloud_runtime::tailnet::catalog::tests`.
4. Run `cargo test -p horizon-core cloud_runtime::companions::lifecycle::tests::tailnets`.
5. Run `cargo fmt --all -- --check`.
6. Run `./scripts/check-maintainability.sh`.
7. Run `python3 scripts/check-horizon-mcp-skills.py`.
8. Save the test logs with the exact source hash.

## Required results

The concurrent tests stop a writer and a remover at declared barriers.
The remover keeps all cloud locks until credential deletion stops.
A writer on an existing cloud returns Busy during that interval.
A new cloud selection waits for catalog ownership and then refuses a missing ID.
No denied path calls the synthetic provider counter or the credential sink.

Remove reads changed selections and allocation state after initial cloud discovery.
A new cloud before catalog ownership makes Remove return Busy.
A pending request keeps its network when the current selection names another network.
This rule includes a request whose target claim does not yet exist.
Malformed metadata and uncertain provider state keep the saved credentials.
An unresolved catalog journal blocks owned catalog reads.
The journal and credentials stay intact until Settings recovery settles that generation.

Allocation refuses a selected network whose saved ID no longer exists.
A failed recovery keeps its original selection, request, and commit marker.
It makes zero provider decisions.
An explicit `none` request keeps the existing recovery path.

## Limits

The catalog lock covers local state checks and durable local writes.
It does not cover a provider network request.
Remove can return Busy for any active cloud operation in the catalog.
The tests do not prove administrator device deletion, enrollment receipt cleanup,
OAuth binding, or successful allocation on a live provider.
