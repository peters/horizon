---
procedure: upkeep-github-policy
feature: The Upkeep policy of a worker's GitHub access (horizon-worker-github upkeep)
platforms: [linux]
cost: rents compute
destructive: yes (GPO-04 deletes the cloud that the setup made)
secrets: [GitHub sign-in of Cloud settings, Hetzner API token, ChatGPT login of Codex]
owner: peters
---

# Upkeep GitHub policy test procedure

## 1. Purpose

This procedure proves that on an Upkeep cloud the worker's GitHub service refuses the
changes that an Upkeep never needs, and that the board shows the refusals.

## 2. Applicability

- Candidate: a Horizon build and a published worker image (the quick-start pin) that both
  have the Upkeep policy. Images publish only from `main`, so run this procedure after the
  pin bump that carries the policy, before the live Upkeep moves to it.
- Platforms: Linux host, one Hetzner `cx23` Upkeep cloud.
- This procedure does not test the quality of the agent's work.

## 3. Safety

> **CAUTION:** THE SETUP RENTS A HETZNER SERVER AND A VOLUME. The provider bills them
> until GPO-04 deletes the cloud.

> **CAUTION:** USE A TEST REPOSITORY THAT YOU OWN. The tasks push a branch and ask for
> a workflow dispatch in it. Never use a repository with production workflows.

## 4. Equipment and preconditions

- The [local device smoke fixture](../../../scripts/device-smoke/README.md) with
  `--native-view`, a persistent session, and Cloud settings with a GitHub sign-in, a
  Hetzner project and the ChatGPT login of Codex.
- A test repository `<repo>` with a workflow `.github/workflows/ci.yml` and a second
  workflow `.github/workflows/manual.yml` that has `on: workflow_dispatch`.

## 5. Setup

1. In New Upkeep, select only `<repo>` and click **Start Upkeep**. Finish the GitHub
   authorization of the cloud.

   Result: The card shows **Ready** and the board of the Upkeep.

2. Open a shell panel on the worker. Type `ls -l /run/horizon-worker/github-upkeep-policy`.

   Result: The file exists and belongs to root.

3. Ask the agent for push access to `<repo>` with the `github_access` tool from the
   **Upkeep agent** panel, and clone `<repo>` to `/tmp/check` there.

   Result: Horizon allows the request without a prompt.

## 6. Tasks

### 6.1 GPO-01: The refused API changes

1. Run `gh release create policy-check --repo <repo> --notes x`,
   `gh api -X POST repos/<repo>/statuses/<head> -f state=success -f context=ci`,
   `gh api -X PUT repos/<repo>/contents/.github/workflows/ci.yml -f message=x -f content=eA== -f sha=<sha>`
   and `gh workflow run manual.yml --repo <repo>`.

   Result: Each command fails with a reason that starts with `Horizon: this Upkeep`.
   GitHub has no release, no status, no change of `ci.yml` and no run.

2. Add `{"repository": "<repo>", "workflow": "manual.yml"}` to `workflows` in
   `~/.horizon/upkeep/<cloud>/upkeep.json`, wait for the next exchange, and run
   `gh workflow run manual.yml --repo <repo>` again.

   Result: GitHub starts the workflow.

### 6.2 GPO-02: Ordinary work still works

1. Change a file outside `.github` in `/tmp/check`, commit, and run
   `git push origin HEAD:refs/heads/policy-check`.

   Result: GitHub has the branch with the commit.

2. Run `gh pr create --repo <repo> --head policy-check --title "Policy check" --body x`,
   then `gh pr comment` and `gh pr edit --add-label` on it.

   Result: Each command works.

### 6.3 GPO-03: The refusals are logged and shown

1. Type `cat /workspace/.horizon-gate/refused.jsonl` in the shell panel.

   Result: The file belongs to root and has one line for each refusal of GPO-01.

2. Wait for the next exchange and look at **Latest run** on the board.

   Result: The board shows the refusals as lines that start with `The worker refused`.

### 6.4 GPO-04: Delete the cloud

1. Close the pull request of GPO-02 and delete the branch `policy-check`. Remove the
   Upkeep cloud from its card and confirm the deletion.

   Result: Hetzner shows no server or volume of the cloud.

## 7. Pass criteria

- GPO-01 to GPO-04 give their results.
- No refused change reached GitHub.

## 8. Cleanup

GPO-04 deletes every resource of the setup.

## 9. Record of results

Put the results in the pull request. Keep private repository names out of public evidence.
