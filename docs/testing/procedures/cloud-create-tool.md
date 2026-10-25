---
procedure: cloud-create-tool
feature: cloud create MCP tool
platforms: [linux]
cost: rents compute
destructive: yes
secrets: [provider API key in Cloud settings]
owner: peters
---

# Cloud create tool test procedure

## 1. Purpose

This procedure shows that the `cloud_create` MCP tool rents one quick-start cloud
without a person, and that it rents nothing when the machine policy or the
request does not allow it. It also shows that the cloud appears on the card of
the calling agent's workspace and that the card cannot deploy that cloud a second
time.

## 2. Applicability

- Candidate: a Horizon build with the `cloud_create` MCP tool, from
  `cargo build` on this checkout.
- Platforms: Linux. The cloud provider is the one the profile names. Quick start
  uses RunPod.
- This procedure does not test:
  - a `.horizon/cloud.yml` profile, a worker choice, a plan token, siblings or a
    publishing selection. This version does not offer them.
  - the CLI. This version has no CLI plan entry for a create.
  - GitHub sign-in with a person. That step belongs to the cloud card procedure.
  - stop, delete and removal of a cloud. Use the cloud card procedure and the
    delete tool procedure.

## 3. Safety

> **CAUTION:** EVERY SUBMITTED CLOUD RENTS A PAID WORKER. Read `cloud_list` and
> the cloud card after each submit, and stop the cloud that the run created
> before you end the run.

> **CAUTION:** THE TEST WRITES AN `agent_lifecycle` SECTION INTO THE CLOUD
> SETTINGS OF THIS COMPUTER. Copy the settings file first, and put the copy back
> in section 8. A wrong section refuses every agent create on this computer.

## 4. Equipment and preconditions

- One Horizon desktop with an agent panel in a workspace.
- A Git repository on this computer with a committed revision and no
  `.horizon/cloud.yml` in that commit.
- A RunPod API key in Cloud settings, as `runpod_key_file`.
- A saved Horizon session for the workspace of the agent panel. An ephemeral
  session refuses a create.
- The cloud settings file, at `HORIZON_HOME/cloud/settings.json`.
- An MCP client for the Horizon browser MCP tools, or a CLI that sends the same
  request.

## 5. Setup

1. Copy the cloud settings file to a private path outside the repository.

   Result: The copy exists and holds the same bytes.

2. Read the cloud state root and the settings file, and note whether the file
   has an `agent_lifecycle` section.

   Result: You know which section you replace and later restore.

3. Start Horizon from the candidate build and open the workspace with the agent
   panel.

   Result: The agent panel is live, and the cloud list of that workspace is
   visible.

## 6. Tasks

### 6.1 A1 — A machine without a policy rents nothing

1. Remove the `agent_lifecycle` section from the cloud settings file, and keep
   the rest of the file as it was.

   Result: `Settings::load` accepts the file, and Cloud settings shows no agent
   lifecycle values.

2. Call `cloud_create` with `operation submit`, a lowercase UUID `operation_id`,
   the absolute `repository`, and `quick_start: true`.

   Result: The answer holds `phase: submitted`. Horizon reads the settings on a
   worker thread, so the refusal is the next answer.

3. Call `cloud_create` with `operation status` and the same `operation_id`.

   Result: `phase` is `failed`, `done` is true, and `error` is
   `cloud_create_refused: no agent lifecycle policy is configured`. No cloud card
   appears, and `cloud_list` stays the same.

### 6.2 A2 — The policy allows one repository, one provider and one rate

1. Add an `agent_lifecycle` section with `providers: ["runpod"]`,
   `max_hourly` above the current RunPod CPU price, `repositories` with the
   absolute path of the test repository, and `allow_github_sign_in_skip: false`.

   Result: The file loads. An invalid value, such as a relative repository path
   or a provider that Horizon cannot create clouds on, makes `Settings::load`
   fail.

2. Call `cloud_create` again with the `operation_id` of A1 step 2.

   Result: The answer holds `phase: submitted` and the new `cloud` ID.

