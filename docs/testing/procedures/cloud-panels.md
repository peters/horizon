---
procedure: cloud-panels
feature: Cloud panels end to end (setup, catalog, deployment, panels, lifecycle, tailnets, companions, offers, Local Network Bridge, teardown)
platforms: [linux]
cost: rents compute
destructive: yes
secrets: [RunPod API key, Hetzner Cloud API token, coding agent API keys, registry pull and push credentials, GitHub token for Git, test tailnet auth key]
owner: peters
---

# Cloud panels test procedure

## 1. Purpose

This procedure makes sure that Horizon cloud panels work from the first setup to
the last deletion. It covers 113 tests in 12 areas. Each test has an ID that a
report uses to give a result.

## 2. Applicability

- Candidate: a debug build of `origin/main`. Use it for a full smoke test of
  cloud panels, and after a change to a cloud function.
- Platforms: Linux with Xvfb. Providers: RunPod and Hetzner. Hetzner workers are
  CPU only.
- This procedure does not test:
  - macOS and Windows hosts, and their credential stores.
  - Daytona and Fly.io. These providers are design fixtures only.
  - The creation of a cloud from a CLI or MCP tool. Creation is UI only.
  - The signed project-session runtime with a tailnet. That runtime refuses a
    tailnet.

## 3. Safety

> **CAUTION:** DELETE EVERY TEST CLOUD AT THE END OF THE RUN. A RunPod network
> volume and a Hetzner volume cost money when the cloud is stopped. A Hetzner
> server costs money while it exists.

> **CAUTION:** DELETE ONLY THE RESOURCES THAT THE RESOURCE LEDGER RECORDS. Other
> workers, volumes and tailnet devices can belong to other people.

> **CAUTION:** USE ONLY A DEDICATED, PRE-AUTHORIZED TEST TAILNET AUTH KEY. Do not
> put the key in an issue, a log, a screenshot or a file in the repository.

> **CAUTION:** DO NOT PUT A SECRET IN A SCREENSHOT, A RECORDING OR A LOG. A person
> who gets a provider key can rent compute on that account.

> **CAUTION:** DO NOT CHANGE THE ACL POLICY, THE FIREWALL OR THE TAILSCALE SERVE
> SETTINGS OF THE PC. Other people use these settings to get access to the PC.

Each area file has a CAUTION before each step that rents compute, deletes a
resource, sends a secret or changes tailnet access.

## 4. Equipment and preconditions

- A Linux host with the build tools in `AGENTS.md`, Xvfb, Openbox, x11vnc,
  bubblewrap, `dbus-daemon`, `gnome-keyring-daemon`, `secret-tool`, Python 3,
  Git, Git LFS, OpenSSH, `jq` and `curl`.
- Docker with BuildKit and buildx. Rootless Docker is permitted.
- A Horizon that runs on the host and can show a Device panel in the workspace
  of the agent.
  See the [device smoke fixture](../../../scripts/device-smoke/README.md).
- Provider accounts that the operator owns:
  - A RunPod account with no Serverless endpoints.
  - A Hetzner Cloud project that only Horizon uses.
- Credentials in the current cloud settings of the operator, or in the secret
  store of the test account. Write only references in the evidence.
- A container registry. A build profile needs push credentials.
- For each image-only profile, a public worker image that reports all contract
  markers. Pin the image by digest, for example `<registry>/<image>@sha256:<digest>`.
  A private image needs a pull credential that can read it.
- A worker image that reports all contract markers, with
  `horizon-tailnet-contract=1`. The image must contain Claude, Codex, a browser
  and a desktop.
- A test tailnet with a reusable, pre-authorized, non-ephemeral auth key. For the
  tests from PC to cloud, the PC must be on the same tailnet.
- A private evidence directory, `<evidence>`, outside the repository.
- Permission from the operator to rent compute, with a cost limit and a time
  limit for cleanup.

### 4.1 Names in this procedure

