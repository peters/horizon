---
procedure: worker-package-token
feature: Package reads on a worker with a read-only token chain that only root holds and renews
platforms: [linux]
cost: rents compute
destructive: yes
secrets: [a token with only the read:packages scope from a device sign-in of Horizon's OAuth app, in <evidence>/chain.json and <evidence>/packages-chain.json, the SSH identity of Cloud settings]
owner: peters
---

# Worker package token test procedure

## 1. Purpose

This procedure proves that root on a worker keeps and renews a package token
chain that no agent can read, and that the API broker adds its token only to
package reads for the owners of the cloud's own repositories. An agent can then read the versions of a private NuGet
package and their dates. Without a token, the refusal says that the cloud has no
package read access.

## 2. Applicability

- Candidate: each candidate that changes `horizon-worker-github-packages`, the
  package rules of `horizon-worker-github-api-rest` or `horizon-worker-github-api`,
  or the `packages` operation of `horizon-worker-github`.
- Platforms: a Linux worker at a provider, for example a Hetzner cx23 from quick
  start.
- Lanes:
  - Lane U: the unit tests.
  - Lane W: a real worker, a real package token and a private NuGet package.
- This procedure does not test: how Horizon sets up the cloud's package read
  access, or the NuGet registry. Lane W gets its token from a device sign-in of
  Horizon's OAuth app with only `read:packages`, as Horizon's packages sign-in
  does.

## 3. Safety

> **CAUTION:** DO NOT SHOW THE PACKAGE TOKEN IN EVIDENCE OR IN A REPORT. It reads
> every package that its account can read.

> **CAUTION:** USE ONLY A TOKEN WITH THE SCOPE `read:packages`. Do not use the
> token of your own `gh` login or of Horizon's image publishing.

> **CAUTION:** DO NOT REVOKE HORIZON'S OAUTH APP ON GITHUB IN THE CLEANUP. That
> also ends the image publishing sign-in of Horizon. W05 rotates the test chain,
> so the copy in the evidence stops working, and the worker that holds the new
> chain is deleted in the cleanup.

> **CAUTION:** DELETE THE TEST CLOUD IN THE CLEANUP. The cloud rents compute until
> somebody deletes it.

> **CAUTION:** DO NOT SHOW THE NAMES OF PRIVATE PACKAGES OR OWNERS IN PUBLIC
> EVIDENCE. Write `OWNER` and `PACKAGE` in the pull request.

## 4. Equipment and preconditions

- A clone of the candidate commit in `<repo>`.
- Python 3.11 or later, `curl` and `jq` on this computer.
- For lane W:
  - A GitHub account that can read a private NuGet package `PACKAGE` of the owner
    `OWNER`, and a repository `OWNER/REPOSITORY` of that owner.
  - A test cloud with GitHub connected and a grant for `OWNER/REPOSITORY`, and a
    root shell on its worker (`ssh root@<worker>`).
  - A private evidence directory `<evidence>` with mode `0700` on this computer.

## 5. Setup

1. For lane W, start a device sign-in of Horizon's OAuth app with only the scope
   `read:packages`.

   ```sh
   (umask 077; curl -sS -X POST -H 'Accept: application/json' \
     -d 'client_id=Ov23liPNCPgWraJDo6XM&scope=read:packages' \
     https://github.com/login/device/code > <evidence>/device.json)
   jq -r '.verification_uri + " " + .user_code' <evidence>/device.json
   ```

   Result: The command shows an address and a code.

2. Ask the operator to open the address, type the code and click **Authorize**.

   Result: GitHub says that the device is connected.

3. Get the chain and write the token request to a private file. The token does
   not go to the terminal.

   ```sh
   (umask 077; curl -sS -X POST -H 'Accept: application/json' \
     -d "client_id=Ov23liPNCPgWraJDo6XM&device_code=$(jq -r .device_code <evidence>/device.json)&grant_type=urn:ietf:params:oauth:grant-type:device_code" \
     https://github.com/login/oauth/access_token > <evidence>/chain.json
    jq '{version: 1, client_id: "Ov23liPNCPgWraJDo6XM", repositories: ["OWNER/REPOSITORY"],
      chain: {access_token, refresh_token,
      access_expires_at: ((now | floor) + .expires_in),
      refresh_expires_at: ((now | floor) + .refresh_token_expires_in)}}' \
      <evidence>/chain.json > <evidence>/packages-chain.json)
   jq -r .scope <evidence>/chain.json
   ```

   Result: The last command shows `read:packages`. `<evidence>/packages-chain.json`
   has mode `0600`.

4. If the image of the worker is older than the candidate, copy the candidate's
   `horizon-worker-github*` files to `/usr/local/bin` on the worker. Then stop the
   process `horizon-worker-github serve` and start it again as root with
   `setsid nohup horizon-worker-github serve </dev/null >/dev/null 2>&1 &`.

   Result: `horizon-worker-github status` on the worker shows `"packages":null`.

## 6. Tasks

### 6.1 U01 — Unit tests

1. Run the unit tests of the worker.

   ```sh
   python3 -B -m unittest discover -s <repo>/examples/cloud-worker -p 'test_*.py'
   ```

   Result: The output ends with `OK`. No test is skipped.

### 6.2 W01 — A package read without a token

1. On the worker, run a package read as the agent user.

   ```sh
   horizon-worker-tailnet agent gh api /orgs/OWNER/packages/nuget/PACKAGE/versions
   ```

   Result: `gh` stops with an error. The message starts with
   `Horizon: this worker has no package read access`.

### 6.3 W02 — Root keeps the chain

1. Give the chain to root on the worker with stdin. Do not put it in argv.

   ```sh
   ssh root@<worker> horizon-worker-github packages < <evidence>/packages-chain.json
   ```

   Result: The output starts with `{"packages":{"state":"ok"`.

2. On the worker, run `horizon-worker-github status`.

   Result: The output has `"packages":{"state":"ok"` and no token.

3. On the worker, search the files and the environment of the agent for the
   token. Use the first eight characters of the token as `PREFIX`.

   ```sh
   horizon-worker-tailnet agent sh -c 'env; cat /workspace/home/.nuget/NuGet/NuGet.Config 2>/dev/null' | grep -c PREFIX
   grep -rlF PREFIX /workspace/home /workspace/.horizon-gate 2>/dev/null | wc -l
   ```

   Result: Both commands show `0`.

4. On the worker, examine the chain files.

   ```sh
   stat -c '%a %U %n' /workspace/.horizon-root/github-packages/state.json /run/horizon-github-packages/state.json
   ```

   Result: Each file that exists has mode `600` and the owner `root`. At least one
   exists.

### 6.4 W03 — The agent reads a package version and its date

1. Run the package read of W01 again, with the date of the newest version.

   ```sh
   horizon-worker-tailnet agent gh api /orgs/OWNER/packages/nuget/PACKAGE/versions --jq '.[0] | .name + " " + .created_at'
   ```

   Result: The output shows a version and a date, such as `1.2.3 2026-10-01T08:00:00Z`.

2. Read one version by its ID from step 1 (`--jq '.[0].id'`).

   ```sh
   horizon-worker-tailnet agent gh api /orgs/OWNER/packages/nuget/PACKAGE/versions/ID --jq .created_at
   ```

   Result: The output shows the same date.

### 6.5 W04 — Refusals

1. On the worker, run each of these commands as the agent user
   (`horizon-worker-tailnet agent ...`).

   | Command | The message has |
   |---|---|
   | `gh api /orgs/github/packages/nuget/x/versions` | `github owns none of this cloud` |
   | `gh api /orgs/OWNER/packages/container/x/versions` | `only NuGet and npm` |
   | `gh api -X DELETE /orgs/OWNER/packages/nuget/PACKAGE/versions/ID` | `never a change of a package` |
   | `gh api /orgs/OWNER/packages` | `only NuGet and npm` |
   | `gh api '/orgs/OWNER/packages?package_type=nuget&q=x'` | `only the query parameters` |
   | `gh api -X HEAD /orgs/OWNER/packages/nuget/PACKAGE/versions` | `only GET package reads` |
   | `gh api -H 'Range: bytes=0-9' /orgs/OWNER/packages/nuget/PACKAGE/versions` | (no error: the whole list comes back) |
   | `gh api -X GET /orgs/OWNER/packages/nuget/PACKAGE/versions -f x=1` | `only the query parameters` |

   Result: Each command stops with an error, and the message has the text in the
   table. The package still exists on GitHub.

2. List the NuGet packages of `OWNER` as the agent user.

   ```sh
   horizon-worker-tailnet agent gh api '/orgs/OWNER/packages?package_type=nuget&per_page=100' \
     --jq '[.[].repository.full_name] | unique'
   ```

   Result: The list has `OWNER/REPOSITORY` and `null` at most. It has no other
   repository.

3. On the worker, read the last lines of `/run/horizon-github/requests.log`.

   Result: The lines of W03 have `"request":"package"` and `"granted":true`. The
   lines of the refusals have `"granted":false`. No line has the token.

### 6.6 W05 — Root renews the chain

1. Give root the chain again, with an access token that the service renews at its
   next check (in 31 minutes or less it renews).

   ```sh
   jq '.chain.access_expires_at = ((now | floor) + 1860)' <evidence>/packages-chain.json \
     | ssh root@<worker> horizon-worker-github packages
   ```

   Result: The output starts with `{"packages":{"state":"ok"`.

2. Wait two minutes. On the worker, run `horizon-worker-github status`.

   Result: `packages` has a `last_refresh_at` and an `access_expires_at` about 8
   hours from now, and `last_error` is `null`.

3. Run the package read of W03 again.

   Result: The output shows a version and a date.

### 6.7 W06 — Root removes the chain

1. Give root a request without a chain.

   ```sh
   printf '{"version":1,"chain":null}' | ssh root@<worker> horizon-worker-github packages
   ```

   Result: The output is `{"packages":null}`.

2. Run the package read of W03 again.

   Result: The message starts with `Horizon: this worker has no package read access`.
   `/workspace/.horizon-root/github-packages/state.json` and
   `/run/horizon-github-packages/state.json` do not exist.

## 7. Pass criteria

- Lane U shows `OK`.
- W01 to W06 give each result.
- No evidence shows the token.

## 8. Cleanup

1. Delete the test cloud with **Manage › Delete cloud resources** on its card.

   Result: The provider shows no server, volume or SSH key of the test cloud.

2. Delete `<evidence>/device.json`, `<evidence>/chain.json` and
   `<evidence>/packages-chain.json`. Do not revoke Horizon's OAuth app on GitHub.

   Result: The files do not exist.

## 9. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). If not, put the results in the
pull request. Keep private evidence out of the repository.
