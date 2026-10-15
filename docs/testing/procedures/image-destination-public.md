---
procedure: image-destination-public
feature: Host-scoped image destination operations
platforms: [linux, macos, windows]
cost: none
destructive: no
secrets: synthetic-only
owner: peters
---

# Public image destination operations

## 1. Purpose

Check shared destination policy through the normal CLI plan runner, stdio MCP
server and an isolated desktop host. This procedure allocates no worker.

## 2. Applicability

Protocol and parser tests run on all CI platforms. The native host fixture uses
Linux X11. Windows cloud operations remain unsupported. These checks do not prove
real sign-in, registry publication or issuer permission.

## 3. Safety

Use synthetic metadata and a private home. Do not use installed credentials,
start a real sign-in, publish an image or allocate compute.

## 4. Equipment and preconditions

- A task checkout and the repository Rust prerequisites.
- A frozen Horizon binary and normal `horizon-browser` CLI/MCP binary.
- An isolated native desktop with a public Horizon Device viewer.
- A synthetic Git checkout with an image-omitting build profile.
- Private cloud settings with the staging setting enabled and a synthetic registry
  binding. Credential files must be absent.

## 5. Setup

1. Run `localci check` and examine `localci plan --json`.

   Result: Select the applicable repository, lint and Rust test lanes.

2. Run the isolated fixture from `scripts/device-smoke/README.md`.

   Result: Record the candidate inputs, frozen executable hashes and actual child
   PID. Establish native presentation and motion through three public inspections.

3. Register a local agent panel in the synthetic checkout.

   Result: The host owns its launch directory. Keep Settings closed.

## 6. Tasks

For every action, retain its operation ID. If the receipt is pending, poll
`status` with that ID and no profile until `done` is true; assess the nested
result or error before sending the next mutation.

### 6.1 IP-01 — Public contract

1. Run the protocol tests.

   ```sh
   cargo test -p horizon-browser-control
   cargo test -p horizon-browser-mcp
   cargo test -p horizon-browser-cli
   ```

   Result: The actual CLI plan and stdio MCP routes expose
   `cloud_image_destination`. Listing and authentication prompt readback pass.
   Source, settings authority and secret fields are refused before queuing.
   The existing browser tools, including zoom, remain available.

### 6.2 IP-02 — Local host ownership

Run `cargo test -p horizon-ui --bin horizon cloud_membership`.

Result: Saved cloud membership refuses new operations before live groups are
restored. Both saved and live membership invalidate retained operation owners;
cancellation settles without changing source or credentials.

1. Run the shared manager and actual host adapter tests.

   ```sh
   cargo test -p horizon-core --lib cloud_runtime::publishing::operations
   cargo test -p horizon-ui --bin horizon cloud_panel::image_destinations
   ```

   Result: A local agent can list, select and reset its exact checkout/profile.
   A registered subdirectory resolves to the Git root on a background worker.
   Expired, foreign-host, foreign-agent and remote/SSH requests refuse.
   Source YAML stays unchanged and no credential store or compute resource is made.

2. On the first and final committed public heads, run the ignored real-checkout
   listing with `HORIZON_TEST_REPOSITORY` set to the real Horizon checkout.

   ```sh
   cargo test -p horizon-ui --bin horizon real_repository_public_listing_from_a_registered_subdirectory_is_read_only -- --ignored
   ```

   Result: Its registered `crates/horizon-core` folder resolves to the actual root.
   The real `cpu` profile is read through private synthetic host settings.
   Source YAML and private settings bytes stay unchanged. No secret or compute
   credential is read. Keep these logs separate from synthetic protocol evidence.

### 6.3 IP-03 — Actual host and interfaces

1. Use a CLI plan to list the synthetic agent's named profile.

   Result: A receipt reports only nonsecret metadata and listed registry choices.

2. Through normal stdio MCP, select a listed registry with the exact
   `shown_override` returned by listing.

   Result: Only the private machine preference changes. The selected binding is
   the existing exact grant, not a newly inferred credential.

3. Use a stale snapshot to reset, then use the current snapshot.

   Result: The stale action refuses. The current action removes only its override.
   Listing and an unknown-profile request return their correct result or refusal.

4. Repeat from a registered checkout subdirectory and a second local actor.

   Result: The first actor resolves the same Git root. Another actor cannot read
   back, cancel or replay the first actor's receipt.

5. Run the deterministic cancellation and disabled-setting tests.

   Result: A receipt retains its actor, host, settings root and registered path.
   Lost scope, lack of sign-in observation and lifetime limits request cancellation.
   Active jobs schedule polling even when the terminal has no output.
   The default-off setting refuses mutations before activation. Disabling it
   while mocked authentication waits refuses the preference commit and preserves
   the disabled settings bytes.

## 7. Pass criteria

All applicable tasks pass on the candidate inputs. Keep native host proof,
protocol fixtures, local matrix and exact-head cross-platform CI separate.
No result qualifies a real registry, account or worker.

## 8. Cleanup

1. Close the exact owned application normally and close its public viewer.

   Result: Owned children exit, the target expires and the private home is removed.
   The user's Horizon and installed account settings remain unchanged.

## 9. Record of results

Record commands, inputs, hashes, first/final real-source checks, public CLI/MCP
responses and owned cleanup. Keep private evidence outside the fixture home.
Record unexecuted platforms and real authentication explicitly.
