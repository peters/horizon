---
procedure: connect-github
feature: Connect GitHub (the GitHub card, per-cloud sign-in, access requests)
platforms: [linux]
cost: rents compute
destructive: yes
secrets: [a GitHub account that can create a GitHub App, a RunPod or Hetzner key in Cloud settings]
owner: peters
---

# Connect GitHub test procedure

## 1. Purpose

This procedure proves that Connect GitHub creates the app, that a new cloud signs in
once in each mode, that a reconnect does not ask again, and that the person decides
the access requests of agents.

## 2. Applicability

- Candidate: a Horizon build with Connect GitHub and a worker image whose
  `horizon-worker-check --git-auth` reports `horizon-github-chain-contract=1`.
- Platforms: Linux. Provider: RunPod or Hetzner.
- This procedure does not test: the GitHub client alone. The
  [GitHub App tokens procedure](github-app-tokens.md) tests it.

## 3. Safety

> **CAUTION:** DELETE THE TEST APP IN THE CLEANUP. An app that you do not use stays on
> your account and can receive new authorizations.

> **CAUTION:** DELETE THE TEST CLOUDS IN THE CLEANUP. Each cloud rents compute until
> somebody deletes it.

> **CAUTION:** USE TEST REPOSITORIES ONLY. Agents in the test clouds push to the
> repositories that you install the app on.

## 4. Equipment and preconditions

- A GitHub account with two test repositories, `<owner>/<repo-a>` and
  `<owner>/<repo-b>`. Use a test account if one is available.
- A local checkout of `<repo-a>` whose `origin` is on GitHub, with a committed
  `.horizon/cloud.yml`.
- Cloud settings contain a provider key.
- A browser that is signed in to the GitHub account. It is the default browser.

## 5. Setup

1. Open **Cloud settings**.

   Result: The GitHub card shows **Not connected** and **Connect GitHub**.

## 6. Tasks

### 6.1 G01 — Connect GitHub

1. Click **Connect GitHub**.

   Result: The browser shows GitHub's form to create a GitHub App. The card shows
   **Waiting for GitHub**.

2. Click **Create GitHub App** in the browser.

   Result: GitHub shows the installation page of the new app.

3. Select **Only select repositories**.

   Result: GitHub shows the repository picker.

4. Select `<repo-a>`.

   Result: The picker shows `<repo-a>`.

5. Click **Install**.

   Result: GitHub shows the installed app.

6. Look at the GitHub card in Horizon.

   Result: The card shows **Connected** and **App: horizon-<suffix>**.

7. Run `stat -c '%a' ~/.horizon/cloud/credentials/github-app-*`.

   Result: The command shows `600`.

### 6.2 G02 — Turn on the device sign-in

1. Look at the GitHub card.

   Result: The card shows **Turn on Enable Device Flow in the app's settings, once.**

2. Click **Open app settings**.

   Result: The browser shows the settings of the app.

3. Select **Enable Device Flow** and click **Save changes**.

   Result: GitHub saves the settings.

4. Click **Check again** on the card.

   Result: The card shows **Device sign-in is on.**

5. Make sure that **Ask me for each new cloud** is selected, and click
   **Save settings**.

   Result: Cloud settings close without an error.

### 6.3 G03 — Sign in a new cloud with one click

1. Start a new cloud `gh-ask` from the checkout of `<repo-a>`.

   Result: The card shows the deployment steps.

2. Wait until the card shows **Approve GitHub access for this cloud**.

   Result: The card shows a code such as `WDJB-MJHT`, **Open GitHub** and **Skip**.

3. Click **Open GitHub**.

   Result: The browser shows GitHub's device page. The clipboard holds the code.

4. Paste the code and click **Continue**.

   Result: GitHub shows the app and **Authorize**.

5. Click **Authorize**.

   Result: The card continues to **Ready**. The steps card shows
   **GitHub: signed in as <login> · 1 repository**.

6. In a terminal panel of `gh-ask`, run `git -C <checkout> push --dry-run origin HEAD:refs/heads/gh-ask-check`.

   Result: Git shows the branch that it would create. It asks for no password.

7. In the same panel, run `cat /workspace/.horizon-root/github/state.json`.

   Result: The shell shows **Permission denied**.

### 6.4 G04 — Reconnect without a new sign-in

1. Click **Reconnect** on the `gh-ask` card.

   Result: The card continues to **Ready** and does not show a code. The output
   shows **GitHub: the worker holds current access.**

2. Click **Connect GitHub again** under the GitHub line of the `gh-ask` steps card,
   or in the **Status** tab of its drawer.

   Result: The cloud reconnects and the card shows a new code with **Open GitHub**.

