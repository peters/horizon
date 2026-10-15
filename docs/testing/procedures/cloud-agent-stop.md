---
procedure: cloud-agent-stop
feature: The agent stop of a dedicated cloud worker
platforms: [linux]
cost: rents compute
destructive: yes
secrets: [RunPod API key file, Hetzner token file, SSH identity file]
owner: peters
---

# Cloud agent stop test procedure

## 1. Purpose

This procedure makes sure that an agent on a dedicated worker can stop the
worker when its task is done, without an idle period in the profile. The worker
must refuse the stop while another agent session works. Where the worker has no
stop credential, the worker must record the request for Horizon to take.

## 2. Applicability

- Candidate: each candidate that changes `horizon-worker-stopd`,
  `horizon-worker-stop`, the supervisor, the worker check, or how Horizon takes
  an agent's stop request.
- Platforms: Linux. The worker image is amd64.
- Providers: RunPod for tasks A01 to A03, Hetzner for tasks A04 and A05.
- This procedure does not test: the idle stop of an older image, a shared
  worker, a profile with hosted devices, or a Docker host. A Docker host takes
  requests as Hetzner does.

## 3. Safety

> **CAUTION:** EACH LANE RENTS COMPUTE. The provider bills the worker while it
> runs, and bills the storage after the stop.

> **CAUTION:** USE ONLY THE CLOUDS THAT THIS RUN MADE. If you stop or delete
> other clouds, other people lose their work.

> **CAUTION:** DO NOT SHOW THE SETTINGS FILE, THE KEY FILES OR THE WORKER
> ENVIRONMENT IN EVIDENCE. The environment of a RunPod worker contains its stop
> credential.

## 4. Equipment and preconditions

- A cloud settings file, `<settings>`, with a RunPod key and a Hetzner token.
  Use references to the key files. Do not write a key in this procedure.
- A synthetic repository with a `.horizon/cloud.yml` file. It has these two
  profiles, both without `idle_stop_minutes`:
  - `runpod-agent`: RunPod, CPU, the cheapest offer, `agents: [claude]`.
  - `hetzner-agent`: Hetzner, the cheapest server type, `agents: [claude]`.
- A worker image built from the candidate `examples/cloud-worker` directory.
  Before the worker images of the candidate are published, use the quick start
  image and copy the candidate scripts onto the worker in step 5.4.
- The `cloud_deploy` example program, built from the candidate commit:

  ```bash
  cargo build -p horizon-core --example cloud_deploy --features cloud-workspaces
  ```

- A clock. Write down the time of each result.

In this procedure, `<state root>` is the directory of the cloud. Agent
sessions run as the agent user on their own tmux server. In the root shell,
`<agent tmux>` is this command, typed in `/workspace`:

```bash
horizon-worker-tailnet agent tmux -L horizon-cloud
```

## 5. Setup

1. Deploy a cloud with the profile `runpod-agent`:

   ```bash
   target/debug/examples/cloud_deploy deploy <settings> <repository> runpod-agent <state root> <cloud id>
   ```

   Result: The last line says that the cloud is ready.

2. Open an SSH connection to the worker as root.

   Result: You have a root prompt on the worker.

3. A root SSH shell does not have the environment of the worker. Load it into
   the shell:

   ```bash
   while IFS= read -r -d '' variable; do
     case $variable in RUNPOD_*|HORIZON_WORKER_*) export "$variable" ;; esac
   done < /proc/1/environ
   ```

   Result: `echo "$HORIZON_WORKER_CAPABILITIES"` shows the capabilities JSON
   with its quotes.

4. If the image is not built from the candidate, copy the candidate scripts
   `horizon-worker-stopd`, `horizon-worker-stop`, `horizon-worker-supervise`,
   `horizon-worker-check` and `horizon-worker-configure` to `/usr/local/bin/`
   with mode 755.
   Then start the service:

   ```bash
   setsid nohup horizon-worker-stopd > /var/tmp/stopd.log 2>&1 < /dev/null &
   ```

   Result: `/run/horizon-worker/stop.sock` exists.

## 6. Tasks

### 6.1 A01 — The worker offers the agent stop without an idle period

1. Type `horizon-worker-check | grep agent-stop`.

   Result: The output is `horizon-agent-stop-contract=1`.

2. Type `horizon-worker-stopd --mode`.

   Result: The output is `self`.

3. Type `HORIZON_WORKER_STARTUP=1 horizon-worker-stopd --mode`.

   Result: The output is `none`. A shared worker offers no stop.

### 6.2 A02 — The worker refuses a stop while another agent session works

1. Make two synthetic agent sessions, `smoke-a` and `smoke-b`. Write `claude`
   in `/workspace/sessions/<session>/agent` for each.

   Result: The two files exist.

