---
procedure: nightly-macos-release-watch
feature: Watch job of the macOS release builds workflow
platforms: [github-actions]
cost: none
destructive: no
secrets: none
owner: peters
---

# Nightly macOS release builds watch test procedure

## 1. Purpose

This procedure proves three behaviors of the Watch job in `.github/workflows/macos-release-builds.yml`:

- A failed run on `main` adds a comment on issue #1534.
- The next green run adds one "recovered" comment.
- A green run after a green run adds no comment.

## 2. Applicability

- Candidate: the workflow file and `scripts/nightly-release-watch.py` on `main`.
- Platforms: GitHub Actions on `peters/horizon`.
- This procedure does not test: the release build steps. The CI change detection tests and the build itself examine them.
- This procedure does not test: the skip gate after a failure, a cancelled run between a failure and a green run, or a re-run. `scripts/test_nightly_release_watch.py` examines these cases in Repo checks.

## 3. Safety

> **CAUTION:** DO THE TASKS ONLY ONE TIME FOR EACH CHANGE. The Watch job writes public comments on issue #1534. GitHub sends each comment to the issue author.

## 4. Equipment and preconditions

- The GitHub CLI `gh`, signed in to an account that can start workflows on `peters/horizon`.
- The workflow change is on `main`. GitHub starts `workflow_dispatch` only for a workflow file on the default branch.
- No other run of **macOS release builds** is in progress on `main`.
- Do W1 and W2 one after the other, and not between 01:45 and 03:30 UTC. A scheduled run between them adds the recovery comment before W2.

## 5. Setup

1. Count the watch comments on issue #1534:

   ```bash
   gh api --paginate repos/peters/horizon/issues/1534/comments \
     --jq '.[] | select(.body | startswith("<!-- nightly-macos-release -->")) | .html_url'
   ```

   Result: You see the list of earlier watch comments. Record the number.

## 6. Tasks

### 6.1 W1 — Failure comment

1. Start a run that fails on purpose:

   ```bash
   gh workflow run macos-release-builds.yml --ref main -f simulate_failure=true
   ```

   Result: A new run starts.

2. Wait until the run is complete:

   ```bash
   gh run watch "$(gh run list --repo peters/horizon --workflow macos-release-builds.yml --limit 1 --json databaseId --jq '.[0].databaseId')" --repo peters/horizon
   ```

   Result: The run fails. The **Simulated failure** job fails. The **Artifacts** jobs do not run. The **Watch** job is green.

3. Do the setup command again.

   Result: One new comment shows. It starts with `<!-- nightly-macos-release -->`, gives the run link and the commit, and gives `Failed jobs: Simulated failure`.

### 6.2 W2 — Recovery comment

1. Start a normal run:

   ```bash
   gh workflow run macos-release-builds.yml --ref main
   ```

   Result: A new run starts. It builds macOS arm64 and macOS x64, which can take 25 minutes or more.

2. Wait until the run is complete. Use the command in W1 step 2.

   Result: The run is green.

3. Do the setup command again.

   Result: One new comment shows. It says that the macOS release builds recovered and gives the run link.

### 6.3 W3 — Silent success

1. Wait for the next scheduled run at 02:17 UTC.

   Result: The run is green. If no commit was added to `main` after W2, the run does not build again.

2. Do the setup command again.

   Result: No new comment shows.

## 7. Pass criteria

- W1 adds exactly one failure comment with the run link, the commit and the failed job.
- W2 adds exactly one recovery comment.
- W3 adds no comment.
- No task opens an issue.

## 8. Cleanup

1. Keep the comments. They are the record of the run.

   Result: Issue #1534 shows the W1 and W2 comments.

## 9. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). If not, put the results in the
pull request. Keep private evidence out of the repository.
