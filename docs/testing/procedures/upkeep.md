---
procedure: upkeep
feature: Upkeep board (status of the dependency pull request agent)
platforms: [linux]
cost: none
destructive: no
secrets: none
owner: peters
---

# Upkeep test procedure

## 1. Purpose

This procedure proves that the Upkeep board shows the status that an Upkeep agent
writes: the next run, the runs of the last 24 hours, the pull requests by state and
the latest run. It also proves that a pull request row opens its page beside the
board.

## 2. Applicability

- Candidate: a Horizon build with the `upkeep` panel kind.
- Platforms: Linux host.
- This procedure does not test: the Upkeep agent, its worker or its schedule. The
  board reads a synthetic status document.

## 3. Safety

No caution applies. The board only reads a local file, and the synthetic status
names made-up repositories and one public pull request.

## 4. Equipment and preconditions

- An isolated test desktop that a native VNC Device panel shows (see the AGENTS.md
  UI test rules).
- Network access to github.com for task UPK-03.

## 5. Setup

1. Run `python3 -I docs/testing/procedures/upkeep/board.py <folder>`. Use a new,
   empty folder outside the repository.

   Result: The command shows `<folder>/board.yaml`. The folder holds `status.json`
   and `board.yaml`.

2. On the test desktop, start the candidate with
   `horizon --config <folder>/board.yaml --ephemeral`.

   Result: Horizon shows the workspace **Upkeep** with one panel **Upkeep**.

3. Click **Fit**.

   Result: The board fills the view.

## 6. Tasks

### 6.1 UPK-01 — The board shows the status

1. Look at the top of the board.

   Result: The board shows **Upkeep** and the line
   **Codex · every hour · Hetzner cx23, Helsinki · 5 repositories**.

2. Look at **Next run**.

   Result: It shows the time to the next full hour and **Next run at** that hour in UTC.

3. Look at **Last 24 hours**.

   Result: It shows one bar for each hour. A failed run is red, a skipped run has an
   outline, and a green mark shows a run that merged. The total shows **12 merged**.

4. Look at the list.

   Result: The list shows 5 open pull requests in groups **Blocked**, **Fixing**,
   **Needs review**, **Checks running** and **Ready to merge**. Each row shows the
   repository, the number and title, the check state, the time and the note.

5. Look at **Latest run**.

   Result: It shows 5 lines with their UTC times. The last line is
   **Run finished: 1 merged, 1 fixing, 1 waiting on checks**.

### 6.2 UPK-02 — The chips filter the list

1. Click **2 Needs you**.

   Result: The list shows only **Blocked** and **Needs review**.

2. Click **2 Merged, 7 days**.

   Result: The list shows `acme/docs` #17 and `acme/web` #401. It does not show
   `acme/api` #80, which merged 9 days ago.

3. Click **5 Open**.

   Result: The list shows the 5 open pull requests again.

### 6.3 UPK-03 — A row opens its pull request

1. Click the row `horizon` #1516.

   Result: A browser panel **Pull request** opens to the right of the board and shows
   `https://github.com/peters/horizon/pull/1516`. The view shows the board and the
   page.

2. Click the row `api` #86.

   Result: The same browser panel goes to `https://github.com/acme/api/pull/86`,
   where GitHub shows that the page is not found (`acme/api` is made up). No second
   browser panel opens.

### 6.4 UPK-04 — A wrong or missing status is shown as such

1. In `<folder>/status.json`, change the `url` of `acme/web` #409 to
   `https://example.com/acme/web/pull/409`. Wait 2 seconds.

   Result: The board shows all rows.

2. Click the row `web` #409.

   Result: Nothing opens, and the browser panel keeps its page. The board opens only
   the GitHub page of the row's own repository and number.

3. Replace the content of `<folder>/status.json` with `{`. Wait 2 seconds.

   Result: Under the summary line, the board shows in yellow **The Upkeep status is
   not valid** with the parser's reason, and **The board shows the last status it
   read.** The rows stay.

4. Delete `<folder>/status.json`. Wait 2 seconds.

   Result: The yellow line says **The Upkeep status file is gone.** The rows stay.

5. Close Horizon and start it again as in step 2 of the setup.

   Result: The board shows **Upkeep has not reported yet.** and the path that it reads.

## 7. Pass criteria

- UPK-01 to UPK-04 give the results above.

## 8. Cleanup

1. Close Horizon and delete `<folder>`.

## 9. Record of results

Put the results in the pull request. If the run must be kept, write a report in
`docs/testing/reports/` with [the report template](../reports/TEMPLATE.md).
