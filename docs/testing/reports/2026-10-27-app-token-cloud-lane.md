---
procedure: github-app-installation-token.md
candidate_commit: 837d84b4ac22e4b06589b46589e85df23f4fd1cf
candidate_sha256: 74272d72339223d3033910d81d87fddec652a9369d598187fdd75b49a87c1a09
date: 2026-10-27
lanes: [agent-cloud, github-app-token]
---

# GitHub App installation-token cloud lane, 2026-10-27

> **This lane was removed afterwards.** Horizon removed the GitHub App installation-token path later on 2026-10-27, because an installation token lasts one hour, and this report is kept as the history of a run that happened.

## 1. Summary

An agent rented a cloud, ran commands in it, committed, pushed, opened a pull request, merged it
and deleted the cloud. Every step used a Horizon MCP tool. No person signed in to GitHub at any
point. The worker acted as `horizon-for-peters[bot]`, which is the bot of this machine's GitHub
App.

The run covered the create, exec, commit, push, pull request, merge and delete tasks of the
procedure. It did not cover step 6 and step 8 of that procedure, and section 4 says why. The run
found three defects before it passed. All three are fixed and merged.

## 2. Results

| Task ID | Result | Note | Defect |
|---|---|---|---|
| create | pass | `cloud_create` reached `ready`. The answer said the worker holds an installation token as `horizon-for-peters[bot]` for one repository. | — |
| exec | pass | `cloud_exec` answered as uid 10001 with exit status 0 and found `/workspace/checkout`. | — |
| commit-push-pr | pass | The worker committed `e481ba1`, pushed branch `app-token-cd22631b` and opened pull request 1. The author was `app/horizon-for-peters`. | #1887, #1891 |
| merge | pass | The worker merged pull request 1 with a squash merge. GitHub answered `mergedAt 2026-10-27T14:20:23Z`, merge commit `20cbd71d`. | — |
| delete | pass | `cloud_delete` answered `absent: true` at the phase `done`. The provider showed the worker as absent. | — |
| image pin | pass | The pinned base image accepted the payload. Before the fix, the worker refused it and the create failed at `Start sessions`. | #1890, #1892 |
| step 6, worker status | not run | The worker's chain status is a root-only command, and an agent panel has no root in the worker. The create answer named the token the worker holds, as the note in section 6 shows. | — |
| step 8, token limit | not run | This step needs the same cloud after the merge, and the cloud was deleted. The same limit was measured on its own, outside a cloud: a token minted for one repository read that repository with `200` and another repository of the same installation with `404`. | — |

## 3. Defects

- #1887: The token request named a repository as `owner/name`. GitHub refused it with `422`. The
  request must name the repository by its name alone.
- #1890 and #1892: The quick-start image pin was older than the worker-side support for an
  installation-token chain. The worker refused the payload, and the create failed with
  `Private runtime upload failed`. The upload sends no reason to any log.
- #1891: The worker refused the login `horizon-for-peters[bot]` as `Invalid GitHub login`. A
  GitHub App token acts as that bot account.

## 4. Deviations from the procedure

- Step 4 was not used. The policy refused a repository the app does not reach before this run,
  as step 4 requires.
- The merge step passed `--delete-branch`. The merge succeeded, and the branch delete failed
  because the branch is the checkout of the worker's own worktree. The exit status is 1, and the
  pull request is still merged. A later procedure must not pass that option.
- `cloud_exec` accepts a timeout of at most 120000 ms. The procedure must not ask for more.
- Step 6 did not run. `horizon-worker-github status` needs root inside the worker, which an
  agent panel does not have. A later procedure must either name a root path or use the create
  answer, which names the token and the bot login.
- Step 8 did not run in this cloud, because the cloud was deleted at the end of the run and the
  step needs a live worker. The property was measured without a cloud, as the next section says.

## 5. Cleanup

- RunPod worker: deleted. The provider answered `404` for its ID afterwards.
- Fixture: stopped. Only the processes of this run were stopped.
- GitHub: pull request 1 is merged into `master`, and `APP-TOKEN.md` is on `master`. Keep both;
  they are the evidence of the run.

## 6. Evidence

- Candidate: commit `837d84b4`, binary SHA-256 `74272d72339223d3…`.
- Create answer: `GitHub: the worker holds a GitHub App installation token as
  horizon-for-peters[bot] for 1 repositories. It ends at most an hour after the mint, and only
  this machine installs a new one.`
- Exec answer: exit status 0, uid 10001, `/workspace/checkout` present.
- Push: `[app-token-cd22631b e481ba1] docs: record the GitHub App installation-token lane`.
- Pull request: `https://github.com/peters/horizon-hello-world/pull/1`, author
  `app/horizon-for-peters`, merged `20cbd71d`, file `APP-TOKEN.md` on `master`.
- Token limit, measured without a cloud on the same App key: the mint answered `201` for
  `horizon-hello-world` with `contents: write`, `pull_requests: write` and `metadata: read`; a
  read of that repository answered `200` and a read of another repository of the same
  installation answered `404`.
- Delete: phase `done`, `absent: true`.
- Transcript of the run: the fixture state directory of the run, written by the agent panel.
