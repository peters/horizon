-- The schema of the secrets file of one Horizon home.
--
-- This file is the one place that says how a secrets file is made. The Rust
-- store of `crates/horizon-core/src/secret_store/` includes it, and the smoke
-- fixture `scripts/device-smoke/serve.py` runs it with Python's `sqlite3`, so a
-- file that the fixture seeds is a file that Horizon reads.
--
-- A row holds one secret value as it is: the id of the value, its bytes, the
-- version of this row and when it was written. The file mode is the protection;
-- see `docs/review/trust-boundaries/named-secrets.md`.

CREATE TABLE secrets (
    id TEXT PRIMARY KEY,
    body BLOB NOT NULL,
    version INTEGER NOT NULL,
    updated_millis INTEGER NOT NULL
) STRICT;

CREATE TABLE meta (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
) STRICT;
