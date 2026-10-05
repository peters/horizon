# BrowserStack native-app testing

Use this runbook with [the native contract](../architecture/remote-native-app-testing.md).
The reference project is the sibling `youpark.mobile.native`, with a synthetic backend
from `youpark.no`. Read both projects' AGENTS.md before changing their workflows.

## Implementation status

The #1255 prerequisite stack provides native contracts, driver actions, provider
artifacts/tunnels, private ownership/capacity journals and managed-backend declarations.
The native project supplies executable build/backend helpers and an anonymous shell
recipe. Complete shared host orchestration, the app MCP tools, live Device presentation
and the full physical-device matrix are still required. Do not describe contract,
simulator or helper tests as an end-to-end `device_test_run` result.

## Prepare once

- Configure an App Automate profile using Horizon's credential bindings. Native
  entitlement/catalog/quota are separate from browser entitlement. Keep credentials
  in Horizon's configured store; never paste them into contracts, commands or reports.
- Before unattended work, check that the store is unlocked and the host can capture
  its bounded credential lease without a prompt. On Linux, a locked or ambiguous
  Secret Service item must fail closed. Unlocking can expire later in the session.
- Use an approved existing Mac SSH alias with Xcode, unattended access and the native
  project's ignored Firebase client configuration. The reference unsigned Debug IPA
  does not require distribution-signing credentials. Keep remote builds in unique
  directories under the approved build root and preserve unrelated CI workspaces.
- Install the local Android SDK and the project's Java version. Check the sibling
  backend's existing Docker, npm and .NET prerequisites. Reuse its shared development
  services; never reset the shared checkout or stop the shared Docker stack.
- Configure the official tunnel binary with a verified host checksum. Binary location,
  checksum, credential destination and private state root are host configuration,
  never project or MCP overrides. Retain the private journal registry with host state.

## Reference project preparation

From the selected isolated native checkout, the existing helper commands are:

```sh
python3 scripts/remote-device/build.py ios --host fintermac
python3 scripts/remote-device/build.py android
python3 -m unittest discover -s scripts/remote-device -p test_helpers.py -v
```

The helpers publish the contract's artifacts atomically and report their SHA-256.
Do not upload their logs or ignored Firebase configuration. This preparation alone
neither allocates a physical device nor establishes that the app reaches the backend.

## Concurrent execution requirements

Read the project's exact matrix and recipes. Resolve every entry against one fresh
native catalog before allocation; report unavailable entries rather than dropping them.
Build each platform once and reuse its unchanged captured artifact within its valid
lease. The reference contract has four rows and a ceiling of two concurrent sessions.
Use fresh native account/team capacity under the shared journal lock, including queued,
local pending and uncertain reservations. Quota counts alone do not identify overlap.

Each device needs its own synthetic backend worktree, database/prefix and dynamically
selected loopback port. Never share mutable seeded state between concurrent devices.
Pass a fresh private directory through `HORIZON_APP_BACKEND_DIR` and retain the guardian's
stdin cancellation pipe. Readiness requires `{"native_backend_ready":1,"port":<port>}`
after the complete backend/frontend stack and anonymous config are verified. Bind only
that device's declared loopback endpoints into its restricted tunnel and launch settings.

Cancellation or host EOF must stop the owned helper and nested groups. The helper emits
`{"native_backend_closed":1}` only after confirmed nested cleanup and worktree removal.
Missing acknowledgement holds uncertainty. Stop one lane without interrupting another;
continue other devices after a failing recipe step. Private helper logs retain at most
4 MiB while complete child output is drained.

## Evidence and completion

Verify fresh provider metadata for the actual native app, physical model and OS.
Require real requests from that app to its assigned loopback backend; the helper's own
readiness GET is not app network proof. Report each device and recipe step with its actual
outcome, duration and evidence, including skipped/unverified steps and cleanup uncertainty.

Open a task-owned native Horizon Device panel, prove displayed advancing frames during
the scenario and preserve user navigation. Agent control uses the app MCP lifecycle;
viewing uses public `device_panel`. Static screenshots and a connection flag do not qualify
live presentation. Provider-generated video/log retention is separate from local evidence.

Release exact owned sessions, app leases, tunnels and backend processes. An uncertain
reply requires bounded reconciliation of recorded intent/identity, never speculative
reallocation or account-wide cleanup. Verify success, cancellation and calling-host crash
paths. Keep every missing runtime or physical-device gate explicit until it passes.