3. Approve the code as in G03 steps 3 to 5.

   Result: The card shows **Ready** and **GitHub: signed in as <login> · 1 repository**.

### 6.5 G05 — Skip the sign-in

1. Start a new cloud `gh-skip` from the same checkout.

   Result: The card shows the code and **Skip** after the worker is ready.

2. Click **Skip: no GitHub for this cloud**.

   Result: The card continues to **Ready**. The steps card shows
   **GitHub: Skipped: this cloud has no GitHub access.**

### 6.6 G06 — Decide an agent's request

1. In an agent panel of `gh-ask`, ask the agent to call `github_access` for
   `<owner>/<repo-b>` with push access and a short reason.

   Result: The agent waits. The cloud shows a request at the top right:
   **Push to <owner>/<repo-b>** with the reason, and the buttons
   **Allow for this cloud** and **Deny**.

2. Click **Deny**.

   Result: The request card closes. The agent reports that access was denied.

3. Ask the agent to call `github_access` for `<owner>/<repo-b>` again, and click
   **Allow for this cloud**.

   Result: The card shows **The GitHub App is not installed on this repository.**
   The request stays and shows again at the next poll. The agent still waits.

4. On GitHub, add `<repo-b>` to the installation of the app.

   Result: GitHub shows two repositories for the app.

5. Wait until the request shows again on the cloud, then click
   **Allow for this cloud**.

   Result: The request card closes. The agent's `github_access` call reports that
   access was allowed.

6. Ask the agent to run `git ls-remote https://github.com/<owner>/<repo-b>`.

   Result: Git shows the references. It asks for no password.

7. Open a new agent panel in `gh-ask`. Ask its agent to run
   `git ls-remote https://github.com/<owner>/<repo-b>`.

   Result: Git shows the references without a new request. Access is per cloud,
   so every agent session of the cloud reaches the repository.

### 6.7 G07 — Sign in new clouds automatically

1. In Cloud settings, select **Automatic (no clicks after the first approval)** and
   click **Save settings**.

   Result: Cloud settings close without an error.

   > **CAUTION:** REVOKE ONLY THE TEST APP. Revoking an app ends the access it has
   > for your account.

2. On GitHub, open **Settings › Applications › Authorized GitHub Apps** and revoke
   the test app.

   Result: GitHub does not list the test app. The app is not authorized for your
   account, as for a person who never used Ask mode. The access of `gh-ask` also
   ends; G06 is complete, so no later task needs it.

3. Start a new cloud `gh-auto` from the same checkout.

   Result: After the worker is ready, the browser shows GitHub's page to authorize
   the app.

4. Click **Authorize**.

   Result: The browser shows **GitHub is connected for this cloud. You can close
   this page.** The `gh-auto` card shows **Ready** and
   **GitHub: signed in as <login> · 1 repository**. It showed no code.

5. Start a new cloud `gh-auto-2` from the same checkout.

   Result: The browser opens a page from GitHub and then shows **GitHub is
   connected for this cloud.** without a click. The card shows **Ready** and the
   GitHub line.

### 6.8 G08 — Disconnect

1. In Cloud settings, click **Disconnect** on the GitHub card.

   Result: The card says that Save settings disconnects, and that running clouds
   keep their access until the app is deleted on GitHub.

2. Click **Save settings**.

   Result: The GitHub card shows **Not connected**.

## 7. Pass criteria

- The app secret file is private, and Horizon keeps no private key of the app.
- In Ask mode a new cloud needs one Authorize click. In Automatic mode only the first
  sign-in of the app needs one, and later clouds need none.
- A reconnect does not ask again.
- A skipped sign-in leaves the cloud **Ready** without GitHub access.
- Agents push without a password and cannot read the stored chain.
- A request for a repository outside the installation is not allowed.
- **Allow for this cloud** and **Deny** reach the agent.
- An allowed repository reaches every agent session of the cloud.

## 8. Cleanup

1. Delete the clouds `gh-ask`, `gh-skip`, `gh-auto` and `gh-auto-2` with **Delete cloud…**.

   Result: The board does not show them. The provider shows no worker for them.

2. On GitHub, open the settings of the app, then **Advanced**.

   Result: GitHub shows **Delete GitHub App**.

3. Click **Delete GitHub App**, type the name and confirm.

   Result: GitHub deletes the app. All its tokens stop working.

4. Delete the branch `gh-ask-check` from `<repo-a>` if it exists.

   Result: GitHub does not show the branch.

## 9. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). If not, put the results in the
pull request. Keep the app name and the logins in the private evidence only.
