# Dependencies

The **Dependencies** panel shows a dependency worker: a cloud worker that keeps
Dependabot pull requests moving across your repositories. The worker reads the
Dependabot settings and `AGENTS.md` of each repository, runs the checks that they
require, and reports its progress to the panel. You review the pull requests in
GitHub.

> **Status:** This version has the panel, the setup steps and the repository
> portfolio. It cannot start a dependency worker in a cloud yet. Until the worker
> service is available, only the [test worker](#try-it-with-the-test-worker) can
> connect.

## Open the panel

1. Click **Dependencies** in the toolbar.

   Result: The Dependencies panel opens in the active workspace. If the panel is
   already open, Horizon shows it.

You can also open Quick Nav and type `Dependencies`.

Close the panel when you do not need it. A worker continues when the panel is closed.

## Set up

The panel shows three steps until a worker reports. A step that waits for an
earlier step stays locked.

### 1. Connect GitHub

GitHub is necessary. Nothing after this step unlocks without it.

1. Click **Connect GitHub…**.

   Result: Cloud settings open.

2. In the **GitHub** card, click **Connect GitHub** and follow the steps in your browser.

   Result: Horizon creates its own GitHub App on your account. The step shows
   **Connected** and the name of the app.

Each worker signs in through this app for its own cloud. See
[Connect GitHub](cloud-workspaces.md#connect-github) for the sign-in modes and
how to disconnect.

### 2. Choose repositories

1. Click **Choose on GitHub**.

   Result: GitHub shows the repository access of the app.

2. Select the repositories that the worker can reach.

The worker maintains the repositories that have a `.github/dependabot.yml`. It
keeps their update groups, schedules and ignore rules. When the worker reports,
the step shows the number of repositories.

### 3. Start a dependency worker

This step is not available in this version. The step says so, and the
**Start worker** button stays disabled.

## Read the portfolio

When a worker reports, the panel shows the portfolio:

- **Agent health** beside the title: the agent state and the age of its last
  heartbeat. If SSH is down, the health is unknown and the panel says so.
- **Filter tiles**: all repositories, the ones that need attention, the ones in
  progress, the queued ones and the complete ones. Click a tile to filter. Click
  it again to show all repositories.
- **Pull requests**: one bar with the verified, in progress, queued, blocked,
  failed and paused pull requests.
- **Search**: repository names, ecosystems and pull request titles.
- **Table**: each repository with its ecosystems, Dependabot groups, open pull
  requests and status.

Click a repository to see its details: its status, each pull request with its
latest check result and a link to GitHub, the parsed Dependabot updates, and its
instructions.

## Change instructions

1. Click **Instructions**.
2. Edit the global instructions, or select **One repository** and edit the
   instructions for that repository.
3. Click **Save to worker**.

   Result: The worker gets the instructions over SSH. It uses them when it starts
   its next task. The dialog shows the saved revision and the revision that the
   worker uses.

If the worker is not reachable, the dialog keeps your draft and tells you why
the save failed.

## Diagnose a worker

**Debug with local agent** collects the health and the instruction revisions of
the worker over SSH. It does not collect prompts, logs or credentials. You can
copy this context, or open it in an agent on this computer.

**Worker terminal** opens a terminal panel that shows the worker's run.

## Try it with the test worker

The [test worker](../scripts/dependencies-fixture/README.md) runs on this
computer. It has 21 synthetic repositories and 38 synthetic pull requests. It
uses real SSH with generated keys and strict host keys. It reads the Dependabot
settings and `AGENTS.md` files of its synthetic repositories and runs their
fixture checks. GitHub, CI, review and merges are simulated, and the panel says
so. The test worker does not use a GitHub account.

1. Start the test worker with a private data folder outside the repository.

   ```sh
   python3 scripts/dependencies-fixture/serve.py --root "$(mktemp -d)" --delay 3
   ```

   Result: The script prints `{"ready": true, ...}` and the path of the folder.

2. Start Horizon with `HORIZON_MAINTENANCE_FIXTURE` set to that folder.

3. Open the Dependencies panel.

   Result: The steps show **Simulated by the test worker**, then the portfolio
   opens.

## Limits

- Horizon cannot start a dependency worker in a cloud yet.
- The panel does not merge, approve or edit pull requests. GitHub does that.
- Only one Dependencies panel opens at a time. It shows one worker.
