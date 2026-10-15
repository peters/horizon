---
procedure: image-publication-credentials
feature: image publication credential isolation
platforms: [linux, macos, windows]
cost: none
destructive: no
secrets: none
owner: peters
---

# Image publication credential test procedure

## 1. Purpose

This procedure checks account identity and temporary Docker logins for one publication operation.
The tests use synthetic tokens. They do not change an installed login.

## 2. Applicability

- Candidate: The host publication preparation helper for epic #1597.
- Platforms: Linux, macOS and Windows.
- This helper has no launch caller yet. Existing launches keep their behavior.
- This procedure does not test a live registry push, UI selection or cloud allocation.

## 3. Safety

Use synthetic loopback responses and disposable credential stores only. Never
provide a real token or complete a real account sign-in for these fixtures.

## 4. Equipment and preconditions

- A candidate checkout and the supported Rust toolchain.
- No account, registry credential or running Horizon is required.

## 5. Setup

1. Open a terminal in the candidate checkout.

   Result: `Cargo.toml` names the Horizon workspace.

## 6. Tasks

### 6.1 IC-01 — Check provider account identity

1. Run the provider identity tests.

   ```sh
   cargo test -p horizon-cloud github
   ```

   Result: A user has a nonzero provider ID. An answer with ID zero is refused.

### 6.2 IC-02 — Refuse another publisher

1. Run the host publication tests.

   ```sh
   cargo test -p horizon-core publication::tests --lib
   ```

   Result: A reused name with another user ID is refused. Different letter case retains the same account.

### 6.3 IC-03 — Keep operation logins separate

1. Examine the two-operation test result from IC-02.

   Result: Each operation has its own Docker configuration and token. The shared configuration stays unchanged.

2. Examine the cleanup assertions.

   Result: Dropping one operation removes only its temporary configuration. The other operation keeps its login.

### 6.4 IC-04 — Check cancellation, permissions and renewal

1. Examine the cancellation results from IC-02.

   Result: An already cancelled operation creates no login folder or prompt. Cancellation
   after authentication removes the prepared configuration and returns an error.
   A cancelled revoked-token lookup makes no new sign-in request.
   A cancelled device-code lookup displays no authorization prompt.
   Cancellation during token polling or account lookup ends the displayed prompt and saves no credential chain.

2. Examine the permission and metadata results.

   Result: On Unix, the configuration directory uses mode 0700 and its file uses mode 0600.
   Account JSON contains no token. The secret diagnostic value hides the token.

3. Examine the renewal results.

   Result: Token rotation retains the provider ID and saves the new refresh token before use.
   A private operation login accepts a new token for the same account. Another account
   leaves its configuration unchanged.

### 6.5 IC-05 — Recover the retained account

1. Examine `a_recorded_account_can_sign_in_without_accepting_or_erasing_another_actor`
   from IC-02.

   Result: A cached account B does not make a retained account A impossible to
   recover. The helper emits a device prompt for A. A response with A's reused
   login but another immutable ID is refused without saving that chain or erasing
   B. The correct account A prepares its private login and saves its chain.

2. Examine `renewal_recovers_only_the_recorded_account_without_replacing_another_login`.

   Result: The actual preparation and renewal entry paths share the recovery
   boundary. A wrong identity ends the displayed prompt with refusal, preserves
   the prior chain and leaves the operation-private login unchanged. A correct
   identity renews only the retained operation's login.
   The explicit `select_account` helper forces device sign-in without reusing the
   cached account. No launch caller or visible account selector is activated.

3. Examine the cached renamed-account fixture.

   Result: The same immutable account under a changed namespace is refused with
   a rename explanation before another device prompt. Expected-account sign-in
   cancellation still ends the prompt without saving a chain.

## 7. Pass criteria

- IC-01 through IC-05 pass.
- Another account cannot replace the expected publisher.
- No token enters serialized account metadata or diagnostic output.
- An operation owns its temporary Docker login until it finishes.

## 8. Cleanup

The tests remove their temporary configurations. They create no external resource and change no installed credential.
The normal Cargo cache can remain.

## 9. Record of results

Put the candidate commit, toolchain, commands and IC task results in the pull request.
Keep private account data out of public evidence.
