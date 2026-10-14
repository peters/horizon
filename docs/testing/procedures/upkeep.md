---
procedure: upkeep
feature: Upkeep board, worker loop and New Upkeep (the dependency pull request agent)
platforms: [linux]
cost: rents compute (UPK-10, UPK-13 and UPK-15)
destructive: yes (UPK-12 deletes the cloud that UPK-10 made)
secrets: [GitHub sign-in, Hetzner API token, ChatGPT login of Codex]
owner: peters
---

# Upkeep test procedure

## 1. Purpose

This procedure proves that the Upkeep board shows the status that an Upkeep agent
writes: the next run, the runs of the last 24 hours, the pull requests by state and
the latest run. It also proves that a pull request row opens its page beside the
board, and that **New Upkeep** starts a real Upkeep on a Hetzner cloud.

## 2. Applicability

- Candidate: a Horizon build with the `upkeep` panel kind.
- Platforms: Linux host.
- UPK-01 to UPK-06 read a synthetic status document. UPK-07 runs the worker
  exchange in a local container. UPK-08 to UPK-13 use real GitHub data and a real
  Hetzner cloud. UPK-15 uses a real pull request and a second Hetzner cloud.
- This procedure does not test: the quality of the work of the agent, Renovate
  pull requests, or a schedule longer than one hour.

## 3. Safety

Tasks UPK-01 to UPK-07 only read local files, and the synthetic status names
made-up repositories and one public pull request.

> **CAUTION:** TASK UPK-10 RENTS A HETZNER SERVER AND A VOLUME. The provider bills
> them until UPK-12 deletes the cloud.

> **CAUTION:** IN UPK-10 THE AGENT CAN PUSH TO AND MERGE PULL REQUESTS OF THE
> REPOSITORY THAT YOU SELECT. Select only a repository where you are permitted to
> merge Dependabot pull requests.

> **CAUTION:** TASK UPK-15 RENTS A SECOND HETZNER SERVER AND A VOLUME FOR A TEST CLOUD
> THAT RUNS THE CODE OF A PULL REQUEST. Horizon deletes them when the test ends.

> **CAUTION:** DO NOT RECORD PRIVATE REPOSITORY NAMES. The repository list of New
> Upkeep shows every repository that the GitHub App can reach. Blur or crop them in
> a recording that you publish.

> **CAUTION:** TASK UPK-13 USES A RUNNING UPKEEP CLOUD AND CHANGES ITS REPOSITORIES,
> ITS PAUSE STATE AND ITS RUNS. A run can merge real bot pull requests. Use an Upkeep
> whose repositories you are permitted to change.

## 4. Equipment and preconditions

- An isolated test desktop that a native VNC Device panel shows (see the AGENTS.md
  UI test rules).
- Network access to github.com for task UPK-03.
- For task UPK-07: Docker, a Rust toolchain for this repository, and a Horizon
  worker image (`ghcr.io/peters/horizon-worker-base`) on this computer.
- For tasks UPK-08 to UPK-13: the
  [local device smoke fixture](../../../scripts/device-smoke/README.md) with
  `--native-view`, a persistent session, and Cloud settings with a GitHub sign-in,
  a Hetzner project with free capacity for one `cx23` server, and the ChatGPT login
  of Codex. Bind the full credentials folder into the fixture, not single token
  files: Horizon saves a renewed GitHub sign-in with an atomic rename.
- For task UPK-10: two repositories, `<repo>` and `<second>`, each with at least
  one open Dependabot pull request, both where you are permitted to merge
  Dependabot pull requests.
- A browser that is signed in to the GitHub account of Cloud settings. Each new
  cloud asks for one GitHub authorization.

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

3. Close the **Pull request** panel. On the workspace bar, click **Detach**. In the
   new window, click the row `api` #88.

   Result: A **Pull request** browser panel opens. The detached window fits the
   workspace, so the board and the page are both in view.

4. Click **Attach to Main Window**.

   Result: The workspace is back in the main window.

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

