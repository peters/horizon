---
procedure: connect-github
feature: Connect GitHub (the GitHub card, per-cloud sign-in, access requests, image publishing)
platforms: [linux]
cost: rents compute
destructive: yes
secrets: [a GitHub account that can create a GitHub App, a RunPod or Hetzner key in Cloud settings]
owner: peters
---

# Connect GitHub test procedure

## 1. Purpose

This procedure proves that Connect GitHub creates the app, that a new cloud signs in
once in each mode, that a reconnect does not ask again, that the person decides
the access requests of agents, and that Horizon publishes an image to `ghcr.io`
after one approval.

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
- For G09: a checkout whose `.horizon/cloud.yml` profile builds an image to a test
  package `ghcr.io/<owner>/<test-image>`, and no image repository bound for it in
  **Cloud settings › Container registry**.
- For G11: a test organization `<org>` that the GitHub account owns, with a test
  repository `<org>/<repo-c>`, and a local checkout of `<repo-c>` with a committed
  `.horizon/cloud.yml`.
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

5. Make sure that **Ask me for each new cloud** is selected. Close Cloud settings
   and open them again.

   Result: The card shows **Ask me for each new cloud** selected. The card saves
   the choice itself, also without a provider set up.

### 6.3 G03 — Sign in a new cloud with one click

> **CAUTION:** THE NEXT STEP RENTS COMPUTE FOR `gh-ask`. It costs money until the
> cloud is deleted in the cleanup.

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

> **CAUTION:** THE NEXT STEP RENTS COMPUTE FOR `gh-skip`. It costs money until the
> cloud is deleted in the cleanup.

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

1. In Cloud settings, select **Automatic (no clicks after the first approval)**.
   Close Cloud settings and open them again.

   Result: The card shows **Automatic** selected.

   > **CAUTION:** REVOKE ONLY THE TEST APP. Revoking an app ends the access it has
   > for your account.

2. On GitHub, open **Settings › Applications › Authorized GitHub Apps** and revoke
   the test app.

   Result: GitHub does not list the test app. The app is not authorized for your
   account, as for a person who never used Ask mode. The access of `gh-ask` also
   ends; G06 is complete, so no later task needs it.

> **CAUTION:** THE NEXT STEP RENTS COMPUTE FOR `gh-auto`. It costs money until the
> cloud is deleted in the cleanup.

3. Start a new cloud `gh-auto` from the same checkout.

   Result: After the worker is ready, the browser shows GitHub's page to authorize
   the app.

4. Click **Authorize**.

   Result: The browser shows **Horizon received GitHub's answer and is finishing
   the sign-in. You can close this page.** The `gh-auto` card shows **Ready** and
   **GitHub: signed in as <login> · 1 repository**. It showed no code.

> **CAUTION:** THE NEXT STEP RENTS COMPUTE FOR `gh-auto-2`. It costs money until
> the cloud is deleted in the cleanup.

5. Start a new cloud `gh-auto-2` from the same checkout.

   Result: The browser opens a page from GitHub and then shows **Horizon received
   GitHub's answer** without a click. The card shows **Ready** and the GitHub
   line.

### 6.8 G08 — Disconnect

1. In Cloud settings, click **Disconnect** on the GitHub card.

   Result: The GitHub card shows **Not connected** and says that running clouds
   keep their access until the app is deleted on GitHub. No **Connect GitHub
   again** shows on the cloud cards.

### 6.9 G09 — Publish an image to ghcr.io

> **CAUTION:** THE NEXT STEP MOVES HORIZON'S PUBLISHING SIGN-IN AND DOCKER LOGIN
> ASIDE. Step 5 puts them back. Keep `<evidence>` private: the copies hold tokens.

1. Move the stored publishing sign-in and Horizon's Docker login aside, so Horizon
   asks again:

   ```sh
   mkdir -m 700 -p <evidence>/d-backup
   for f in horizon-github-packages.json config.json; do
     test -e ~/.horizon/cloud/docker/$f && mv ~/.horizon/cloud/docker/$f <evidence>/d-backup/$f
   done; true
   ```

   Result: neither file exists in `~/.horizon/cloud/docker`.

> **CAUTION:** THE NEXT STEP RENTS COMPUTE FOR `gh-publish`. It costs money until the
> cloud is deleted in the cleanup.

2. Start a new cloud `gh-publish` from the checkout for G09.

   Result: After **Build locally**, the card shows **Allow Horizon to publish images
   for you** with a code. The caption says that the permission covers images only.

3. Click **Open GitHub**, paste the code and click **Authorize**.

   Result: The box closes. The output shows **Horizon may now publish images as
   <login>**, and the push continues. `stat -c '%a' ~/.horizon/cloud/docker/horizon-github-packages.json`
   shows `600`.

