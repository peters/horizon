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

## 1. Purpose

This procedure proves that approved pull requests land through one tested batch.
It also proves that the queue skips or stops on a conflict, a moved head, a moved
main and a stopped land.

## 2. Applicability

- Use a checkout of `main` that contains `scripts/merge-queue.py`.
- Platforms: Linux. The script uses only Python, git and gh.
- Do tasks 6.1 to 6.4 with real approved pull requests, because `land` merges
  them. Do tasks 6.5 and 6.6 with `--dry-run` or with pull requests that are not
  ready to merge.

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

2. Run `python3 scripts/merge-queue.py land`.

   Result: The script prints one merge commit for each pull request, in queue
   order, then "each merge has its tested tree; batch landed". The `mq/*` branch
   is gone. The tree of `main` is the tree of the batch tip.

### 6.5 SKIP — Conflict and moved head

1. Queue a pull request whose change conflicts with `main`, then run
   `python3 scripts/merge-queue.py build --dry-run`.

   Result: The script prints "skipped, conflicts with main or an earlier PR in
   the batch". The other pull requests stay in the batch.

2. Push a new commit to a queued pull request, then run
   `python3 scripts/merge-queue.py list`.

   Result: The script shows "head drifted" for that pull request. `build` skips
   it.

3. Run `python3 scripts/merge-queue.py dequeue <PR> --reason "test"`.

   Result: The label is gone. `list` does not show the pull request.

### 6.6 STOP — Moved main and stopped land

1. After a `build`, merge another pull request into `main` outside the queue, or
   wait for one. Run `python3 scripts/merge-queue.py status`.

   Result: The script prints "main moved ... build again". `land` refuses.

2. If a `land` stops after some merges, for example because a later pull request
   gets a new review thread, run `python3 scripts/merge-queue.py status`.

   Result: The script names the blocker. After you remove the blocker, `land`
   continues from the last merge and does not merge a pull request twice.

## 7. Pass criteria

- Each queued head lands as one squash commit with its queued message.
- The tree of `main` after the batch is the tree that CI tested.
- A conflict, a moved head or a dequeue keeps a pull request out of the batch.
- A moved `main` stops `build` results from landing.
- A stopped `land` continues from its last merge.

## 8. Cleanup

1. Run `python3 scripts/merge-queue.py list`.

   Result: The queue shows only the pull requests that still wait.

2. Run `git ls-remote origin 'refs/heads/mq/*'`.

   Result: Only the branch of a batch that waits to land remains.

## 9. Record of results

Put the results in the pull request. Keep private evidence out of the repository.
