# Upkeep

Upkeep is one agent on a cloud worker that keeps dependency pull requests moving.
The agent reviews the pull requests that bots such as Dependabot open. It fixes what
an update broke. It squash-merges a pull request when every gate passes for its exact
head. This page describes the parts that exist now.

## The Upkeep board

The `upkeep` panel kind is the board of one Upkeep. The board shows these items:

- The agent, the schedule, the worker and the number of repositories.
- **Next run**: the time to the next run. The card shows **Running** while a run
  continues and **Paused** when the schedule stops.
  - **Late**: the planned run is more than 10 minutes overdue.
  - **No report**: a run still shows as running after two schedule periods, and
    after at least three hours. The worker can be stopped.
- **Last 24 hours**: one bar for each hour, and the total of merged pull requests.
  A red bar shows a failed run. An outline shows a skipped run. A green part shows
  merges.
- The pull requests in groups by state: **Blocked**, **Fixing**, **Needs review**,
  **Checks running**, **Ready to merge**, **Other** (a state that this Horizon does
  not know) and **Merged**. The **Merged, 7 days** chip shows the pull requests that
  merged in the last seven days. The other chips show no merged rows.
- Chips that filter the list: **Open**, **Needs you** (Blocked and Needs review),
  **Fixing**, **Checks running**, **Ready to merge**, **Merged, 7 days**, and
  **Other** only when a row has an unknown state.
- **Latest run**: the last lines that the agent wrote about its work.

A click on a row opens its pull request page in the browser panel **Pull request**
next to the board. The board uses the same panel for the next row. A browser panel
that the board did not open is never used, also when it has the same name. After a
click, the view fits the workspace, so the board and the page are both in view. In a
workspace with a layout preset, the preset places the new panel.

The board is read-only. It reads the status document that the panel `command` names.
When the command is empty, it reads `~/.horizon/upkeep/status.json`. A config file
can open a board:

```yaml
workspaces:
- name: Upkeep
  terminals:
  - name: Upkeep
    kind: upkeep
    command: /path/to/status.json
```

## Trust boundary of the status document

The agent on the worker writes the status document. Horizon copies it to this
computer. The board treats the document as untrusted input.

The board guarantees these items:

- It reads the document on a separate thread, not on the UI thread. It looks for a
  change every two seconds.
- It reads at most 4 MiB and refuses a larger document.
- It reads every entry of a list but keeps at most twice the limit while it reads.
  Then it keeps the most important entries, whatever their order in the document.
- It keeps at most these entries:
  - 500 pull requests. Open pull requests come before merged pull requests.
  - The newest 1,500 runs.
  - The newest 100 activity lines.
  - 1,000 repository names.
- It ignores runs and activity lines that are more than one day in the future.
- It cuts each text to 300 characters.
- It parses the document into a fixed schema. It never runs a part of the document.
  - It ignores unknown fields.
  - A field of the wrong type gets its default value.
  - It leaves out a list entry that it cannot parse, for example a pull request
    without a repository or a number.
  - It shows an unknown state as **Other** and an unknown check state as
    **No checks**.
  - It refuses a time that is out of range in UTC.
  - It refuses a whole document that is nested more than 128 levels deep, the
    limit of the JSON parser.
- It puts runs and activity lines in time order before it keeps the newest. Thus
  the document can list them in any order.
- Each bar shows all runs that started in its hour, up to now. The merged total
  counts the same runs. A run that starts later than now does not count yet.
- A row opens a page only when its URL is exactly
  `https://github.com/<owner>/<name>/pull/<number>`. The owner, name and number
  must be those of the row, and the repository must be a plain `owner/name`.
  Other URLs open nothing.
- When a newer document is not valid or the file is gone, the board keeps the last
  valid status. A yellow line tells the cause. The board reads the file every two
  seconds and compares its content, so it sees every change, also one that keeps the
  time and length of the file.

The board does not guarantee that the status is true. The agent writes it, and an
agent can make errors. Check a merge on GitHub before you act on it.

## The worker loop

Horizon keeps the Upkeep loop on the cloud worker with an exchange. An exchange is
one SSH command. The code is in `crates/horizon-core/src/upkeep/worker.rs` and
`crates/horizon-core/src/upkeep/worker/`. No part of the Horizon window starts an
exchange yet. The exchange does these steps:

1. It writes the loop (`run.sh` and `run.py`), the settings (`config.json`) and
   the agent instructions (`prompt.md`) to `/workspace/home/upkeep`.
2. It writes the session binding in `/workspace/sessions/upkeep-<cloud>`. Thus
   the agents of the loop can ask for GitHub access and open browser panels.
3. It writes the Run now and Pause signals.
4. It starts the loop in the tmux session `upkeep-<cloud>` when the session does
   not run the loop.
5. It reads back the runs, the next run time and the status that the agents wrote.

