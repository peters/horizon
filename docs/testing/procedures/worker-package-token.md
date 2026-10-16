---
procedure: worker-package-token
feature: Package reads on a worker with a read-only package token that only root holds
platforms: [linux]
cost: rents compute
destructive: yes
secrets: [a classic GitHub token with only the read:packages scope in <evidence>/packages-token.json, the SSH identity of Cloud settings]
owner: peters
---

# Worker package token test procedure

## 1. Purpose

This procedure proves that root on a worker keeps a package token that no agent
can read, and that the API broker adds it only to package reads for the owners of
the granted repositories. An agent can then read the versions of a private NuGet
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
- This procedure does not test: how Horizon gets the token and gives it to the
  worker, or the NuGet registry. Lane W uses a classic token with only
  `read:packages` that the operator makes; any token with that scope works the
  same way.

## 3. Safety

> **CAUTION:** DO NOT SHOW THE PACKAGE TOKEN IN EVIDENCE OR IN A REPORT. It reads
> every package that its account can read.

> **CAUTION:** USE A DEDICATED TOKEN WITH A SHORT EXPIRY. Do not use the token of
> your own `gh` login. Delete the token on GitHub in the cleanup.

> **CAUTION:** DELETE THE TEST CLOUD IN THE CLEANUP. The cloud rents compute until
> somebody deletes it.

> **CAUTION:** DO NOT SHOW THE NAMES OF PRIVATE PACKAGES OR OWNERS IN PUBLIC
> EVIDENCE. Write `OWNER` and `PACKAGE` in the pull request.

## 4. Equipment and preconditions

- A clone of the candidate commit in `<repo>`.
- Python 3.11 or later on this computer.
- For lane W:
  - A GitHub account that can read a private NuGet package `PACKAGE` of the owner
    `OWNER`, and a repository `OWNER/REPOSITORY` of that owner.
  - A test cloud with GitHub connected and a grant for `OWNER/REPOSITORY`, and a
    root shell on its worker (`ssh root@<worker>`).
  - A private evidence directory `<evidence>` with mode `0700` on this computer.

## 5. Setup

1. For lane W, make a classic token on GitHub
   (**Settings › Developer settings › Personal access tokens › Tokens (classic)**).
   Select only the scope `read:packages`. Set the expiry to 7 days.

   Result: GitHub shows the token one time.

2. Write the token request to a private file. Do not write the token in a
   terminal history.

   ```sh
   (umask 077; read -rs TOKEN; printf '{"version":1,"token":"%s"}' "$TOKEN" > <evidence>/packages-token.json; unset TOKEN)
   ```

   Result: `<evidence>/packages-token.json` has mode `0600`.

3. If the image of the worker is older than the candidate, copy the candidate's
   `horizon-worker-github*` files to `/usr/local/bin` on the worker. Then stop the
   process `horizon-worker-github serve` and start it again as root with
   `setsid nohup horizon-worker-github serve </dev/null >/dev/null 2>&1 &`.

   Result: `horizon-worker-github status` on the worker shows `"packages":false`.

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

### 6.3 W02 — Root keeps the token

1. Give the token to root on the worker with stdin. Do not put it in argv.

   ```sh
   ssh root@<worker> horizon-worker-github packages < <evidence>/packages-token.json
   ```

   Result: The output is `{"packages":true}`.

2. On the worker, run `horizon-worker-github status`.

   Result: The output has `"packages":true` and no token.

3. On the worker, search the files and the environment of the agent for the
   token. Use the first eight characters of the token as `PREFIX`.

   ```sh
   horizon-worker-tailnet agent sh -c 'env; cat /workspace/home/.nuget/NuGet/NuGet.Config 2>/dev/null' | grep -c PREFIX
   grep -rlF PREFIX /workspace/home /workspace/.horizon-gate 2>/dev/null | wc -l
   ```

   Result: Both commands show `0`.

4. On the worker, examine the token files.

   ```sh
   stat -c '%a %U %n' /workspace/.horizon-root/github/packages.json /run/horizon-github/packages.json
   ```

   Result: Each file has mode `600` and the owner `root`.

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
   | `gh api /orgs/github/packages/nuget/x/versions` | `github owns no repository` |
   | `gh api /orgs/OWNER/packages/container/x/versions` | `only NuGet and npm` |
   | `gh api -X DELETE /orgs/OWNER/packages/nuget/PACKAGE/versions/ID` | `never a change of a package` |
   | `gh api /orgs/OWNER/packages` | `only NuGet and npm` |

   Result: Each command stops with an error, and the message has the text in the
   table. The package still exists on GitHub.

2. On the worker, read the last lines of `/run/horizon-github/requests.log`.

   Result: The lines of W03 have `"request":"package"` and `"granted":true`. The
   lines of the refusals have `"granted":false`. No line has the token.

### 6.6 W05 — Root removes the token

1. Give root a request without a token.

   ```sh
   printf '{"version":1,"token":null}' | ssh root@<worker> horizon-worker-github packages
   ```

   Result: The output is `{"packages":false}`.

2. Run the package read of W03 again.

   Result: The message starts with `Horizon: this worker has no package read access`.
   `/workspace/.horizon-root/github/packages.json` and
   `/run/horizon-github/packages.json` do not exist.

## 7. Pass criteria

- Lane U shows `OK`.
- W01 to W05 give each result.
- No evidence shows the token.

## 8. Cleanup

1. Delete the test cloud with **Manage › Delete cloud resources** on its card.

   Result: The provider shows no server, volume or SSH key of the test cloud.

2. Delete the token on GitHub
   (**Settings › Developer settings › Personal access tokens › Tokens (classic)**).

   Result: GitHub does not show the token.

3. Delete `<evidence>/packages-token.json`.

   Result: The file does not exist.

## 9. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). If not, put the results in the
pull request. Keep private evidence out of the repository.
