# Upkeep

Upkeep is one agent on a cloud worker that keeps dependency pull requests moving: it
reviews the pull requests that bots such as Dependabot open, fixes what the update
broke, and squash-merges a pull request when every gate passes for its exact head.
This page describes the parts that exist so far.

## The Upkeep board

The `upkeep` panel kind is the board of one Upkeep. It shows:

- the agent, the schedule, the worker and the number of repositories;
- **Next run**, or **Running** while a run is in progress, or **Paused**;
- **Last 24 hours**: one bar for each hour, with the runs that failed, were skipped or
  merged pull requests, and the total merged;
- the pull requests by state (**Blocked**, **Fixing**, **Needs review**,
  **Checks running**, **Ready to merge**, **Merged**), with chips that filter them.
  Merged pull requests stay for seven days;
- **Latest run**: the last lines that the agent wrote about its work.

A click on a row opens its pull request page in a browser panel **Pull request**
beside the board. The board reuses that panel for the next row.

The board is read-only. It reads the status document that its panel's `command` names,
or `~/.horizon/upkeep/status.json` when the command is empty. A config file can open
one:

```yaml
workspaces:
- name: Upkeep
  terminals:
  - name: Upkeep
    kind: upkeep
    command: /path/to/status.json
```

## Trust boundary of the status document

The agent on the worker writes the status document, and Horizon copies it to this
computer. The board treats it as untrusted input.

The board guarantees:

- It reads the document on its own thread, never on the UI thread, and at most every
  two seconds.
- It reads at most 4 MiB. A larger document is refused. It keeps at most 500 pull
  requests, the newest 100 runs, the newest 100 activity lines and 1,000 repository
  names, and it cuts each text to 300 characters.
- It parses the document into a fixed schema and never runs anything from it.
  Unknown fields are ignored. A field of the wrong type falls back to its default,
  and a list entry that does not parse (for example a pull request without a
  repository or number) is left out. A state that this Horizon does not know shows
  as **Other**, and an unknown check state as **No checks**.
- It puts runs and activity lines in time order before it keeps the newest, so the
  document can list them in any order. Each hour's bar combines all runs that started
  in that hour, and the merged total counts the same runs.
- A row opens a page only when its URL is exactly
  `https://github.com/<owner>/<name>/pull/<number>` for the row's own repository and
  number, and the repository is a plain `owner/name`. Any other URL opens nothing.
- When a newer document is not valid or the file is gone, the board keeps the last
  valid status and says why in yellow. A read that failed is tried again every two
  seconds, also when the file's time and length did not change.

The board does not guarantee that the status is true. The agent writes it, and an
agent can be wrong. Check a merge on GitHub before you act on it.
