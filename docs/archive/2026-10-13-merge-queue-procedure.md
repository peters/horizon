---
procedure: merge-queue
feature: Merge queue for approved pull requests (scripts/merge-queue.py)
platforms: [linux]
cost: none
destructive: yes      # land merges pull requests into main and deletes the batch branch
secrets: the gh login of the repository owner
owner: peters
---

# Merge queue test procedure

> **Archived 2026-10-13.** The merge queue was removed by #1547, which reverted #1544. This procedure is kept as a record. It does not apply to the repository any more.

## 1. Purpose

This procedure proves that approved pull requests land through one tested batch.
It also proves that the queue skips or stops on a conflict, a moved head, a moved
main and a stopped land.

## 2. Applicability

- Use a checkout of `main` that contains `scripts/merge-queue.py`.
- Platforms: Linux. The script uses only Python, git and gh.
- Queue only pull requests that have merge permission and pass every merge gate,
  in every task. Do tasks 6.5 and 6.6 only with `--dry-run` or by observation
  when the condition occurs. Do not cause a conflict, a push or a merge to test
  them.

## 3. Safety

> **WARNING: LAND MERGES INTO MAIN.** Queue only heads that have merge permission
> from `peters` and pass every merge gate in AGENTS.md.

> **CAUTION: DO NOT MERGE OUTSIDE THE QUEUE WHILE A BATCH LANDS.** The script
> finds such a merge but cannot prevent it.

## 4. Equipment and preconditions

- `gh auth status` shows the account of the repository owner.
- Two or more open pull requests into `main` that meet every merge gate, each
  with a hand-written squash message in a file.
- "Require branches to be up to date before merging" is off for `main`.

## 5. Setup

1. Run `python3 scripts/merge-queue.py list`.

   Result: The script prints the current queue, or "The merge queue is empty."

## 6. Tasks

### 6.1 ENQUEUE — Queue approved heads

1. For each pull request, run
   `python3 scripts/merge-queue.py enqueue <PR> --head <sha> --subject "<subject>" --body-file <file>`.

   Result: The script prints "#<PR> queued at <sha>". The pull request has the
   `merge-queue` label and a comment that starts with `<!-- merge-queue v1 -->`.

2. Run the same command with a subject that contains a closing keyword, for
   example "Fixes #1".

   Result: The script refuses the entry and names the closing keyword. Nothing
   changes on the pull request.

### 6.2 BUILD — Build and test one batch

1. Run `python3 scripts/merge-queue.py build --dry-run`.

   Result: The script prints the batch branch name, the base, the tip and the
   pull requests in queue order. It prints "dry run: nothing pushed".

2. Run `python3 scripts/merge-queue.py build`.

   Result: The script pushes `mq/<UTC time>`. The CI workflow starts on the tip.
   The release artifact jobs do not run.

### 6.3 STATUS — Wait for CI

1. Run `python3 scripts/merge-queue.py status` while CI runs.

   Result: The script names each open job of the batch tip.

2. Run the command again after CI is complete.

   Result: The script prints "ready to land".

### 6.4 LAND — Land the batch

1. Run `python3 scripts/merge-queue.py land --dry-run`.

   Result: The script prints the pull requests that it would merge.

2. > **CAUTION: THIS STEP MERGES EACH PULL REQUEST INTO MAIN AND DELETES THE
   > BATCH BRANCH.** Do this step only for heads that have merge permission.

   Run `python3 scripts/merge-queue.py land`.

   Result: The script prints one merge commit for each pull request, in queue
   order, then "each merge has its tested tree; batch landed". The `mq/*` branch
   is gone. The tree of `main` is the tree of the batch tip.

### 6.5 SKIP — Conflict and moved head

1. When a queued pull request conflicts with `main` or with an earlier queued
   pull request, run `python3 scripts/merge-queue.py build --dry-run`.

   Result: The script prints "skipped, conflicts with main or an earlier PR in
   the batch". The other pull requests stay in the batch.

2. When a queued pull request gets a new commit, for example a review fix, run
   `python3 scripts/merge-queue.py list`.

   Result: The script shows "head drifted" for that pull request. `build` skips
   it.

3. When a queued pull request must leave the queue, run
   `python3 scripts/merge-queue.py dequeue <PR> --reason "<reason>"`.

   Result: The label is gone. `list` does not show the pull request.

### 6.6 STOP — Moved main and stopped land

1. Do not merge into `main` to cause this test. If a merge that `peters`
   approved separately, or a Dependabot merge, reaches `main` after a `build`,
   run `python3 scripts/merge-queue.py status`.

   Result: The script prints "main moved ... build again". `land` refuses.

2. If a `land` stops after some merges on a blocker, for example a new review
   thread on a later pull request or "did not merge ... Run land again to
   resume", run `python3 scripts/merge-queue.py status`.

   Result: The script records a merge that GitHub made but `land` did not
   confirm, if its tree is the tested tree, and names each blocker. After you
   remove the blocker, `land` continues from the last merge and does not merge a
   pull request twice.

3. If a `land` stops with "MISMATCH", run `python3 scripts/merge-queue.py status`.

   Result: The script prints "an earlier land stopped on a mismatch". `land`
   refuses until a new `build`. Check main CI before you build again.

## 7. Pass criteria

- Each queued head lands as one squash commit with its queued message.
- The tree of `main` after the batch is the tree that CI tested.
- A conflict, a moved head or a dequeue keeps a pull request out of the batch.
- A moved `main` stops `build` results from landing.
- A `land` that stops on a blocker continues from its last merge. A `land` that
  stops on a mismatch refuses until a new `build`.

## 8. Cleanup

1. Run `python3 scripts/merge-queue.py list`.

   Result: The queue shows only the pull requests that still wait.

2. Run `git ls-remote origin 'refs/heads/mq/*'`.

   Result: Only the branch of a batch that waits to land remains.

## 9. Record of results

Put the results in the pull request. Keep private evidence out of the repository.
