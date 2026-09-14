# macOS keychain credential store smoke (2026-09-15)

Part of [#628](https://github.com/peters/horizon/issues/628): the OS keychain
half of the credential store, exercised against a real macOS keychain. Host: a
Mac Studio M4 Max on macOS 26.6.2 with cargo 1.97.1, a fresh worktree of `main`
at 9b10e551, and the opt-in test
`remote_browser_credential::tests::os_store_round_trip_smoke`
(`cargo test -p horizon-core os_store_round_trip_smoke -- --ignored`). The
test creates one item under a unique `remote-browser/smoke/<pid>` slot through
`KeyringCredentialStore`, probes it, reads it back through the secret sink,
deletes it, and confirms it is gone. No provider credential is involved.

## Result

| Step | Outcome |
| --- | --- |
| `KeyringCredentialStore::open()` | store available |
| Presence probe before the write | absent |
| `put` of the smoke value | stored |
| Presence probe and readiness | `Present` |
| Read back through the secret sink | matches |
| `delete`, then a dump of the keychain | no item left |

The test passed in 0.33 s once the default keychain was one the session could
write to (below). The macOS adapter uses the Apple native keyring store; the
probe is the attribute search that never fetches the secret, and the sink is
the same zeroized path the Remote browsers tab uses.

## What the session could not do, and what that means

The Mac was driven over SSH while the user was logged in at the console. In
that context every write to the login keychain fails with
`errSecInteractionNotAllowed` (-25308, "User interaction is not allowed"),
both for `security add-generic-password` and for Horizon's adapter, while
metadata-only probes still succeed. Horizon reported the write as
`Platform { kind: "platform_failure" }` rather than `Locked`, so the tab would
show the store as unavailable instead of offering the unlock-and-retry path.
That mapping is tracked in
[#660](https://github.com/peters/horizon/issues/660); it does not affect a
user working in their own desktop session, where the login keychain is
unlocked for the application.

To complete the round trip the smoke used the standard non-interactive
pattern: `security create-keychain` for a temporary keychain, unlock it, add it
to the user search list and make it the default, run the test, then restore
the login keychain as the default, restore the search list, and delete the
temporary keychain. The keychain defaults were verified restored afterwards.

## Not covered

- Starting a remote device session from the Mac (the remainder of the
  per-platform acceptance item) was not exercised; the phase 6 evidence run is
  Linux-only tooling.
- Windows credential manager: no smoke yet.