> **CAUTION:** THE NEXT STEP RENTS COMPUTE FOR `gh-publish-2`. It costs money until
> the cloud is deleted in the cleanup.

4. Start a new cloud `gh-publish-2` from the same checkout.

   Result: The push runs without the publishing box.

> **CAUTION:** THE NEXT STEP DELETES THE PUBLISHING SIGN-IN AND DOCKER LOGIN THAT
> THIS TASK MADE. Delete only these two files.

5. Delete the files this task made and put the ones from step 1 back:

   ```sh
   rm -f ~/.horizon/cloud/docker/horizon-github-packages.json ~/.horizon/cloud/docker/config.json
   for f in horizon-github-packages.json config.json; do
     test -e <evidence>/d-backup/$f && mv <evidence>/d-backup/$f ~/.horizon/cloud/docker/$f
   done; rmdir <evidence>/d-backup; true
   ```

   Result: `~/.horizon/cloud/docker` holds the same files as before step 1, and
   `<evidence>/d-backup` does not exist. On GitHub, **Settings › Applications ›
   Authorized OAuth Apps** still lists **Horizon** when it did before; revoke it
   there if this task authorized it for the first time.

### 6.10 G10 — Pick and clone a private repository

1. Open **New cloud** while GitHub is connected. Under **Where is your code?**,
   click **Pick from your GitHub repositories**.

   Result: The first time, a box asks to sign in to GitHub on this computer, with a
   code in Ask mode. In Automatic mode the browser returns by itself.

2. Approve the code on GitHub if one shows.

   Result: The dialog lists `<owner>/<repo-a>` and `<owner>/<repo-b>`.
   `stat -c '%a' ~/.horizon/cloud/credentials/github-host-*.json` shows `600`.

3. Type `repo-b`, then click `<owner>/<repo-b>`.

   Result: The field shows its link. The dialog says that the repository is
   private and offers **Clone with GitHub** in place of a token field.

4. Click **Clone with GitHub**.

   Result: The clone starts by itself and finishes without a token field. The
   dialog then shows **Preparing cloud…** and the cloud's choices. `git -C <clone> config --get
   remote.origin.url` shows the plain `https://github.com/<owner>/<repo-b>.git` link,
   and `.git/config` holds no token.

5. Close the dialog, open **New cloud** again and click **Pick from your GitHub
   repositories**.

   Result: The list shows without a sign-in.

### 6.11 G11 — Install the app on an organization

1. On GitHub, open the settings of the app, then **Advanced**.

   Result: GitHub shows **Make private**. The app is public, so other accounts can
   install it.

2. Open the checkout of `<repo-c>` and start a cloud `gh-org` in Ask mode. Approve
   the code on GitHub.

   Result: The steps card shows **GitHub: the app is not installed on
   <org>/<repo-c>.** It also says that the app installs on `<org>` only when it is
   public, and it shows the links to the **Advanced** page and the installation
   page.

3. Open the installation page, select `<org>`, then select `<repo-c>` and click
   **Install**.

   Result: GitHub shows the app installed on `<org>`.

4. Click **Connect GitHub** again on the `gh-org` card. Approve the code on GitHub.

   Result: The card shows the cloud connected to `<org>/<repo-c>`. The deployment
   output shows no line of JSON from the worker's GitHub service, and no diagnosis
   names such a line as the root cause.

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
- The first push to `ghcr.io` asks once to publish images, and a later push does not.
- New cloud lists the connected repositories after one sign-in of this computer and
  clones a private one without a token.
- A new app is public. An organization installs it and its cloud gets access.

## 8. Cleanup

> **CAUTION:** THE NEXT STEP DELETES SEVEN CLOUDS AND THEIR WORKSPACES. Delete only
> the clouds of this procedure.

1. Delete the clouds `gh-ask`, `gh-skip`, `gh-auto`, `gh-auto-2`, `gh-publish`,
   `gh-publish-2` and `gh-org` with **Delete cloud…**.

   Result: The board does not show them. The provider shows no worker for them.

2. On GitHub, open the settings of the app, then **Advanced**.

   Result: GitHub shows **Delete GitHub App**.

> **CAUTION:** THE NEXT STEP DELETES THE GITHUB APP AND ENDS EVERY TOKEN IT GAVE
> OUT. Delete only the test app of this procedure.

3. Click **Delete GitHub App**, type the name and confirm.

   Result: GitHub deletes the app. All its tokens stop working.

> **CAUTION:** THE NEXT STEP DELETES A BRANCH. Delete only `gh-ask-check` in the
> test repository.

4. Delete the branch `gh-ask-check` from `<repo-a>` if it exists.

   Result: GitHub does not show the branch.

## 9. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). If not, put the results in the
pull request. Keep the app name and the logins in the private evidence only.
