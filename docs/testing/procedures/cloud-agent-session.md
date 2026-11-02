---
procedure: cloud-agent-session
feature: cloud agent session tools
platforms: [linux]
cost: rents compute
destructive: yes
secrets: [provider API key in Cloud settings, GitHub sign-in of this computer]
owner: peters
---

# Cloud agent session tool test procedure

## 1. Purpose

This procedure shows that the `cloud_agent_message` tool types one instruction
into the agent terminal of a cloud session, that `cloud_agent_read` returns the
bounded tail of that terminal, and that a call the machine or the cloud does not
allow is refused and types nothing.

## 2. Applicability

- Candidate: a Horizon build with the `cloud_agent_message` and `cloud_agent_read`
  MCP tools, from `cargo build` on this checkout.
- Platforms: Linux. The tools reach a cloud worker over SSH, so the run needs a
  cloud on a provider that Cloud settings configure.
- This procedure does not test:
  - the `cloud_exec` tool. Use the cloud command procedure.
  - a companion cloud's sessions. This version offers the tools for the clouds of
    the calling workspace only.
  - a structured status answer. This version returns terminal text.
  - the CLI. These tools have no CLI plan entry.

## 3. Safety

> **CAUTION:** EVERY CLOUD IN THIS PROCEDURE RENTS A PAID WORKER. Read
> `cloud_list` and the cloud card after each create, and stop the cloud before you
> end the run.

> **CAUTION:** AN INSTRUCTION IN A CLOUD TERMINAL ACTS WITH THE PERSON'S GITHUB
> PUSH GRANT. Send only the text in this procedure, and check that the agent in the
> terminal does nothing you did not ask for.

> **CAUTION:** THE TEST WRITES AN `agent_lifecycle` SECTION INTO THE CLOUD SETTINGS
> OF THIS COMPUTER. Copy the settings file first, and put the copy back in section
> 8. A wrong section refuses every agent create on this computer.

> **CAUTION:** THE TEST READS A TERMINAL THAT THE AGENT AND THE PERSON SHARE. The
> answer can hold a value that either of them printed. Do not copy such a value
> into a report, a commit or an issue.

## 4. Equipment and preconditions

- One Horizon desktop with an agent panel in a workspace.
- A Git repository on this computer with a committed revision and no
  `.horizon/cloud.yml` in that commit.
- A provider API key in Cloud settings for the provider that the policy allows.
- A GitHub sign-in of this computer in Cloud settings.
- A saved Horizon session for the workspace of the agent panel.
- The cloud settings file, at `HORIZON_HOME/cloud/settings.json`, with an
  `agent_lifecycle` section that allows the provider and the repository.
- An MCP client for the Horizon browser MCP tools, or a CLI that sends the same
  request.

## 5. Setup

1. Copy the cloud settings file to a private path outside the repository.

   Result: The copy exists and holds the same bytes.

2. Create the cloud for this run and wait until it is ready.

   Result: `cloud_list` shows the cloud in your workspace with a rate, and its card
   shows **Ready**.

3. Open one agent session on the cloud's card.

   Result: The cloud card shows an agent panel, and the worker runs its terminal.

4. Read the sessions of the cloud.

   Result: `cloud_list` returns a `sessions` array with one entry, and its
   `panel_id` is the identifier that this run uses.

## 6. Tasks

### 6.1 A — Send one instruction and read it

1. Call `cloud_agent_message` with the cloud ID, the `panel_id` from step 5.4 and
   the text `Reply with the single word ZEPHYR and change no file.`

   Result: The answer holds `agent`, `worktree`, `text`, `truncated: false`,
   `exit_status: 0` and `uid: 10001`, and its `text` holds the instruction.

2. Read the same session with `cloud_agent_read` and no `lines`.

   Result: The answer holds the agent's reply with `ZEPHYR`, and `truncated` is
   `false`.

3. Read the same session with `lines: 20` and then with `lines: 600`.

   Result: Both answers arrive. The second answer holds at least as much text as
   the first.

### 6.2 B — A refused body types nothing

1. Call `cloud_agent_message` with the same cloud and session and the text
   `line one` followed by a newline and `line two`.

   Result: The tool refuses with `cloud_agent_invalid_request`, and no part of the
   text appears in the session.

2. Send the text of step 6.1.1 again.

   Result: The instruction arrives and the agent replies.

### 6.3 C — A session that the cloud does not name

1. Call `cloud_agent_message` with the same cloud and a `panel_id` that no session
   of that cloud uses.

   Result: The tool refuses with `cloud_agent_unknown_session`, and the session of
   step 5.3 shows no new text.

2. Call `cloud_agent_read` with the same unknown `panel_id`.

   Result: The tool refuses with `cloud_agent_unknown_session`.

### 6.4 D — Another cloud and a cloud that is not ready

1. Read `cloud_list` and choose a second cloud in the same workspace that this
   workspace does not own, or one in another workspace.

   Result: `cloud_list` shows one cloud for the calling workspace, so this step
   uses a `cloud` ID of another workspace.

2. Call `cloud_agent_message` with that ID.

   Result: The tool refuses with `cloud_agent_unknown_cloud`, and no terminal of
   this workspace shows new text.

3. Stop the cloud of this run, and call `cloud_agent_read` with its ID.

   Result: The tool refuses with `cloud_agent_not_ready: Stopped`.

4. Resume the cloud and wait until it is ready again.

   Result: The card shows **Ready**, and `cloud_list` lists the cloud again.

### 6.5 E — The journal holds no text

1. Read the private journal of the cloud, at
   `HORIZON_HOME/browser/audit/cloud-agent/`, and open the file of that cloud.

   Result: Each line holds `actor`, `cloud`, `session`, `action`, `text_bytes`,
   `answer_bytes`, `outcome` and the duration. No line holds the instruction text
   of step 6.1.1.

### 6.6 F — The cloud shows the work

1. Call `cloud_agent_message` with the text
   `Create a branch named agent/proof and commit one empty commit on it.`

   Result: The answer arrives with `exit_status: 0`.

2. Read the session until the agent reports that it pushed the branch.

   Result: The terminal shows the push, and the branch exists in the repository
   that the cloud holds.

3. Read the branch with `git ls-remote` on this computer.

   Result: The branch `agent/proof` exists with the commit that the agent made.

## 7. Pass criteria

- Task A, B, C, E and F pass.
- Task D passes when the run has a second workspace with its own cloud.

## 8. Cleanup

1. Stop the cloud of this run on its card.

   Result: The card shows **Stopped**.

2. Delete the branch `agent/proof` from the repository.

   Result: `git ls-remote` lists no such branch.

3. Put the copy of the cloud settings file back.

   Result: The file holds the bytes of step 5.1.

4. Remove the private copy and the journal of this run.

   Result: The private paths are gone.

## 9. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). If not, put the results in the
pull request. Keep private evidence out of the repository.
