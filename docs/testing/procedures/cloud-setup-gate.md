---
procedure: cloud-setup-gate
feature: Setup gate (Horizon's proof and desktop preview of the setup agent on real repositories)
platforms: [linux]
cost: none
destructive: yes
secrets: [the Git login of this computer for private Rust git dependencies, the NuGet credentials of this computer for private .NET packages]
owner: peters
---

# Setup gate test procedure

## 1. Purpose

This procedure runs the setup gate on a candidate. The setup gate runs Horizon's proof
and the desktop preview on a list of real repositories, the same way that the **New
cloud** dialog runs them. It finds problems that a code review and the unit tests do
not find.

## 2. Applicability

- Candidate: the head of a pull request that changes the setup agent, the proof or the
  desktop preview. Run the gate on the first head that is ready for review. Run it again
  on the final head before you ask for permission to merge.
- Platforms: Linux with Docker.
- This procedure does not test: the setup agent itself, the **New cloud** dialog, a
  deploy or a provider. No step rents compute. The
  [setup agent procedure](cloud-setup-agent.md) tests the dialog.

## 3. Safety

> **CAUTION:** USE ONLY REPOSITORIES THAT YOU MAY COPY INTO LOCAL DOCKER. The proof
> copies the committed tree of each repository into a Docker volume.

> **CAUTION:** KEEP THE GATE LIST OUT OF THE HORIZON REPOSITORY. The list names
> private repositories and local paths. Do not write these names in a pull request.

> **CAUTION:** DO NOT REMOVE DOCKER RESOURCES BY PATTERN AND DO NOT USE
> `docker system prune`. Other runs and other people use the same Docker. The runner
> removes only the resources of its own run names, by their exact names.

## 4. Equipment and preconditions

- Docker on the host, through a socket of this computer. The runner does not start
  when Docker does not answer or uses a daemon on another computer. At the start, the
  runner sets `DOCKER_HOST` for itself and for each gate lane, so a Docker context that
  changes during the run changes nothing.
- Free disk. The runner does not start when the output folder or the Docker root folder
  has less than 100 GB free. A large Git LFS repository needs tens of GB for its
  snapshot and its volume.
- Python 3 and a Rust toolchain that builds Horizon.
- For each repository, a local worktree. Its `HEAD` commit must contain the `.horizon`
  setup to prove. The proof clones the committed tree, so an uncommitted change is not
  in it.
- A load average below about 30. At a higher load, tests with time limits in the
  repositories fail for no real reason.

## 5. Setup

1. Write the gate list, a JSON file outside the Horizon repository. Write one entry for
   each repository, with the keys in [10.1](#101-gate-list). Example with synthetic
   names:

   ```json
   [
     {
       "repo": "example-api",
       "path": "/var/tmp/gate-worktrees/example-api",
       "spec": {"workflow": "ci.yml", "jobs": ["test", "audit"], "event": "push"},
       "leave_out": ["audit"]
     },
     {
       "repo": "example-desktop",
       "path": "/var/tmp/gate-worktrees/example-desktop",
       "spec": {"commands": ["make test"]},
       "preview": {"command": "./run-app.sh", "size": "1280x800"},
       "heavy": true
     }
   ]
   ```

   Result: The file is a JSON array with one object for each repository.

2. Check out each worktree at the commit to prove.

   Result: `git -C <path> status --short` shows nothing for each entry.

## 6. Tasks

### 6.1 G01: Run the gate

> **CAUTION:** THE GATE USES THE GIT LOGIN AND THE NUGET CREDENTIALS OF THIS COMPUTER.
> It clones the private Rust git dependencies that the lock files pin, and it restores
> private .NET packages. The CI of the proof can read them.

1. In the worktree of the candidate, run this command. Use the options in
   [10.2](#102-runner-options) to change the defaults.

   ```bash
   python3 scripts/setup-gate.py <list>
   ```

   Result: The runner builds the horizon-ui tests. Then it prints the run folder and
   one line for each gate lane, with its run names, its log and its entries.

2. Wait for the entries to end.

   Result: For each entry that ends, the runner prints one line. The line shows PASS or
   FAIL, the gate lane, the time of each phase, the preview and the first failure line.

3. Read the summary at the end.

   Result: The table has one row for each entry, in the order of the list. The last
   line counts the entries that passed and failed. The runner also writes this text to
   `summary.txt` in the run folder.

### 6.2 G02: Read the results

1. Read the columns of each row. [10.3](#103-summary) gives their meaning.

   Result: A PASS row has a time for Build, CI and Fast.

2. If a row shows FAIL, open `logs/<label>.log` in the run folder.

   Result: The log shows the whole output of the proof and the failure.

3. If a job fails outside Horizon too, add it to `leave_out` of its entry. Then run the
   gate again for that entry only, with `--only <label>`.

   Result: The summary names the job in a line that starts with "Left out of the spec".

4. Examine the lines under the table.

   Result: No line starts with "Left behind" or "NOT REMOVED".

### 6.3 G03: Stop a run

1. While a proof runs, press Ctrl-C in the terminal of the runner.

   Result: The runner prints "SIGINT: stopping the lanes and removing this run's
   resources". The summary shows "no result: stopped by SIGINT" for each entry that
   started and did not end, and "not started: stopped by SIGINT" for each entry that did
   not start. Lines that start with "Removed for an entry that did not end" name the
   Docker resources and temporary folders that the runner removed. The exit code is 130.

2. Find the six hexadecimal digits at the end of the run folder name. Run
   `docker ps --all --format '{{.Names}}' | grep <digits>`.

   Result: The command shows nothing.

3. Do step 2 again with `docker images`, `docker volume ls` and `docker network ls`.

   Result: The commands show nothing. The run folder has no `tmp` folder.

4. If no other setup runs on this computer, run
   `docker ps --all --format '{{.Names}}' | grep horizon-contract-horizon-setup-check-`.

   Result: The command shows nothing. The worker check containers have no run name in
   their names, so step 2 does not find them.

## 7. Pass criteria

- Every entry shows PASS, and the exit code is 0.
- No line under the table starts with "Left behind" or "NOT REMOVED".
- After G03, Docker lists no name that contains the digits of the run.

## 8. Cleanup

The runner removes the Docker resources of each entry and the temporary folders of
each gate lane. It keeps the run folder with the logs.

> **CAUTION:** THE NEXT STEP DELETES A FOLDER. Make sure that the path is a run folder
> in the output folder of the runner, and not a worktree.

1. When you do not need the logs, delete the run folder.

   Result: The output folder does not contain the run folder.

## 9. Record of results

Put the summary table in the pull request. Replace each private repository name with a
synthetic name, for example `example-api`. Keep the logs and the gate list out of the
repository.

## 10. Reference

### 10.1 Gate list

| Key | Required | Meaning |
|---|---|---|
| `repo` | yes | The name of the repository. |
| `path` | yes | The worktree of the repository. A relative path starts at the folder of the list. |
| `spec` | yes | What the proof runs, the same as `proof` in the status file of the setup agent. For GitHub Actions: `workflow`, `jobs`, `event` and `skip` (steps to leave out). Without GitHub Actions: `commands`. |
| `preview` | no | The desktop preview after a passed proof, the same as `preview` in the status file: `command`, `size` (`WxH`) and `kvm`. |
| `label` | no | The name of the entry in the summary and in the log names. The default is `repo`. Use it when two entries prove the same repository. |
| `heavy` | no | `true` for a repository that needs much disk, for example a large Git LFS repository. The runner puts all heavy entries in gate lane 1, so two of them never run at the same time. |
| `leave_out` | no | Jobs of `spec.jobs` that the gate does not run. For example, a `cargo audit` job that fails on an upstream advisory, also outside Horizon. |

The runner refuses a list with an unknown key, two entries with the same label, a path
that is not a folder or a `leave_out` job that `spec.jobs` does not name.

### 10.2 Runner options

| Option | Default | Meaning |
|---|---|---|
| `--out DIR` | `/var/tmp/horizon-setup-gate` | The output folder. Each run makes a run folder in it. The runner refuses a folder below `/tmp`, because `/tmp` can be a RAM file system. |
| `--lanes N` | `2` | The number of gate lanes in parallel. More lanes increase the load. |
| `--min-free-gb N` | `100` | The free disk, in GB, that the runner needs to start. If the runner cannot read the free disk, it does not start. `0` stops the check. |
| `--only PATTERN` | all entries | Run only the entries whose label or repo agrees with the shell pattern. Use it more than one time for more patterns. |
| `--binary PATH` | a new build | Use this horizon test binary, and do not build. |

The runner puts each other entry in the gate lane with the smallest load. A heavy entry
counts as three entries.

The gate stops a proof after 6 hours, far above the limits of each proof phase, so that
it stops only a proof that hangs. It stops a preview that shows no window after 10
minutes. The runner stops a gate lane after 7 hours for each of its entries.

### 10.3 Summary

| Column | Meaning |
|---|---|
| Entry | The label of the entry. |
| Result | PASS when the proof passed and the preview did not fail. Otherwise FAIL. |
| Build, CI, Fast | The seconds of Build image, Run CI in image and Fast rerun. `-` shows a phase that did not run. |
| Preview | `live` and the seconds to the first window, `unavailable` when this computer cannot run the app (for example, no `/dev/kvm`), `failed`, or `-` without a preview. An unavailable preview does not fail the entry. |
| First failure | The failed phase and the first line of its failure, a preview failure, or why the entry has no result. |

| Line under the table | Meaning |
|---|---|
| Left out of the spec | The `leave_out` jobs of an entry. |
| Left behind by an entry that ended, then removed | A Docker resource or a temporary folder that the cleanup of the candidate did not remove. The runner removed it. This is a failure of the candidate. |
| Removed for an entry that did not end | A resource of an entry that a stop or a failed gate lane interrupted. |
| NOT REMOVED, remove it by hand | A resource that the runner could not remove. Remove it with its exact name. |

| Exit code | Meaning |
|---|---|
| `0` | All entries passed, and nothing was left behind. |
| `1` | An entry failed, or something was left behind. |
| `2` | The runner did not start: the list, Docker, the free disk or the build. |
| `128` + the signal number | A stop: `130` for Ctrl-C, `143` for SIGTERM. |

### 10.4 Run folder

| File | Content |
|---|---|
| `summary.txt` | The summary. |
| `results.jsonl` | One line for each entry that ended: `entry`, `run`, `passed`, `seconds`, `phases` (each with `phase`, `seconds` and `failure`), `preview` and `log`. |
| `started.jsonl` | One line for each entry that started, with the names of its Docker resources. |
| `logs/<label>.log` | The whole proof log of the entry. |
| `logs/<label>.preview.png` | The screenshot of the preview, when the image has ImageMagick. |
| `lanes/lane-<N>.json`, `lanes/lane-<N>.log` | The list and the output of the gate process of gate lane N. |

Each gate lane runs one process of the ignored test
`proves_each_listed_repository` in
`crates/horizon-ui/src/app/cloud_panel/production/repository_setup/proof/gate.rs`.
The runner gives it `GATE_LIST`, `GATE_OUT`, `GATE_PREFIX` and a `TMPDIR` of its own.
Each process is in a new session, so the runner stops each process of the gate lane by
its exact PID. The run names have the form `gate-<token>-<lane>-<index>`. `<token>` is
the six hexadecimal digits at the end of the run folder name.
