---
procedure: upkeep-github-policy
feature: The Upkeep policy of a worker's GitHub access (horizon-worker-github upkeep)
platforms: [linux]
cost: rents compute
destructive: yes (GPO-05 deletes the cloud that the setup made)
secrets: [GitHub sign-in of Cloud settings, Hetzner API token, ChatGPT login of Codex]
owner: peters
---

# Upkeep GitHub policy test procedure

## 1. Purpose

This procedure proves that on an Upkeep cloud the worker's GitHub service refuses the
changes that an Upkeep never needs, an agent's approval included, that a merge passes
only the merge gate, and that the board shows the refusals.

## 2. Applicability

- Candidate: a Horizon build and a published worker image (the quick-start pin) that both
  have the Upkeep policy. Images publish only from `main`, so run this procedure after the
  pin bump that carries the policy, before the live Upkeep moves to it.
- Platforms: Linux host, one Hetzner `cx23` Upkeep cloud.
- This procedure does not test the quality of the agent's work.

## 3. Safety

> **CAUTION:** THE SETUP RENTS A HETZNER SERVER AND A VOLUME. The provider bills them
> until GPO-05 deletes the cloud.

> **CAUTION:** USE A TEST REPOSITORY THAT YOU OWN. The tasks push branches, merge a pull
> request and ask for a workflow dispatch in it. Never use a repository with production workflows.

## 4. Equipment and preconditions

- The persistent launcher of the [cloud panels procedure](cloud-panels.md), with Cloud
  settings that have a GitHub sign-in, a Hetzner project and the ChatGPT login of Codex.
  `<data-home>` is the host path of its private home, as that procedure defines it.
- A test repository `<repo>` with a workflow `.github/workflows/ci.yml` that runs on
  `pull_request`, and a second workflow `.github/workflows/manual.yml` that has
  `on: workflow_dispatch`.

## 5. Setup

1. In New Upkeep, select only `<repo>` and click **Start Upkeep**. Finish the GitHub
   authorization of the cloud.

   Result: The card shows **Ready** and the board of the Upkeep.

2. Wait one minute for the first exchange. Open a shell panel on the worker and type
   `ls -l /run/horizon-worker/github-upkeep-policy`.

   Result: The file exists and belongs to root.

3. Open a Codex panel on the worker and ask it to call the `github_access` tool for
   `<repo>` with push access. On the cloud card, allow the request for the cloud. In the
   shell panel, clone `<repo>` to `/tmp/check`.

   Result: The clone works. Every agent and shell panel of the cloud now reaches
   `<repo>`, under the policy.

## 6. Tasks

Run the commands of the tasks in the shell panel of setup step 2, in `/tmp/check`.

### 6.1 GPO-01: The refused API changes

1. Note the head commit of the default branch as `<head>`
   (`gh api repos/<repo>/commits/HEAD --jq .sha`) and the blob of the workflow file as
   `<sha>` (`gh api repos/<repo>/contents/.github/workflows/ci.yml --jq .sha`). Run
   `gh release create policy-check --repo <repo> --notes x`,
   `gh api -X POST repos/<repo>/statuses/<head> -f state=success -f context=ci`,
   `gh api -X PUT repos/<repo>/contents/.github/workflows/ci.yml -f message=x -f content=eA== -f sha=<sha>`,
   `gh api -X PUT repos/<repo>/contents/policy-check.txt -f message=x -f content=eA==`
   and `gh workflow run manual.yml --repo <repo>`. In `/tmp/check`, run
   `git commit --allow-empty -m x`, `git push origin HEAD:refs/heads/<default>` (the
   default branch of `<repo>`), `git tag policy-check` and `git push origin policy-check`.

   Result: Each `gh` command and each `git push` fails with a reason that starts with
   `Horizon: this Upkeep`. GitHub has no release, no status, no change of `ci.yml`, no `policy-check.txt`, no
   run, no new commit on the default branch and no tag.