6. Close Horizon. Run `python3 -I docs/testing/procedures/upkeep/board.py <folder>`
   again.

   Result: `<folder>` holds a new `status.json` for the next tasks.

### 6.5 UPK-05 — The board comes back with the session

1. Start the candidate with `horizon --config <folder>/board.yaml`, without
   `--ephemeral`, as a test user whose Horizon home holds no session.

   Result: Horizon shows the board as in UPK-01.

2. Close Horizon and start it again with the same command.

   Result: Horizon restores the session. The board shows the same status from
   `<folder>/status.json`.

### 6.6 UPK-06 — Unusual states are shown as such

1. In `<folder>/status.json`, change the `state` of `acme/api` #88 to `closed`. Wait
   2 seconds.

   Result: The board shows the chip **1 Other**, and the row is in the group
   **Other**. The other rows stay.

2. Change the `state` of `acme/api` #88 back to `waiting_checks`. Wait 2 seconds.

   Result: The **Other** chip is gone.

3. Change `schedule.next_run_at` to a time one hour before now. Wait 2 seconds.

   Result: **Next run** shows **Late** in yellow and **Was due at** that time.

4. Change `schedule.paused` to `true`. Wait 2 seconds.

   Result: **Next run** shows **Paused** and **No runs until resumed**.

### 6.7 UPK-07 — The worker exchange keeps root out of the agent folders

1. Run `HORIZON_TEST_UPKEEP_COMMAND=<folder>/command.sh cargo test -p horizon-core --lib upkeep::worker`.
   Use a new, empty folder outside the repository.

   Result: The tests pass. `<folder>/command.sh` holds the remote command of one
   exchange, as Horizon builds it.

2. Run `docs/testing/procedures/upkeep/worker.sh <image> <folder>/command.sh`.
   `<image>` is the worker image. The script starts a container without network. In
   the container, the agent account puts links to `/etc/hostname` and `/etc/shadow`
   in its Upkeep folders. Then root runs the command two times: first without the
   agent isolation, then with it.

   Result: The script shows `PASS` for each of these items, and it stops with exit
   status 0:
   - `root without isolation is refused`
   - `the exchange answers`
   - `no file outside the folders changed`
   - `the links are replaced by files`
   - `the agent account owns every written path`
   - `the loop runs as the agent account`

3. Run `python3 -B -m unittest discover -s examples/cloud-worker -p test_upkeep_worker.py`.

   Result: The tests pass. They include
   `test_a_run_now_sent_again_with_its_token_starts_one_run`: a **Run now** that is
   sent again with its token starts one run only.

### 6.8 UPK-08 — New Upkeep in the toolbar Menu

1. Start the candidate in the fixture. Click **Menu** in the toolbar.

   Result: The menu shows **New Upkeep** under **CLOUD**, after
   **Add cloud here**.

2. Click **New Upkeep**.

   Result: The dialog **New Upkeep** opens. No new workspace shows.

3. Click **Cancel**.

   Result: The dialog closes. The board did not change.

### 6.9 UPK-09 — New Upkeep finds the repositories

1. Open **New Upkeep** again.

   Result: The dialog shows **Finding your repositories and their open pull
   requests...**. After some seconds, it shows one tab for each owner and the
   repositories of the first owner. Each repository shows `<n> open` or **no open
   pull requests**. The repositories with the most open pull requests come first.

2. Clear **Only with open pull requests**.

   Result: The list also shows the repositories without open pull requests.

3. Type part of the name of `<repo>` in **Filter repositories**.

   Result: The list shows only the repositories that contain the text.

4. Click **Renovate**.

   Result: The dialog searches again. The counts are the open pull requests of
   Dependabot and Renovate.

5. Click **Renovate** again.

   Result: The dialog searches again for Dependabot only.

6. Click the row of `<repo>`.

   Result: The row is selected. The owner tab shows `1`. The summary shows
   **1 repository** and the open pull requests of `<repo>`. **Start Upkeep** is
   available.

7. Click **Clear**.

   Result: No repository is selected, also in the other owner tabs. **Start Upkeep**
   is unavailable and the footer shows **Choose at least one repository.**