| Name | Meaning |
|---|---|
| `<run>` | The run directory. It is outside `$HOME` and not on a tmpfs with a user quota. |
| `<state>` | The state directory of the persistent launcher, `<run>/fixture`. |
| `<home>` | The real home path. Inside the fixture, the private home of the candidate shows at this path. |
| `<data-home>` | The host path of the private home, `<state>/data/home`. |
| `<repo>` | The primary synthetic repository, `<home>/smoke/app`. |
| `<lib>` | The companion synthetic repository, `<home>/smoke/lib`. |
| `<sib>` | The synthetic repository of the sibling, `<home>/smoke/sib`. |
| `<evidence>` | The private evidence directory. |
| `<ledger>` | The resource ledger, `<evidence>/resource-ledger.tsv`. |
| `<uid>` | The numeric user ID of the operator on the host. |
| `<docker-socket>` | The socket of the rootless Docker daemon of this run, for example `<run>/docker/docker.sock`. |
| `<display>` | The X display of the fixture, from `display` in `<state>/lab.json`. |
| `<x>`, `<y>` | Screen coordinates from a fresh screenshot of the fixture. |
| `<launcher-pid>` | The process ID of the persistent launcher, from `pids` in `<state>/lab.json`. |
| `<test-owner>` | The GitHub owner of the test account. |
| `<build-repository>` | A registry repository that the `runpod-build` profile can push to. |
| `<image>`, `<digest>` | The name and the digest of the private test image for A08, in the GHCR space of `<test-owner>`. |
| `<root shell>` | A root SSH session on a worker. Start it with the command of E09 step 4 without `true`. |

The [technical names](../../style/technical-names.md) define the terms fixture
terminal, worker shell, persistent launcher, restart marker and resource ledger.

### 4.2 Planned clouds

The tests use these clouds. You can use fewer clouds. If you do, record the
change in the report as a deviation.

| Cloud title | Provider and profile | Tailnet | Tests |
|---|---|---|---|
| `smoke-a` | Hetzner, CPU, image-only | test tailnet | C31, D01, D05, E01–E09, A09, T03–T08, T10–T14, G01–G10, G12, N01–N05, L01–L04, L09, O02 |
| `smoke-b` | Hetzner, CPU, image-only | test tailnet | T08, T14 |
| `smoke-r` | RunPod, CPU, network volume | test tailnet | C31, D02, D04, T09, L06, L10 |
| `smoke-g` | RunPod, GPU | None | D03 |
| `smoke-sib` | RunPod, CPU, `runpod-build` with the sibling `sib` | None | C10, G02, G11, L05 |
| `smoke-lib` | Hetzner, CPU, companion repository `<lib>` | None | G01, G03–G10, G12 |
| `smoke-lib0` | Hetzner, CPU, companion repository `<lib>`, no worker | None | G08 |
| `smoke-x` | Hetzner, CPU, image-only, idle stop of 10 minutes | None | L07, L08 |

### 4.3 Rules for each step