The loop starts one run on the schedule, or sooner after **Run now**. It starts no
run while the loop has the pause signal. A run lists the open pull requests of the
bots in each repository. It starts one Codex agent for each repository that has
pull requests that changed since the last clean run. The repositories in the
order list run one at a time first. Then the other repositories run side by side.
The number of agents that run side by side depends on the processor cores, the
memory and the peak memory of earlier runs. The loop stops an agent and all of its
processes after 90 minutes. A run that **Run now** started also starts agents for
the repositories that did not change. **Run now** during a run starts no second
run. A pause skips the runs on the schedule while it continues. When an exchange
installed a new loop, the loop starts the new loop between two runs. When the schedule changes, the loop
calculates the next run again. When the loop starts, it records a run that did not
end (for example, because the loop was stopped) as failed. The loop keeps the last
1 MiB of a log of an agent that grew to more than 8 MiB.

## Trust boundary of the worker loop

The worker loop runs as the agent account. The agent can change all files in
`/workspace/home/upkeep`, the loop included. Thus the exchange treats each file in
that folder as untrusted input.

Untrusted input:

- The files in `/workspace/home/upkeep`, `/workspace/sessions/upkeep-<cloud>` and
  `/workspace/session-data/upkeep-<cloud>`. The agent can replace a file with a
  link, a pipe or a folder.
- The status, the runs and the next run time that the exchange reads back.
- The content of the pull requests on GitHub: titles, bodies, comments, code,
  check logs and pages that the agent opens.

Trusted input:

- The settings of the person: the repositories, the bots, the schedule, the
  worker name and the instructions. Horizon examines them before it sends them.

The exchange and the loop guarantee these items:

- The exchange runs as the account that owns `/workspace/home`. On a worker that
  isolates agents, Horizon starts the exchange through
  `horizon-worker-tailnet agent`, as the agent account. On other workers, the
  agents and the exchange run as root. The exchange stops with an error when it
  runs as a different account from the owner of `/workspace/home`. Thus root
  never writes into a folder that the agent account controls.
- The exchange opens each folder without links and makes sure that the folder
  belongs to its account. It does not use a folder that is a link.
- The exchange writes each file to a new temporary file with a random name. Then
  it moves the temporary file into place. When the agent replaced a file with a
  link, the exchange replaces the link. It never writes through the link.
- The exchange stops with an error when a folder has the name of a file that it
  writes.
- The exchange reads only regular files. It does not read a link, a pipe, a
  device or a folder. It reads at most 1 MiB of status and the last 256 KiB of
  runs. It sends back no status when the answer is more than 3 MiB.
- From each run, the exchange sends back only an integer start time, an integer
  end time and a short outcome text. It sends back the next run time only when it
  is an integer in the range of Horizon. Thus a run file of the agent cannot make
  the answer invalid.
- Horizon accepts these settings only:
  - 1 to 50 repositories. Each repository is a GitHub `owner/name` that is
    listed one time.
  - An order list that has only repositories of the list.
  - 1 to 20 bots. Each bot is a GitHub login, `name[bot]` or `app/name`.
  - The Codex agent.
  - A schedule from every 15 minutes to every 24 hours.
  - Instructions of at most 8 KiB and a worker name of at most 200 bytes,
    without control characters. The instructions can have line breaks and tabs.
  - A cloud ID of letters, digits, `-` and `_`, and a revision of 40 or 64
    lowercase hexadecimal characters.
- The loop merges the reports of the agents into one status. It keeps only list
  entries that are valid JSON objects of at most 2 KiB and 32 levels. It keeps at
  most 300 pull requests and the newest 80 activity lines. Thus the status stays
  below the 1 MiB that the exchange reads, and Horizon can parse it.
- When a repository has no open pull requests from the bots, the loop removes
  its open pull requests from the status. It keeps the merged pull requests.
- The live panel of the loop shows lines from the logs of the agents. The loop
  removes control characters from these lines. Thus text from a pull request
  cannot send a control sequence to the terminal panel.
- The loop examines the repositories and the bots in `config.json` again. It
  leaves out the names that Horizon does not accept. Thus no name can start an
  option of `gh`, or change a file name or the agent instructions.
- The loop gives the instructions to Codex on the standard input. Thus
  instructions that start with `-` are not options.
- The loop starts Codex without approvals only when the worker lets agents run
  without approvals: the agents do not run as root, and the worker policy
  `/run/horizon-worker/agent-approvals` is `auto`. On other workers the loop
  starts no agent, and the run fails with the reason. This is the same rule that
  `horizon-worker-run` uses for agent panels.
- The loop gives each agent the session environment of the worker image, as
  `horizon-worker-run` does. When the worker refuses that environment, the run
  starts no agent.

The exchange and the loop do not guarantee these items:

- They do not prevent that the agent changes its own loop, its settings or its
  status. The agent account can write all of these files.
- They do not prevent prompt injection. The agent reads pull requests that other
  people and bots write, and it runs with push access to the repositories of this
  Upkeep. Give an Upkeep only the repositories and the bots that you trust it to
  merge.
- They do not make the status true. The board treats it as untrusted input, as
  the section [Trust boundary of the status document](#trust-boundary-of-the-status-document)
  describes.
