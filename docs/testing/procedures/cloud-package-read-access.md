---
procedure: cloud-package-read-access
feature: The cloud's package read access (a read:packages device sign-in of Horizon's OAuth app, held and renewed by the worker)
platforms: [linux, macos]
cost: rents compute
destructive: yes
secrets: [the GitHub sign-in of the operator in a browser, a Hetzner or RunPod key in Cloud settings]
owner: peters
---

# Cloud package read access test procedure

## 1. Purpose

This procedure proves that a deployment asks for the cloud's package read access
right after its GitHub access, that the chain with `read:packages` reaches the
root service of the worker, and that an agent on the worker can then read the
versions of a private NuGet package. It also proves that Skip continues the
deployment with a clear reason, and that **Connect GitHub again** asks again. Unit
tests cover the refusal of a broader scope, cancellation and a worker image
without package read access.

## 2. Applicability

- Candidate: a Horizon build with the package sign-in on the cloud card, and a
  worker image with `horizon-worker-github packages` that takes a chain.
- Platforms: Linux and macOS. Provider: Hetzner or RunPod.
- Lanes:
  - Lane U: the unit tests.
  - Lane D: a deployment with a real GitHub account.
- This procedure does not test: the package rules and the renewal of the worker.
  The worker package token procedure (`worker-package-token.md`) tests them.

## 3. Safety

> **CAUTION:** DO NOT SHOW A TOKEN IN A RECORDING, A SCREENSHOT OR A REPORT. The
> chain reads every package that the account can read.

> **CAUTION:** DO NOT REVOKE HORIZON'S OAUTH APP ON GITHUB IN THE CLEANUP. That
> also ends the image publishing sign-in of Horizon.

> **CAUTION:** DELETE THE TEST CLOUDS IN THE CLEANUP. A cloud rents compute until
> somebody deletes it.

> **CAUTION:** DO NOT SHOW THE NAMES OF PRIVATE PACKAGES OR OWNERS IN PUBLIC
> EVIDENCE. Write `OWNER` and `PACKAGE` in the pull request.

## 4. Equipment and preconditions

- A clone of the candidate commit in `<repo>`.
- Cloud settings has a Hetzner or RunPod key, a working SSH identity and Connect
  GitHub, with a grant for a repository `OWNER/REPOSITORY`.
- The operator's GitHub account can read a private NuGet package `PACKAGE` of
  `OWNER`. When `OWNER` is an organization that limits OAuth apps, it granted
  Horizon.

## 5. Setup

1. Do not start Horizon on the computer of the operator for a recording. Use an
   isolated fixture (`scripts/device-smoke/serve.py`) and watch it in a native VNC
   Device panel.

   Result: The candidate Horizon runs in the fixture.

## 6. Tasks

### 6.1 U01 — Unit tests

1. Run the unit tests of the changed parts.

   ```sh
   cargo test -p horizon-cloud --lib scope_tests
   cargo test -p horizon-core --lib github::packages
   cargo test -p horizon-ui cards::github
   ```

   Result: Each command shows `test result: ok`.

### 6.2 D01 — The card asks after the GitHub access

1. Start a test cloud for `OWNER/REPOSITORY` with **Quick start**.

   Result: After the cloud's GitHub step, the card shows **Let this cloud's agents
   read packages**, a code, **Open GitHub** and **Skip: package checks need a
   person**. The text names `read:packages` only.

2. Click **Open GitHub**, paste the code and click **Authorize**.

   Result: The box closes. The deployment output has `GitHub packages: the worker
   can now read the packages of this cloud's owners for its agents.` The cloud is
   ready.

3. On the worker, as root, run `horizon-worker-github status`.

   Result: `packages` has `"state":"ok"`. The output has no token.

4. In an agent panel of the cloud, run:

   ```sh
   gh api /orgs/OWNER/packages/nuget/PACKAGE/versions --jq '.[0] | .name + " " + .created_at'
   ```

   Result: The output shows a version and a date.

5. Reconnect the cloud.

   Result: The card does not ask for the package code again.

### 6.3 D02 — Skip continues without package read access

1. Start a second test cloud for `OWNER/REPOSITORY`. When the package code shows,
   click **Skip: package checks need a person**.

   Result: The deployment continues and the cloud is ready. The output has
   `GitHub packages: This cloud has no package read access, so checks of private
   package versions need a person.`

2. Run the `gh api` command of D01 in an agent panel of this cloud.

   Result: The message starts with `Horizon: this worker has no package read
   access`.

3. Reconnect the cloud.

   Result: The card does not ask again. The output has the line of step 1.

4. Click **Connect GitHub again** on the card of this cloud.

   Result: After the cloud's GitHub step, the card shows the package code again.
   Click **Skip: package checks need a person**.

### 6.4 D03 — An organization that limits OAuth apps

1. Do this task only when an organization `RESTRICTED` that limits OAuth apps and
   did not grant Horizon owns a repository of a test cloud. Start that cloud and
   authorize the package code.

   Result: The deployment output names `RESTRICTED` and the page
   `https://github.com/organizations/RESTRICTED/settings/oauth_application_policy`.
   The line about the worker ends with `except RESTRICTED`.

## 7. Pass criteria

- Lane U shows `test result: ok`.
- D01 and D02 give each result. D03 gives its result when it applies.
- No evidence shows a token.

## 8. Cleanup

1. Delete both test clouds with **Manage › Delete cloud resources** on their
   cards.

   Result: The provider shows no server, volume or SSH key of the test clouds.

## 9. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). If not, put the results in the
pull request. Keep private evidence out of the repository.
