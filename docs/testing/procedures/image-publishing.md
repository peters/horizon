# Test worker image destinations

This procedure tests the reusable destination policy for epic #1597.
The automatic launch path is not active yet.

1. Run the destination tests.

   ```sh
   cargo test -p horizon-cloud image_publishing
   ```

   Result: The tests pass for separate accounts, source identities, profiles and explicit registry grants.

2. Examine the malformed-input cases in `crates/horizon-cloud/src/image_publishing/tests.rs`.

   Result: The tests reject credentials, tags, digests, invalid accounts and invalid grant identities.

3. Examine the account-change test.

   Result: A stored personal destination refuses a different publishing account.

The tests use synthetic account names and registry paths. They publish no image and allocate no worker.