2. Start `smoke-b` so that it prints one line each second:

   ```bash
   <agent tmux> new-session -d -s smoke-b 'while true; do date; sleep 1; done'
   ```

   Result: `<agent tmux> ls` shows `smoke-b`.

3. Start `smoke-a` so that it asks for a stop:

   ```bash
   <agent tmux> new-session -d -s smoke-a \
     'horizon-worker-stop --reason "smoke: task done" > /var/tmp/stop-a.txt 2>&1; sleep 900'
   ```

   Result: After some seconds, `/var/tmp/stop-a.txt` says that another agent
   session produced output in the last two minutes. The worker runs.

4. Type `horizon-worker-stop --reason test` in the root prompt.

   Result: The output says that only an agent session on this worker can stop
   it.

### 6.3 A03 — An agent stops its RunPod worker

1. Stop `smoke-b`. Wait three minutes.

   Result: No agent session prints output.

2. Kill `smoke-a` and start it again with the same command as in step 6.2.3.

   Result: The SSH connection closes in less than one minute.

3. Read the worker status from the RunPod API.

   Result: The desired status is `EXITED`.

4. Type this command:

   ```bash
   target/debug/examples/cloud_deploy reconcile <settings> <state root>
   ```

   Result: The first line contains `"status":"inactive"`.

5. Resume the cloud. Open an SSH connection to the worker again. Type
   `horizon-worker-stop --last`.

   Result: The output contains `smoke: task done` and the session `smoke-a`.

### 6.4 A04 — A worker without a stop credential records the request

> **CAUTION:** THIS TASK RENTS A HETZNER SERVER AND A VOLUME.

1. Deploy a cloud with the profile `hetzner-agent` and open an SSH connection
   to the worker as root. Do setup steps 5.3 and 5.4.

   Result: The worker environment has `HORIZON_WORKER_STOP=horizon`.
   `horizon-worker-stopd --mode` prints `horizon`.

2. Do steps 6.2.1 and 6.2.3 with only the session `smoke-a`.

   Result: `/var/tmp/stop-a.txt` says that Horizon stops the worker the next
   time it checks. The worker runs.

3. Type `horizon-worker-stopd --take` at once.

   Result: The output starts with `{"waiting":`. A take counts the output of
   every agent session, also of `smoke-a`, which started less than two minutes
   ago. The request stays.

4. Wait two minutes. Type `horizon-worker-stopd --take` two times.

   Result: Each output is the same stop record with the reason
   `smoke: task done`. The stop log
   `/workspace/.horizon/self-stops.jsonl` has the record one time.

5. Type `horizon-worker-stopd --untake <at>`, with the `at` value of the
   record.

   Result: The output is `true`. The stop log does not have the record.
   `horizon-worker-stopd --take` prints `null`.

### 6.5 A05 — Horizon stops a Hetzner cloud for its agent

> **CAUTION:** THIS TASK RENTS A HETZNER SERVER AND A VOLUME.

1. Start the [local device smoke fixture](../../../scripts/device-smoke/README.md)
   with `--native-view` and the frozen candidate. Give the fixture home a cloud
   settings file with the Hetzner token. Watch the fixture in a native VNC
   Device panel and start a recorder on its display.

   Result: The Device panel shows the candidate.

2. In the candidate, make a new cloud with the profile `hetzner-agent`.

   Result: The card shows **Ready**.

3. Open an SSH connection to the worker as root. Do setup step 5.3 and steps
   6.2.1 and 6.2.3 with only the session `smoke-a`. Do not type in any panel of
   the cloud.

   Result: `/var/tmp/stop-a.txt` says that Horizon stops the worker the next
   time it checks.

4. Wait three minutes. Do not close the candidate.

   Result: The card logs `claude asked to stop this cloud: smoke: task done`,
   shows **Stopping**, and then **Stopped** with
   `Stopped by claude just now: smoke: task done`. The SSH connection closes.

5. List the servers of the Hetzner project through its API.

   Result: The server of the cloud is gone. Its volume remains.

6. Click **Resume worker** on the card.

   Result: The card shows **Ready** on a new server. The card still names the
   agent's stop until the next stop.

## 7. Pass criteria

- A01 to A05 show each `Result:` line.
- No profile of this run sets `idle_stop_minutes`.
- The worker never stops while another agent session prints output.

## 8. Cleanup

> **CAUTION:** DELETE ONLY THE CLOUDS THAT THIS RUN MADE. A delete removes the
> worker and its storage. You cannot undo it.

1. For each cloud of this run, type this command:

   ```bash
   target/debug/examples/cloud_deploy delete <settings> <state root>
   ```

   Result: The command ends without an error.

2. List the workers, volumes and SSH keys of each provider through its API.

   Result: No resource of this run remains.

## 9. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). If not, put the results in the
pull request. Keep private evidence out of the repository.