### 6.10 UPK-10 — Start Upkeep on Hetzner

> **CAUTION:** THE NEXT STEP RENTS A HETZNER SERVER AND A VOLUME, AND GIVES THE AGENT
> PUSH ACCESS TO `<repo>` AND `<second>`. The agent can merge their Dependabot pull
> requests.

1. Remove the text in **Filter repositories**.

   Result: The list shows all repositories of the owner tab.

2. Select `<repo>` and `<second>`. Click the tab of their owner first when it is
   not open.

   Result: The summary shows **2 repositories**.

3. Click **Start Upkeep**.

   Result: The footer shows **Starting Upkeep...** with a spinner. When the
   checkout of `<repo>` is not on this computer, Horizon clones it first.

4. Wait until the dialog closes.

   Result: The workspace **Upkeep** shows the cloud card and the board. The card
   shows `Hetzner / quick-start`. The board shows **Upkeep has not reported
   yet.**

5. Close Horizon while the card shows **Provision worker** or **Check readiness**.

   Result: Horizon closes. The cloud start stops on this computer.

6. Start Horizon again in the same session.

   Result: The card continues the start.

> **CAUTION:** THE NEXT STEP GIVES THE CLOUD ACCESS TO THE GITHUB ACCOUNT OF CLOUD
> SETTINGS.

7. When the card asks to connect GitHub, finish the authorization in the browser
   that is signed in to GitHub.

   Result: The card shows that GitHub is signed in for 1 repository.

8. Wait until the card shows **Ready**.

   Result: Within one minute, the board shows the agent, **every hour**, the worker
   **Hetzner, always on** and **2 repositories**. The panel **Upkeep agent** opens
   in the workspace and shows the tmux session of the loop.

9. Open the panel picker of the cloud and click **Shell**.

   Result: A shell panel on the worker opens.

10. In the shell panel, type `cat /workspace/home/upkeep/runs.jsonl`.

   Result: Within two minutes after **Ready**, the file shows exactly one run that
   started, also after the restart of steps 5 and 6. The first run does not wait for the
   next full hour. The run started an agent for each repository.

11. Wait until the agent of `<second>` asks for GitHub access to it.

   Result: Horizon allows the request without a prompt. The cloud card shows no
   request. The agent log shows that it can reach `<second>`.

12. Wait until the run ends.

   Result: The board shows the pull requests of `<repo>` with their states, and
   **Latest run** shows the lines of the agent. Each pull request that the agent
   worked on has one comment that starts with `<!-- upkeep -->`.

### 6.11 UPK-11 — Run now, Pause and Repositories

Do this task before UPK-12, on the cloud of UPK-10.

1. On the board, wait until no run continues. Click **Run now**.

   Result: **Run now** becomes unavailable. Within one minute, the board shows
   **Running**, and `/workspace/home/upkeep/runs.jsonl` on the worker has a new run.

2. Click **Pause**.

   Result: Within one minute, **Next run** shows **Paused**. The button shows
   **Resume**. **Run now** is unavailable.

3. Click **Resume**.

   Result: **Next run** shows the next planned time again.

4. Click **Repositories**.

   Result: The dialog **Repositories** opens with the repositories of the Upkeep
   selected.

5. Clear `<second>`. Read the line next to **Save**.

   Result: The line shows `1 removed` and the new number of repositories.

6. Click **Save**.

   Result: The dialog closes. Within one minute, the board shows the new number of
   repositories, and `/workspace/home/upkeep/config.json` on the worker does not
   list `<second>`. Click **Repositories**, select `<second>` and click **Save**.
   Within one minute, `config.json` lists `<second>` again.

7. Click **Repositories**. Change the selection. Click **Cancel**.

   Result: The dialog closes. The board shows the same number of repositories as
   before, and `/workspace/home/upkeep/config.json` on the worker does not change.

8. Make the board approximately 330 px wide with its resize handle.

   Result: The buttons show below the title. No button covers the title or goes
   outside the board. Make the board wide again: the buttons show next to the
   title again.

