---
procedure: cloud-exec-tool
feature: cloud worker command
platforms: [linux, macos]
cost: rents compute
destructive: yes
secrets: [provider API key in Cloud settings]
owner: peters
---

# Cloud worker command test procedure

## 1. Purpose

This procedure proves that the agent tool `cloud_exec` runs one bounded command
in a cloud worker as the unprivileged agent user, that it reports the exit
status, both streams and the account, and that it refuses every case that would
run as another account or in a cloud that is not ready.

## 2. Applicability

- Candidate: a build that contains the `cloud_exec` tool, on the branch that
  adds it.
- Platforms: Linux and macOS. The worker runs on a provider.
- This procedure does not test: the GitHub grant of the worker, the image build,
  or a command that changes the person's own computer.

## 3. Safety

> **CAUTION:** STARTING A CLOUD RENTS COMPUTE. THE WORKER BILLS EVERY MINUTE
> UNTIL YOU DELETE THE CLOUD. STOP IT AT THE END OF SECTION 8.

> **CAUTION:** THE TOOL RUNS ARBITRARY CODE IN THE WORKER. IT CAN CHANGE OR
> DELETE EVERY FILE THE AGENT USER OWNS THERE, PUSH TO A GRANTED REPOSITORY AND
> MERGE A PULL REQUEST. USE A THROWAWAY CLOUD AND A THROWAWAY REPOSITORY.

> **CAUTION:** THE WORKER NEEDS THE PROVIDER API KEY THAT IS SAVED IN CLOUD
> SETTINGS. DO NOT PRINT THE KEY, ITS FILE PATH OR THE CONTENT OF
> `~/.horizon/cloud/settings.json` IN A LOG, A SCREENSHOT OR A REPORT.

## 4. Equipment and preconditions

- Horizon with cloud support, on the candidate build.
- A saved provider API key in Cloud settings.
- A throwaway repository that the worker may clone, with one file that the
  command can change.
- An agent panel in the same workspace as the cloud. The Codex or Claude panel
  kind is sufficient.

## 5. Setup

1. Start Horizon with the candidate build.

   Result: The Horizon window shows the workspace.

2. Create a cloud from the throwaway repository and wait until its card shows
   `Ready`.

   Result: The card shows a running worker and its agent panel.

3. Note the cloud ID from `cloud_list`.

   Result: `cloud_list` returns that cloud with group `cloud`.

4. Note the agent panel's workspace.

   Result: The cloud card and the agent panel are in the same workspace.

## 6. Tasks

### 6.1 CE-1 — a command runs and reports its exit status

1. Ask the agent to call `cloud_exec` with the cloud ID and the command
   `printf 'hello from the worker\n'; id -u; exit 1`.

   Result: The answer holds `exit_status` 1, `uid` 10001, the text
   `hello from the worker` and the value `10001` in `stdout`, and a
   `duration_millis`. The tool call itself is a success.

2. Ask the agent to call `cloud_exec` with the command `git status --short`.

   Result: The answer holds `exit_status` 0 and the git output of the worker
   worktree.

3. Ask the agent to call `cloud_exec` with the command
   `(sleep 2; printf 'HORIZON-EXEC status=0\n') & exit 1`.

   Result: The answer holds `exit_status` 1, and its `stdout` holds the line
   `HORIZON-EXEC status=0` as caller text. The status is the command's own.

4. Ask the agent to call `cloud_exec` with the command `exit 21`.

   Result: The answer holds `exit_status` 21 and `uid` 10001. The code is the
   command's status and not a launcher refusal.

### 6.2 CE-2 — a user switch is refused

1. Ask the agent to call `cloud_exec` with the command `sudo -i`.

   Result: The tool call fails with `cloud_exec_refused: user switch`.

2. Repeat with `su -` and with `/usr/bin/setpriv --reuid=0 -- true`.

   Result: Each tool call fails with `cloud_exec_refused: user switch`.