1. Do one test at a time.
2. Take a fresh screenshot before each click. Do not use old coordinates.
3. After a dialog opens, wait 3 seconds. Then take a new screenshot before you
   click ([issue #1297](https://github.com/peters/horizon/issues/1297)).
4. The operator enters each real secret. Device `type` actions can lose
   characters at action boundaries
   ([issue #1301](https://github.com/peters/horizon/issues/1301)).
5. Until the candidate contains the fix for issue #1301, do not type a real
   secret with a device action.
6. Write each new provider resource in the resource ledger when the cloud card
   shows its ID. Record the provider, the type, the ID, the cloud title and the
   UTC time.
7. Give each test a result: pass, fail or blocked. An interim report can also
   use `not run`.
8. For a fail, open a bug issue and write its link in the report.
9. Put long commands for the fixture terminal in a script file below
   `<data-home>/smoke/bin`. Then type only the short command that starts the script.
10. To open the panel picker inside a cloud frame, use a real Ctrl-double-click.
    Two separate device click actions are not a double-click.
11. If `cloud_deploy` shows `Another controller owns this cloud operation`, wait
    10 seconds and run the same command again. Do not stop the candidate.

## 5. Setup

1. Do the tasks of [area S](cloud-panels/s-test-fixture.md).

   Result: The candidate runs in the persistent launcher. A Device panel shows a
   live view.

   > **CAUTION:** ONLY THE OPERATOR WRITES THE HEADER FILES. Each file contains a
   > provider credential of the test account. Do not show the files.

2. Ask the operator to write two header files with mode `0600` from the credentials of the test accounts.

   ```text
   <run>/hetzner.header: Authorization: Bearer <Hetzner token>
   <run>/runpod.header:  Authorization: Bearer <RunPod key>
   ```

   Result: The two files exist. No command argument contains a credential.

3. Write a script that reads all pages of one Hetzner list.

   ```sh
   cat > <run>/hetzner-list.sh <<'EOF'
   #!/usr/bin/env bash
   # Usage: hetzner-list.sh servers|volumes|ssh_keys|networks|server_types [full]
   set -euo pipefail
   kind=$1; full=${2:-}; page=1
   while [ "$page" != null ]; do
     body=$(curl -fsS -H @<run>/hetzner.header "https://api.hetzner.cloud/v1/$kind?per_page=50&page=$page")
     jq -c --arg k "$kind" --arg f "$full" '.[$k][] | if $f == "full" then . else {kind: $k, id, name} end' <<< "$body"
     page=$(jq -r '.meta.pagination.next_page' <<< "$body")
   done
   EOF
   ```

   Result: The script follows `meta.pagination.next_page` until it is null. An HTTP
   error stops it. With `full`, it writes each complete object.

4. Write a script that reads all pages of one RunPod list.

   ```sh
   cat > <run>/runpod-list.sh <<'EOF'
   #!/usr/bin/env bash
   # Usage: runpod-list.sh pods|network-volumes|registries
   set -euo pipefail
   url="https://api.runpod.io/v2/$1"; next=$url
   while :; do
     body=$(curl -fsS -H @<run>/runpod.header "$next")
     jq -c --arg k "$1" '(if type == "array" then . else (.pods // .networkVolumes // .registries // []) end)[] | {kind: $k, id, name}' <<< "$body"
     more=$(jq -r 'if type == "object" then (.pagination.hasNextPage // false) else false end' <<< "$body")
     [ "$more" = true ] || break
     next="$url?cursor=$(jq -r '.pagination.nextCursor | @uri' <<< "$body")"
   done
   EOF
   ```

   Result: The script follows `pagination.nextCursor` while `hasNextPage` is true. An HTTP error stops it.

   > **CAUTION:** SEND THE HETZNER TOKEN ONLY TO THE HETZNER API. The header file
   > contains the token. Do not show the file or the request headers.

5. Save the Hetzner baseline.

   ```sh
   for k in servers volumes ssh_keys networks; do bash <run>/hetzner-list.sh "$k"; done > <evidence>/hetzner-before.jsonl
   ```

   Result: The file has one line for each Hetzner resource before the run.

   > **CAUTION:** SEND THE RUNPOD KEY ONLY TO THE RUNPOD API. The header file
   > contains the key. Do not show the file or the request headers.

6. Save the RunPod baseline.

   ```sh
   for k in pods network-volumes registries; do bash <run>/runpod-list.sh "$k"; done > <evidence>/runpod-before.jsonl
   ```

   Result: The file has one line for each RunPod resource before the run.

7. Make the resource ledger with one header line.

   ```sh
   printf 'utc\tprovider\ttype\tid\tcloud\tstate\n' > <evidence>/resource-ledger.tsv
   ```

   Result: The resource ledger exists and has no resources.

   > **CAUTION:** ONLY THE OPERATOR SIGNS IN THE LOCAL AGENT. Use a test account.
   > Do not type a password or a key with a device action.

8. In the fixture, open a local Claude Code panel and let the operator sign it in.

   Result: The agent answers a short request. O01, G07, G08, T12 and T13 use this agent.

## 6. Tasks

Some tasks need a cloud or a setting from a later area. Do the tasks in this
order. The L area stops and deletes clouds that the T, G and N areas use.

1. Do A01 to A05, A07 and A08. The setup of this procedure did S01 to S05.
2. Do B01 to B05, then do A06.
3. Do C01 to C30. Do not do step 5 of C02 or the task C09 yet.
4. Do O01 and O03.
5. Do T01 and T02, then do C09. D01 needs the saved test tailnet.
6. Do D01 to D05. D01 and D02 select the places that C31 examines.
7. Do step 5 of C02, then C31 and A09.
8. Do E01 to E09, then O02. O02 uses the Claude Code panel of E02.
9. Do T03 to T11 and T14.
10. Do G01 to G12, then do T12 and T13. Then do the cleanup of area G.
11. Do N01 to N05.
12. Do L01 to L10.

The cleanup of this procedure does X01 to X05.

| Area | File | Tests | Cost |
|---|---|---|---|
| S — Test fixture | [s-test-fixture.md](cloud-panels/s-test-fixture.md) | S01–S05 | none |
| A — Machine setup and credentials | [a-machine-setup.md](cloud-panels/a-machine-setup.md) | A01–A09 | A09 rents compute |
| B — Repository configuration | [b-repository-configuration.md](cloud-panels/b-repository-configuration.md) | B01–B05 | none |
| C — New cloud dialog | [c-new-cloud-dialog.md](cloud-panels/c-new-cloud-dialog.md) | C01–C31 | C31 rents compute |
| D — Deployment | [d-deployment.md](cloud-panels/d-deployment.md) | D01–D05 | rents compute |
| E — Panels in a cloud | [e-panels.md](cloud-panels/e-panels.md) | E01–E09 | rents compute |
| L — Lifecycle | [l-lifecycle.md](cloud-panels/l-lifecycle.md) | L01–L10 | rents compute |
| T — Tailnets | [t-tailnets.md](cloud-panels/t-tailnets.md) | T01–T14 | T03–T12 and T14 rent compute |
| G — Companion repositories | [g-companions.md](cloud-panels/g-companions.md) | G01–G12 | G08 is free |
| O — Offers and cost | [o-offers.md](cloud-panels/o-offers.md) | O01–O03 | O02 rents compute |
| N — Local Network Bridge | [n-local-network-bridge.md](cloud-panels/n-local-network-bridge.md) | N01–N05 | rents compute |
| X — Teardown | [x-teardown.md](cloud-panels/x-teardown.md) | X01–X05 | none |

## 7. Pass criteria

- Each test is pass, or it has a linked defect or a recorded block.
- The candidate child that runs has the same SHA-256 as the frozen candidate.
- The provider APIs show no server, pod, volume or SSH key from the resource
  ledger after X02 and X05.
- No screenshot, recording, log, issue or report shows a secret.

## 8. Cleanup

1. Do the tasks of [area X](cloud-panels/x-teardown.md).

   Result: The provider APIs show no resource from the resource ledger.

2. Compare the provider lists of X02 and X05 with the baselines from the setup.

   Result: The lists are the same as the baselines. The Hetzner network of
   Horizon can stay. The resource ledger records it as kept.

   > **CAUTION:** DELETE ONLY THE STATE DIRECTORY OF THIS RUN. It contains the saved
   > provider keys and the private data of the fixture.

3. Delete the state directory of the fixture and the keyring password file.

   ```sh
   rm -r <run>/fixture && rm -f <run>/keyring-password
   ```

   Result: `<run>` contains no credential file. Keep `<evidence>` outside `<run>`.

## 9. Record of results

Write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). Use the test IDs of the area
files. Keep private evidence, provider IDs and tailnet addresses out of the
repository.