9. Open a board from a config file, as in Setup step 2.

   Result: That board shows no buttons.

### 6.12 UPK-12 — Remove the Upkeep cloud

> **CAUTION:** THE NEXT STEP DELETES THE SERVER, THE VOLUME AND THE SSH KEY OF THE
> CLOUD. Delete only the cloud that UPK-10 made.

1. On the card, delete the cloud.

   Result: The card shows that the cloud is deleted. In the Hetzner console, the
   server, the volume and the SSH key of the cloud do not exist.

### 6.13 UPK-13 — An agent reads and changes the Upkeep through MCP

> **CAUTION:** THIS TASK USES A RUNNING UPKEEP CLOUD. THE WORKER COSTS MONEY WHILE IT
> RUNS. A RUN CAN MERGE REAL BOT PULL REQUESTS IN THE REPOSITORIES OF THE UPKEEP.

Use the Upkeep cloud of UPK-10 before UPK-12, or another running Upkeep on a current
worker image. Use a stand-in agent panel: a `claude`
script first in `PATH` that writes `HORIZON_BROWSER_ACTOR` and
`HORIZON_BROWSER_HOST_INSTANCE` to a file. Call each operation with
`horizon-browser mcp --connect` and these two values.

1. In the workspace of the Upkeep, open the stand-in agent panel. Call
   `cloud_upkeep` with no operation.

   Result: The answer shows the cloud, the repositories, the bots, `every_minutes`,
   `paused`, the runs and the pull requests that the board shows.

2. Call `cloud_upkeep` `discover`.

   Result: The answer lists repositories with `open_pull_requests`. The Upkeep does
   not change.

3. Call `add` with one repository from step 2 that the Upkeep does not have.

   Result: The answer shows `awaiting_person`. Horizon shows the repository list of
   the Upkeep. Above the list, the box "An agent proposes these repositories for the
   Upkeep of the workspace" names the workspace of the Upkeep and shows the repository
   with a tick. The board shows the old number of repositories.

4. Click **Save** in the repository list.

   Result: The board shows the new number of repositories. `status` lists the
   repository.

5. Call `add` with a different repository. Close the repository list without
   **Save**.

   Result: The Upkeep does not change.

6. Call `remove` with the repository from step 3.

   Result: The answer shows `removed: 1`. The board shows the line "An agent removed
   <repository>, <time> UTC" below its title at once. After the next exchange (some
   seconds), the board shows the number of repositories from before step 4.

7. Call `pause`. Wait for the next exchange (at most one minute), then call `status`.

   Result: The answer to `pause` shows `"pause": "requested"`. The board shows the
   line "An agent paused the Upkeep" at once, and **Paused** after the next exchange.
   `status` shows `paused: true`.

8. Call `resume`, then `run_now`.

   Result: The board shows a run in progress after the next exchange. The board
   shows the lines "An agent asked for a run now" and "An agent resumed the
   Upkeep", newest first.

9. Call `create` with one repository.

   Result: The answer starts with `upkeep_exists`. No dialog opens.

   Call `remove` with every repository of the Upkeep.

   Result: The answer starts with `upkeep_invalid_request`. The Upkeep does not
   change.

10. Call `create` with one repository from step 2 and `authors`
    `["app/github-actions"]`.

    Result: The answer starts with `upkeep_invalid_request`.

11. In a workspace without an Upkeep, open the stand-in agent panel. Call `status`
    and `pause`.

    Result: Both answers start with `upkeep_not_found`. The Upkeep in the other
    workspace does not change.

12. In the workspace of step 11, call `create` with one repository from step 2 (a repository that the GitHub App
    reaches; New Upkeep leaves out a repository that it cannot reach).

    Result: The answer shows `awaiting_person`. **New Upkeep** shows the repository,
    the default settings and the note "An agent proposed these settings". Click
    **Cancel**. No cloud starts.

    Call `create` again with `instructions` that are not the default.

    Result: The note says "An agent proposed these settings and its own instructions
    for the workspace" with the name of the workspace of step 11, and "Read the
    instructions in full before you start." Click **Cancel**.