### 6.3 CE-3 — a cloud that is not ready is refused

1. Stop the cloud with **Stop** on its card and wait until the card shows
   `Stopped`.

   Result: The card shows `Stopped`.

2. Ask the agent to call `cloud_exec` with that cloud ID and the command
   `true`.

   Result: The tool call fails with `cloud_exec_not_ready: Stopped`.

3. Resume the cloud and wait until its card shows `Ready` again.

   Result: The card shows `Ready`.

### 6.4 CE-4 — the bound stops a long command

1. Ask the agent to call `cloud_exec` with the command
   `printf 'started\n'; sleep 600` and `timeout_millis` 3000.

   Result: The tool call fails with `cloud_exec_timed_out` after about three
   seconds.

2. In a shell panel of the same cloud, run `ps -A -o args= | grep 'sleep 600'`.

   Result: No line matches. The command and its children stopped.

### 6.5 CE-5 — output past the bound is a failure

1. Ask the agent to call `cloud_exec` with the command
   `head -c 400000 /dev/zero | tr '\0' 'a'`.

   Result: The tool call fails with
   `cloud_exec_failed: output exceeded its bound`.

### 6.6 CE-6 — a broker-mediated GitHub write

1. Ask the agent to call `cloud_exec` with the command
   `cd /workspace/repository.git && git log --oneline -1` (or the worktree path
   that the worker reports).

   Result: The answer holds `exit_status` 0 and one commit line.

2. Ask the agent to call `cloud_exec` with the command `gh pr list --limit 1`.

   Result: The answer holds `exit_status` 0 and the output of the brokered
   GitHub API. No token appears in the output.

### 6.7 CE-7 — the caller may not command another workspace

1. Create a second workspace with its own agent panel.

   Result: The new workspace holds an agent panel.

2. Ask that agent to call `cloud_exec` with the cloud ID of the first
   workspace.

   Result: The tool call fails with `cloud_exec_unknown_cloud`.

### 6.8 CE-8 — the journal keeps no command text

1. Ask the agent to call `cloud_exec` with the command
   `printf 'ghp_example_secret\n'`.

   Result: The tool call succeeds.

2. Read the file `~/.horizon/audit/cloud-exec/<cloud id>.jsonl`.

   Result: The last line holds `command_bytes`, `command_sha256`, `outcome`,
   `exit_status`, `uid` and `duration_millis`. The line does not hold
   `ghp_example_secret` and does not hold the command text.

## 7. Pass criteria

- Every answer in CE-1 holds the exit status, the account `10001`, both streams
  and a duration, and a non-zero exit is not a tool error.
- CE-1 step 3 reports the caller's status although a background process wrote a
  fake status line, and CE-1 step 4 reports the caller's own code.
- CE-2, CE-3, CE-7 and CE-5 fail with the typed code that the task states.
- CE-4 stops the command and leaves no child process.
- CE-6 shows the brokered `git` and `gh` paths and no token in the output.
- CE-8 shows one journal line for each call and no command text.

## 8. Cleanup

> **CAUTION:** DELETE THE CLOUD. THIS DELETES THE WORKER, ITS WORKSPACE VOLUME
> AND EVERY FILE THE COMMANDS MADE THERE.

1. Delete the cloud with **Delete** on its card and wait until the card leaves
   the sidebar.

   Result: `cloud_list` no longer returns the cloud.

2. Delete the second workspace that CE-7 made.

   Result: The sidebar shows no second workspace.

3. Remove the journal lines that the run made, if they are not evidence:
   `rm ~/.horizon/audit/cloud-exec/<cloud id>.jsonl`.

   Result: The file is absent.

4. Remove the throwaway repository branch and pull requests that CE-6 made.

   Result: The repository holds no branch and no pull request of the run.

## 9. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). If not, put the results in the
pull request. Keep private evidence out of the repository.