3. Call `cloud_create` with `operation status` and the same `operation_id`
   again, at least every 10 seconds.

   Result: `phase` moves from `submitted` to `rolling_out`, `stage` names the
   current step, and `lines` grows with the output of the deployment. The cloud
   card of the agent's workspace shows the same step.

4. Wait until `done` is true.

   Result: `phase` is `ready`, `ready.worker` names the worker, `ready.hourly`
   gives its rate, and `ready.image` gives the public base image of the
   quick-start profile.

5. Read `cloud_list` for the workspace.

   Result: The cloud of step 2 is in the list, with its card, its status line
   and its hourly rate.

6. Call `cloud_create` with `operation cancel` and the same `operation_id`.

   Result: The tool answers `cloud_create_done: the cloud is ready; cloud_list
   shows it, and its card stops it`. The card still shows the cloud, and its own
   controls, such as **Stop**, work again.

### 6.3 A3 — One operation owns one cloud

1. While A2 step 4 is still `rolling_out`, click **Start**, **Resume** or
   **Deploy** on that cloud's card.

   Result: The card starts nothing. It shows the deployment of the tool, and the
   controls that would start another deployment stay refused.

### 6.4 A4 — A second cloud for the same repository is refused

1. Call `cloud_create` with `operation submit`, a new `operation_id` and the
   same `repository` as A2 step 1.

   Result: `phase` becomes `failed`, and `error` starts with `cloud_create_busy`.
   No second cloud card appears.

### 6.5 A5 — A commit with its own settings is refused

1. Call `cloud_create` with `operation submit`, a new `operation_id` and the
   `repository` of a checkout whose commit has `.horizon/cloud.yml`.

   Result: The answer holds `phase: submitted`.

2. Call `cloud_create` with `operation status` and that `operation_id`.

   Result: `phase` is `failed`, `error` starts with `cloud_create_needs_profile`,
   and no cloud card appears.

### 6.6 A6 — A create without a saved session is refused

1. Switch to an ephemeral Horizon session, and call `cloud_create` with
   `operation submit`.

   Result: The tool answers `cloud_create_needs_persistent_session`. No cloud
   card appears.

### 6.7 A7 — A cancel does not recall a rented worker

1. Start one more create with the policy of A2, and call `cloud_create` with
   `operation cancel` and its `operation_id` while `phase` is `rolling_out`.

   Result: The answer holds `phase: cancelled` and `cloud_create_cancelled: the
   worker may have been rented; run cloud_list and stop it if it exists`.

2. Read `cloud_list` and stop the cloud that the run created, on its card.

   Result: The card reports the stop, and the worker is gone.

3. Cancel one more create while `phase` is still `submitted`, before the card
   appears.

   Result: The answer holds `phase: cancelled`. No cloud card appears, and
   `cloud_list` stays the same, because nothing was rented.

### 6.8 A8 — The operation of another agent is unknown

1. Call `cloud_create` with `operation status` from a different agent panel, with
   the `operation_id` of A2.

   Result: The tool answers `cloud_create_unknown_operation`.

## 7. Pass criteria

- A1, A5, A6 and A8 rent nothing and add no card.
- A7 step 3 rents nothing and leaves no card: a cancel before allocation removes
  the card the operation registered.
- A2 rents exactly one worker, and its card exists in the agent's workspace
  before the worker is rented.
- A2 step 6 answers `cloud_create_done`: a ready cloud is done for the
  operation, and it stays on its card.
- A3 starts no second deployment. One cloud has one deployment.
- A4 adds no second cloud for the same repository.
- A7 answers the cancel sentence, and the created cloud is stopped in the same
  run.
- `cloud_list` shows every cloud that the run created.

## 8. Cleanup

1. Stop every cloud that this run created, on its card, and wait for the stop.

   Result: `cloud_list` shows no running cloud of this run.

2. Delete the clouds that this run created, with the delete tool or the card.

   Result: The provider reports the worker and its storage absent.

3. Put the copied cloud settings file back, and remove the copy.

   Result: `agent_lifecycle` holds its earlier values, or no section at all.

4. Close Horizon and the MCP client.

   Result: No Horizon process of this run stays alive.

## 9. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). If not, put the results in the
pull request. Give each task ID of section 6 a result. Keep the worker IDs,
credentials and paths of this computer out of the repository.