13. In the **Upkeep agent** panel on the worker, call `cloud_upkeep` `status` with
    `horizon-browser mcp`.

    Result: The answer starts with `upkeep_unavailable`.

### 6.15 UPK-15 — A native test on a test cloud

Do this task before UPK-12, on the cloud of UPK-10. You need a repository `<native>`
in the list of the Upkeep, with an open pull request `<pr>` of a bot of the Upkeep.
The default profile of the `.horizon/cloud.yml` of `<native>` has a desktop, no GPU,
Codex or Claude without approvals, and a build that uses only files in `.horizon`.
Its image starts the agent past its first-run dialogs: for Claude Code, it has the
worker tools of Horizon from #1230 or later.
The image of the profile must be public, or Cloud settings must have a pull login
that Hetzner can use for it. The fixture needs Docker: bind the Docker socket and
the `docker` folder of Cloud settings into it, and set `docker_host`.

> **CAUTION:** THIS TASK RENTS A SECOND HETZNER SERVER AND A VOLUME FOR THE TEST
> CLOUD. Horizon deletes them when the test ends, and after 120 minutes at the
> latest. Make sure in the Hetzner console that they are gone.

> **CAUTION:** THE TEST CLOUD RUNS THE CODE OF `<pr>`. Use a pull request that you
> trust to run on a cloud with the login of your agent.

1. Get the head commit of `<pr>`: `gh pr view <pr> --repo <native> --json headRefOid`.
   In the shell panel of the Upkeep cloud, write the request:

   ```sh
   mkdir -p /workspace/home/upkeep/native/requests
   printf '{"repository": "<native>", "pull_request": <pr>, "head": "<head>", "reason": "native qualification"}' \
     > /workspace/home/upkeep/native/requests/test.json
   ```

   Result: Within one minute, a new card **Test `<name>` #`<pr>`** shows in the Upkeep
   workspace and starts. `/workspace/home/upkeep/native/results/` on the Upkeep worker
   has a file for `<pr>` with the state `starting`.

2. Wait until the test cloud shows **Ready**.

   Result: The panel **Native test agent** opens next to the card, and the desktop
   viewer of the test cloud opens. About 15 seconds later, Horizon pastes the task
   into the agent panel, and the agent starts. The result file shows `running`.

3. Watch the agent panel and the desktop viewer.

   Result: The agent reads `AGENTS.md`, starts the application on the desktop, and
   the desktop viewer shows it.

4. Wait until the agent writes `/workspace/home/native-test/result.json` on the test
   cloud.

   Result: Within one minute, the card of the test cloud shows that it deletes the
   cloud, and then the card closes. The result file on the Upkeep worker shows the
   outcome of the agent (`passed`, `failed` or `blocked`), its summary, the report
   and the names of the evidence files. The evidence is in
   `~/.horizon/upkeep/<cloud>/native/` in the fixture home.

   In the checkout of `<native>` in the fixture home, run `git status` and
   `ls .git/FETCH_HEAD`.

   Result: The branch and the working tree are as before the test, and there is no
   new `FETCH_HEAD`. `git cat-file -t <head>` gives `commit`.

5. Write the same request again (step 1).

   Result: Nothing starts. Horizon tests one head of a pull request one time.

6. Write a request with a head commit that is not the head of `<pr>`.

   Result: No card shows. The result file shows `refused` and names the head of `<pr>`.

7. Remove the files in `/workspace/home/upkeep/native/requests/`.

8. In the Hetzner console, look for the server and the volume of the test cloud.

   Result: They do not exist.

## 7. Pass criteria

- UPK-01 to UPK-13 and UPK-15 give the results above.

## 8. Cleanup

1. Close Horizon and delete `<folder>`.
2. After UPK-12, delete `~/.horizon/upkeep/<cloud>` in the fixture home.

## 9. Record of results

Put the results in the pull request. If the run must be kept, write a report in
`docs/testing/reports/` with [the report template](../reports/TEMPLATE.md).