2. On this computer, open `<data-home>/.horizon/upkeep/<cloud>/upkeep.json`, where `<cloud>`
   is the one folder there, the cloud of this Upkeep. Add the field
   `"workflows": [{"repository": "<repo>", "workflow": "manual.yml"}]`. Wait one minute
   for the next exchange, and in the shell panel run
   `gh workflow run manual.yml --repo <repo>` again.

   Result: GitHub starts the workflow.

### 6.2 GPO-02: Ordinary work still works

1. Change a file outside `.github` in `/tmp/check`, commit, and run
   `git push origin HEAD:refs/heads/policy-check`.

   Result: GitHub has the branch with the commit.

2. Run `gh pr create --repo <repo> --head policy-check --title "Policy check" --body x`
   and note its number `<number>`. Then run `gh label create policy-check --repo <repo>`,
   `gh pr comment <number> --repo <repo> --body x` and
   `gh pr edit <number> --repo <repo> --add-label policy-check`.

   Result: Each command works.

3. Run `gh api '/advisories?ecosystem=pip&per_page=1' --jq '.[0].ghsa_id'`.

   Result: It prints an advisory ID that starts with `GHSA-`.

4. Run `gh pr review <number> --repo <repo> --approve`, then
   `gh pr review <number> --repo <repo> --comment --body x`.

   Result: The approval fails with a reason that starts with `Horizon: this Upkeep`.
   The comment review works.

### 6.3 GPO-03: The merge gate

1. Wait until the checks of the pull request of GPO-02 pass, and note its head as
   `<head>` (`gh pr view <number> --repo <repo> --json headRefOid --jq .headRefOid`).
   Run `gh pr merge <number> --repo <repo> --squash`.

   Result: The merge fails with the reason that this Upkeep merges only the head
   commit that the merge names.

2. Run `gh pr merge <number> --repo <repo> --squash --match-head-commit <head>`.

   Result: The merge fails with the reason that the pull request is blocked on
   `author`: the branch `policy-check` does not start with `upkeep/`. The pull request
   stays open.

3. Run `git push origin HEAD~1:refs/heads/policy-base`, then
   `gh pr edit <number> --repo <repo> --base policy-base`.

   Result: The edit fails with the reason that this Upkeep does not let an agent change
   the base branch of a pull request. The base of the pull request does not change.

4. Run `gh pr create --repo <repo> --base policy-base --head policy-check --title base
   --body base`, note its head as `<head>`, and run
   `gh pr merge <new number> --repo <repo> --squash --match-head-commit <head>`.

   Result: The merge fails with the reason that this Upkeep merges only into the default
   branch. Close the new pull request with `gh pr close <new number> --repo <repo>`.

5. In `/tmp/check`, run `git push origin HEAD:refs/heads/upkeep/policy-check`, open a
   pull request for it as in GPO-02, wait until its checks pass, and merge it with
   `--match-head-commit` as in step 2.

   Result: GitHub merges the pull request.

### 6.4 GPO-04: The refusals are logged and shown

1. Type `ls -ld /workspace/.horizon-gate /workspace/.horizon-gate/refused.jsonl` and
   `cat /workspace/.horizon-gate/refused.jsonl` in the shell panel.

   Result: `ls` shows root as the owner of the folder (`drwxr-xr-x`) and of the file
   (`-rw-r--r--`). The file has one line for each refusal of GPO-01 to GPO-03.

2. Wait one minute for the next exchange. On this computer, type
   `grep -c "The worker refused" <data-home>/.horizon/upkeep/<cloud>/status.json`.

   Result: The count is the number of refusals of step 1, or 20 when there are more.
   **Latest run** on the board shows them among the newest lines, unless the agent
   wrote newer lines since.

### 6.5 GPO-05: Delete the cloud

1. Close the pull request of GPO-02, and delete the branches and the label
   `policy-check`. Remove the Upkeep cloud from its card and confirm the deletion.

   Result: Hetzner shows no server or volume of the cloud.

## 7. Pass criteria

- GPO-01 to GPO-05 give their results.
- An approval that lifts a blocker is covered by the tests of the gate, because the
  account of the worker cannot approve its own pull request.
- No refused change reached GitHub.

## 8. Cleanup

GPO-05 deletes every resource of the setup.

## 9. Record of results

Put the results in the pull request. Keep private repository names out of public evidence.
