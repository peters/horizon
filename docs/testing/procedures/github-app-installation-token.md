# Test procedure: GitHub App installation token for an agent cloud

This procedure shows that a cloud worker reads and writes GitHub with a GitHub App
installation token, and that no person signs in to GitHub for that worker.

Steps 1 to 3 run with the change that adds the token minting. Steps 4 to 8 run after the
agent cloud path gives the token to a cloud, in the pull request that follows it. A run of
this procedure before that change stops after step 3 and records steps 4 to 8 as not run.

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

4. Start the agent cloud.

   CAUTION: this step rents a paid cloud. Delete it in step 8.

   From an agent panel or an outside MCP client, call `cloud_create` with `submit` for the
   test repository. Poll `cloud_create` with `status` until the phase is `ready`.

   Result: the phase is `ready`, and no card asked for a GitHub sign-in.

5. Read the worker's GitHub state.

   In the cloud's terminal, run:

   ```bash
   horizon-worker-github status
   ```

   Result: the answer names the app's bot login (`<slug>[bot]`) and the test repository, and
   it does not name a person's login.

6. Prove a push and a pull request without a sign-in.

   In the cloud's terminal, commit one file and push a branch, then open a pull request.

   Result: the push succeeds, and GitHub shows the pull request from `<slug>[bot]`.

7. Prove the token is limited.

   In the cloud's terminal, try to push to a repository that the agent policy does not list.

   Result: GitHub refuses the push, and the refusal names the permission or the repository.

8. Delete the cloud.

   CAUTION: this step ends the paid cloud and its workspace.

   Call `cloud_delete` with `submit` for that cloud, then poll `cloud_delete` with `status`.

   Result: the last phase is `done`, the answer holds `absent: true`, and the provider shows
   the cloud as absent.

## Evidence to keep

- The `cloud_create` and `cloud_delete` transcripts, with the phases.
- The `horizon-worker-github status` answer, with the bot login.
- The pull request URL and the commit author line.
- The refusal of step 7.

## Limits

- An installation token ends after one hour. A cloud that Horizon did not touch for longer
  than that has no GitHub access until Horizon mints the next token.
- A commit from the app names the bot but GitHub does not attribute it to the bot account.
- This procedure does not test a person's sign-in path, which stays as it is.
