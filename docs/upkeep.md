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
click, the view fits the workspace, so the board and the page are both in view.

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
- It puts runs and activity lines in time order before it keeps the newest. Thus
  the document can list them in any order.
- Each bar shows all runs that started in its hour. The merged total counts the same
  runs.
- A row opens a page only when its URL is exactly
  `https://github.com/<owner>/<name>/pull/<number>`. The owner, name and number
  must be those of the row, and the repository must be a plain `owner/name`.
  Other URLs open nothing.
- When a newer document is not valid or the file is gone, the board keeps the last
  valid status. A yellow line tells the cause. The board tries a failed read again
  every two seconds, also when the time and length of the file did not change.

The board does not guarantee that the status is true. The agent writes it, and an
agent can make errors. Check a merge on GitHub before you act on it.
