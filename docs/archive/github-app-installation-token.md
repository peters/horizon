# Test procedure: GitHub App installation token for an agent cloud

> **Archived 2026-10-27.** Horizon removed the GitHub App installation-token path, because an
> installation token lasts one hour and an agent's cloud must keep pushing for longer than that
> without a person. A cloud now uses this computer's stored GitHub sign-in chain, which its worker
> renews. This procedure is kept as a record. It does not apply to the repository any more.

This procedure shows that a cloud worker reads and writes GitHub with a GitHub App
installation token, and that no person signs in to GitHub for that worker.

Steps 1 to 3 set up the app. Step 4 shows the refusal of a repository that the app does
not reach, before anything is rented. Steps 5 to 9 show the create of an agent's cloud,
which gives the token to the worker, the proof that the worker uses it, and the delete.

Use a test repository that holds no important data, and a test GitHub App.

## Prerequisites

- Horizon runs from a build of the branch under test.
- A GitHub App with `contents: write`, `pull_requests: write` and `metadata: read`, installed
  on the test account.
- The app ID and app slug of that app.
- A cloud settings file that already names the app (`app_id`, `slug`, `client_id`,
  `client_secret_file`).
- A machine user or an agent that may rent one cloud.

## Steps

1. Make an app private key.

   In the app's settings on GitHub, select **Private keys**, then **Generate a private key**.
   GitHub downloads a `.pem` file.

2. Put the key where only this user reads it.

   ```bash
   install -m 0600 ~/Downloads/<app-slug>.<date>.private-key.pem ~/.horizon/cloud/github-app.pem
   ```

3. Name the key in the cloud settings.

   Add `"private_key_file": "/home/<user>/.horizon/cloud/github-app.pem"` to the `github`
   object of `~/.horizon/cloud/settings.json`.

   Then check the load-time refusal. Settings are loaded and validated by the
   `cloud_deploy` example:

   ```bash
   cargo run -p horizon-core --example cloud_deploy -- offers ~/.horizon/cloud/settings.json
   ```

   Give a relative path instead of an absolute one and run it. Result: the answer asks for
   an absolute machine-local path.

   The mode of the key file is checked when Horizon reads the key to mint a token, in step 4.
   Settings load does not read the file, so a key that is missing or readable by other users
   never stops the settings from loading. To see that refusal, change the key's mode to 0644
   and run step 4: the answer names the key file as not a private file. Restore mode 0600.

4. Refuse a repository that the app does not reach.

   Add a second repository to `repositories` of `agent_lifecycle` in the cloud settings.
   This repository must be a local checkout of a GitHub repository that the app is not
   installed on. Then call `cloud_create` with `submit` for it, and poll `cloud_create` with
   `status`.

   Result: the phase is `failed`, the answer starts with `cloud_create_refused`, and the
   text names the GitHub repository. The provider shows no new worker: the refusal comes
   before anything is rented.

   Remove that repository from `repositories` again.

5. Start the agent cloud.

   CAUTION: this step rents a paid cloud. Delete it in step 9.

   From an agent panel or an outside MCP client, call `cloud_create` with `submit` for the
   test repository. Poll `cloud_create` with `status` until the phase is `ready`.

   Result: the phase is `ready`, and no card asked for a GitHub sign-in.

6. Read the worker's GitHub state.

   In the cloud's terminal, run:

   ```bash
   horizon-worker-github status
   ```

   Result: the answer names the app's bot login (`<slug>[bot]`) and the test repository, and
   it does not name a person's login.

7. Prove a push and a pull request without a sign-in.

   In the cloud's terminal, commit one file and push a branch, then open a pull request.

   Result: the push succeeds, and GitHub shows the pull request from `<slug>[bot]`.

8. Prove the token is limited.

   In the cloud's terminal, try to push to a repository that the agent policy does not list.

   Result: GitHub refuses the push, and the refusal names the permission or the repository.

9. Delete the cloud.

   CAUTION: this step ends the paid cloud and its workspace.

   Call `cloud_delete` with `submit` for that cloud, then poll `cloud_delete` with `status`.

   Result: the last phase is `done`, the answer holds `absent: true`, and the provider shows
   the cloud as absent.

## Evidence to keep

- The `cloud_create` and `cloud_delete` transcripts, with the phases.
- The `horizon-worker-github status` answer, with the bot login.
- The pull request URL and the commit author line.
- The refusal of step 4, and the refusal of step 8.

## Limits

- An installation token ends after one hour. The worker then serves no token, its status
  reports `installation_token_expired`, and only this computer installs a new one. A person
  selects **Connect GitHub again** on the cloud's card, which signs the cloud in as that
  person. This procedure does not wait one hour to show that state.
- A commit from the app names the bot but GitHub does not attribute it to the bot account.
- This procedure does not test a person's sign-in path. Without `github.private_key_file`,
  an agent's cloud keeps that path, and `allow_github_sign_in_skip` still decides whether it
  continues without GitHub access.
- The token reaches every repository of the agent lifecycle policy, and not only the
  repository of the cloud.
