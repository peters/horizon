---
procedure: cloud-panels-a-machine-setup
feature: Cloud panels smoke test, area A (machine setup and credentials)
platforms: [linux]
cost: rents compute   # A09 only
destructive: yes      # A04 removes a binding, A08 revokes a provider credential
secrets: [RunPod API key, Hetzner Cloud API token, coding agent API keys, GHCR read-only pull token, GitHub token for Git]
owner: peters
---

# Cloud panels test procedure, area A: machine setup and credentials

## 1. Purpose

This area makes sure that **Cloud settings…** saves provider keys, Hetzner
placement, coding agents, a private registry credential and a Git credential. It
also makes sure that the settings file holds only paths. An absent account
setting must open a repair form.

## 2. Applicability

- Candidate: a debug build of `origin/main`.
- Platforms: Linux with Xvfb.
- This area does not test: the sign-in of an agent on a worker. Area E tests it.

## 3. Safety

> **CAUTION:** THE OPERATOR ENTERS EACH REAL KEY. Device `type` actions can lose
> characters ([issue #1301](https://github.com/peters/horizon/issues/1301)). A
> changed key can fail later or show in a log.

> **CAUTION:** DO NOT PUT A KEY IN A SCREENSHOT, A RECORDING OR A LOG. A person
> who gets a provider key can rent compute on that account.

> **CAUTION:** DO NOT TURN OFF HETZNER WHILE A HETZNER CLOUD EXISTS. Horizon needs
> the Hetzner settings to stop and delete a Hetzner cloud.

## 4. Equipment and preconditions

- The fixture of [area S](s-test-fixture.md), with a live view.
- For a detailed check of saved keys, use the
  [Cloud settings Replace key procedure](../cloud-settings-replace-key.md).
- The RunPod API key, the Hetzner Cloud API token and the agent API keys. They
  come from the current cloud settings of the operator or the secret store of
  the test account.
- For A08: a GHCR image repository of the test account, an image pinned by
  digest in it, and a classic token with only `read:packages`.
- For A09: the cloud `smoke-a` from D01 and a GitHub token that can read and
  write only the repository of the test account.
- The notes in [machine setup](../../../cloud-workspaces.md#one-time-machine-setup),
  [Hetzner machine settings](../../../cloud-hetzner.md#machine-settings) and
  [private registry bindings](../../../private-registry-bindings.md).

## 5. Setup

1. Make sure that the private home of the fixture has no cloud settings.

   ```sh
   ls <data-home>/.horizon/cloud/settings.json
   ```

   Result: The command shows that the file does not exist.

2. Make the directory for the test files and scripts of the run.

   ```sh
   mkdir -p <data-home>/smoke/bin && chmod 700 <data-home>/smoke
   ```

   Result: The fixture shows the directory at `<home>/smoke/bin`.

## 6. Tasks

### 6.1 A01 — Open Cloud settings from the Cloud menu

1. Click **Cloud** in the menu bar.

   Result: The Cloud menu shows **New cloud…** and **Cloud settings…**.

2. Click **Cloud settings…**.

   Result: The **Cloud settings** dialog opens. It shows the **RunPod**,
   **Hetzner Cloud** and **Coding agents** cards.

3. Take a screenshot at once.

   Result: The first screenshot shows the dialog directly after it opens.

4. Wait 3 seconds.

   Result: The dialog stays open.

5. Take a second screenshot.

   Result: The dialog is at the same position and size as in the first screenshot.

### 6.2 A02 — Save the RunPod key and keep it with a blank replacement

> **CAUTION:** THE OPERATOR PASTES THE RUNPOD KEY. Do not type the key with a
> device action, because characters can change.

1. Ask the operator to paste the RunPod API key in **Paste RunPod API key**.

   Result: The field shows a masked value. The card shows **Unsaved key**.

2. Click **Save settings**.

   Result: The dialog closes.

3. Open **Cloud › Cloud settings…** again.

   Result: The RunPod card shows **Key saved** and **Replace**.

4. Record the SHA-256 of the RunPod key file.

   ```sh
   sha256sum <data-home>/.horizon/cloud/credentials/* > <evidence>/a02-keys.sha256
   ```

   Result: The evidence has the hash of the key file, not the key.

5. Click **Replace** on the RunPod card.

   Result: An empty key field opens. **Keep saved key** is directly below the field.

6. Click **Save settings** with the field empty.

   Result: The dialog closes.

7. Compare the key file with the recorded hash.

   ```sh
   sha256sum -c <evidence>/a02-keys.sha256
   ```

   Result: Each line shows `OK`. The blank replacement kept the saved key.

### 6.3 A03 — Turn on Hetzner and save the token and the placement

1. Open **Cloud › Cloud settings…**.

   Result: The Hetzner Cloud card shows **Off**.

2. Switch on **Use Hetzner Cloud for CPU clouds**.

   Result: The card shows **Paste Hetzner API token** and **Placement preferences**.

   > **CAUTION:** THE OPERATOR PASTES THE HETZNER TOKEN. The token gives access to
   > the full Hetzner project.

3. Ask the operator to paste the Hetzner API token in **Paste Hetzner API token**.

   Result: The field shows a masked value.

4. In **Server types, in order of preference**, type the server types of the test plan in order.

   Result: The field shows the types in the order that you typed them.

5. In **Locations, in order of preference**, type the locations of the test plan in order.

   Result: The field shows the locations in the order that you typed them.

6. Click **Save settings**.

   Result: The dialog closes.

7. Examine the `hetzner` section of the settings file.

   ```sh
   jq '.hetzner' <data-home>/.horizon/cloud/settings.json
   ```

   Result: `token_file` is a path. `server_types` and `locations` have the same
   order as the fields.

### 6.4 A04 — Turn off Hetzner and remove its settings

> **CAUTION:** DO NOT TURN OFF HETZNER WHILE A HETZNER CLOUD EXISTS. Horizon needs
> the Hetzner settings to stop and delete a Hetzner cloud.

1. Open **Cloud › Cloud settings…**.

   Result: The Hetzner Cloud card shows **Key saved**.

2. Switch off **Use Hetzner Cloud for CPU clouds**.

   Result: The card shows **Off**.

3. Click **Save settings**.

   Result: The dialog closes.

4. Examine the settings file.

   ```sh
   jq 'has("hetzner")' <data-home>/.horizon/cloud/settings.json
   ```

   Result: The output is `false`. The Hetzner section is not in the settings.

5. Do A03 again.

   Result: Hetzner is on again. The other areas need it.

### 6.5 A05 — Select the coding agents and their sign-in

1. Open **Cloud › Cloud settings…**.

   Result: The Coding agents card shows **Codex** and **Claude**.

2. Select **Codex** and **Claude**.

   Result: The card shows a sign-in choice for each agent: **API key** or
   **Subscription login**.

3. For Claude, select **API key**.

   Result: The card shows **Paste API key** below **Claude**.

   > **CAUTION:** THE OPERATOR PASTES THE AGENT KEY. Do not type the key with a
   > device action, because characters can change.

4. Ask the operator to paste the Claude API key.

   Result: The field shows a masked value.

5. For Codex, select **API key** or **Subscription login**, as the test plan tells you.

   Result: The card shows the choice. With **API key**, the operator pastes the Codex key.

6. Click **Save settings**.

   Result: The dialog closes.

7. Open **Cloud › Cloud settings…** again.

   Result: Each agent with **API key** shows **Key saved**. The **Your workspace**
   card shows both agents.

8. Click **Cancel**.

   Result: The dialog closes. E02 and E03 make sure that the keys work.

### 6.6 A06 — Open the repair form for missing account settings

Do this task after B02. It needs the synthetic repository.

1. Start a second persistent launcher with a new, empty state directory.

   ```sh
   python3 <run>/launcher/serve.py --horizon <run>/bin/horizon \
     --native-view --state <run>/fixture-repair
   ```

   Result: A second candidate starts with no cloud settings.

2. Open a Device panel for the `vnc_address` of the second fixture.

   Result: The Device panel shows a live view of the second fixture.

3. Copy the synthetic repository to the private home of the second fixture.

   ```sh
   mkdir -p <run>/fixture-repair/data/home/smoke && cp -a <data-home>/smoke/app <run>/fixture-repair/data/home/smoke/
   ```

   Result: The second fixture shows the repository at `<home>/smoke/app`.

4. In the second fixture, open **Cloud › New cloud…**.

   Result: The New cloud dialog opens.

5. Type `<home>/smoke/app` as the repository.

   Result: The field shows the repository.

6. Click **Read .horizon/cloud.yml**.

   Result: The dialog loads the profiles. **BEFORE YOU START** shows that a key is not available.

7. Type `smoke-repair` in **Cloud title**.

   Result: The title shows in the field.

8. Press Enter.

   Result: The repair form opens. It keeps the title `smoke-repair` and shows
   **Save and start** and **Cancel**.

   > **CAUTION:** DO NOT CLICK **Save and start**. It saves the settings and
   > starts the cloud. The provider charges money for the worker.

9. Click **Cancel**.

   Result: The form closes. No cloud starts. The board has no cloud `smoke-repair`.

10. Close the Device panel of the second fixture.

    Result: The Device panel closes. The second fixture continues.

11. Stop the second fixture with Ctrl-C.

    Result: Only the first fixture continues.

### 6.7 A07 — Make sure that the settings file holds only paths

1. Show the mode of the settings file and of the credential files.

   ```sh
   cd <data-home>/.horizon/cloud && stat -c '%a %n' settings.json credentials credentials/*
   ```

   Result: `settings.json` and each credential file have mode `600`. The
   `credentials` directory has mode `700`.

2. List the keys of the settings file that end in `_file`.

   ```sh
   jq '[paths(scalars) as $p | select($p[-1] | tostring | endswith("_file")) | getpath($p)]' settings.json
   ```

   Result: Each value is an absolute path.

3. Search the settings file for key prefixes.

   ```sh
   grep -c -E 'rpa_|tskey-|sk-|ghp_|github_pat_' settings.json
   ```

   Result: The output is `0`.

4. Search the settings file for the content of each saved credential file.

   ```sh
   for f in credentials/*; do grep -q -F -f "$f" settings.json && echo "found: $f"; done; echo done
   ```

   Result: The output is only `done`. The settings file contains no saved
   credential, for example the Hetzner token. The command shows no secret.

### 6.8 A08 — Validate, examine and revoke a private registry credential

1. Open **Cloud › Cloud settings…**.

   Result: The **Container registry** card shows **Add image repository**.

2. Click **Add image repository**.

   Result: The card shows the fields for the image repository, the pull login and
   the publish login.

3. Type the GHCR image repository of the test account.

   ```text
   ghcr.io/<test-owner>/<image>
   ```

   Result: The field shows the repository.

4. Type the user name of the token in **Worker pull username**.

   Result: The field shows the user name. It is not a secret.

   > **CAUTION:** THE OPERATOR PASTES THE PULL TOKEN. Use a classic token with only
   > `read:packages`. A token with more scopes gives more access than the worker needs.

5. Ask the operator to paste the token in **Read-only pull credential**.

   Result: The field shows a masked value.

6. Click **Save settings**.

   Result: The dialog closes. The settings file has a reference to the token file, not the token.

7. Open **Cloud › Cloud settings…** again.

   Result: The registry card shows **Needs validation** and a **Pull generation** value.

8. Type the image, pinned by digest, in **Immutable image to validate (repository@sha256:…)**.

   Result: The field shows `ghcr.io/<test-owner>/<image>@sha256:<digest>`.

   > **CAUTION:** THIS STEP SENDS THE PULL TOKEN TO RUNPOD. Horizon makes a
   > provider pull credential with it. Use only the dedicated read-only token.

9. Click **Validate pull access**.

   Result: The card shows **Pull access verified**, the image, the scope and the expiry.

   > **CAUTION:** SEND THE RUNPOD KEY ONLY TO THE RUNPOD API. The header file
   > contains the key. Do not show the file or the request headers.

10. Find the new provider pull credential in the RunPod registry list.

    ```sh
    bash <run>/runpod-list.sh registries
    ```

    Result: The list has one new line. Write its ID in the resource ledger as a RunPod registry credential.

11. Click **Status**.

    Result: The card shows the generation, the provider state and the last validated image.

12. Record the generation that the card shows.

    Result: You have the value for the next steps.

13. Write the status action to `<data-home>/smoke/registry-status.json`.

    ```json
    {"operation":"status","repository":"ghcr.io/<test-owner>/<image>","generation":"<generation>"}
    ```

    Result: The file contains no secret.

14. In the fixture terminal, run the status action with the CLI.

    ```sh
    <run>/bin/cloud_deploy registry <home>/.horizon/cloud/settings.json <home>/smoke/registry-status.json
    ```

    Result: The output shows the same generation, provider state and image as the card.

15. Write a script that sends the same action to the MCP tool `cloud_registry`.

    ```sh
    cat > <data-home>/smoke/bin/registry-mcp.sh <<'EOF'
    #!/usr/bin/env bash
    set -eu
    call=$(jq -c '{jsonrpc: "2.0", id: 2, method: "tools/call",
      params: {name: "cloud_registry", arguments: .}}' ~/smoke/registry-status.json)
    { printf '%s\n' \
      '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"smoke","version":"1"}}}' \
      '{"jsonrpc":"2.0","method":"notifications/initialized"}' \
      "$call"
      sleep 10; } | <run>/bin/cloud_deploy registry-mcp ~/.horizon/cloud/settings.json
    EOF
    ```

    Result: The script is in `<data-home>/smoke/bin/registry-mcp.sh`.

16. In the fixture terminal, run the script.

    ```sh
    bash ~/smoke/bin/registry-mcp.sh
    ```

    Result: The answer with `"id":2` shows the same status as the CLI.

    > **CAUTION:** THIS STEP DELETES THE PROVIDER PULL CREDENTIAL. Revoke only the
    > credential that this run made. A worker that needs it cannot pull its image
    > after a restart.

17. In Cloud settings, click **Revoke pull binding**.

    Result: The card shows that the provider pull credential is revoked.

18. Run the CLI status action again.

    Result: The output shows that the provider pull credential is revoked.

19. Mark the RunPod registry credential of step 10 as deleted in the resource ledger.

    Result: The ledger shows no active credential for the A08 repository.

    > **CAUTION:** REVOKE ONLY THE GHCR TOKEN THAT THIS RUN MADE. Other tokens of
    > the account can give access to other work.

20. Ask the operator to revoke the GHCR token at GitHub.

    Result: The token cannot read the image. Horizon does not revoke the token at its issuer.

21. Mark the GHCR pull token as revoked in the resource ledger.

    Result: The ledger shows the token of A08 as revoked.

The `runpod-build` profile needs a second entry with a push credential.
G02 and L05 use this entry. Keep it until the end of area L.

22. In the **Container registry** card, click **Add image repository**.

    Result: The card shows **New image repository** and empty fields.

23. Type `<build-repository>` in the **Image repository** field.

    Result: The field shows the repository of the build profile.

    > **CAUTION:** THE OPERATOR MUST ENTER THE CREDENTIALS. A device `type` action
    > can lose characters, and a recording can show a credential.

24. Let the operator fill the pull fields and the **Publishing credential** fields for `<build-repository>`.

    Result: The pull fields and the push fields are full. The credentials do not show.

    > **CAUTION:** SAVE ONLY CREDENTIALS FOR THE TEST REPOSITORY. Use a read-only
    > pull credential and a push credential for that repository only.

25. Click **Save settings**.

    Result: The card lists `<build-repository>`. The settings file contains only file references.

### 6.9 A09 — Send a Git credential to the worker and remove unbound keys

Do this task after D01. It uses the cloud `smoke-a`.

1. Ask the operator to write the GitHub token to a private file.

   ```text
   <data-home>/.horizon/cloud/credentials/github
   ```

   Result: The file has mode `600`. Nobody shows its content.

2. Add a `git_credentials` entry for `<repo>` to the settings file.

   ```sh
   cd <data-home>/.horizon/cloud && jq '.git_credentials = [{"local_repository":"<home>/smoke/app","repository":"<test-owner>/app","token_file":"<home>/.horizon/cloud/credentials/github","author_name":"Smoke Test","author_email":"smoke@example.invalid"}]' settings.json > settings.new && chmod 600 settings.new && mv settings.new settings.json
   ```

   Result: The settings file has one Git entry. It contains a path, not the token.

   > **CAUTION:** THIS STEP SENDS THE GITHUB TOKEN TO THE WORKER. Use only a token
   > that can reach the repository of the test account.

3. On the card of `smoke-a`, click **Reconnect cloud**.

   Result: The card shows Ready.

4. In a worker shell of `smoke-a`, show the mode of the credential file.

   ```sh
   stat -c '%a %n' /run/horizon-credentials /run/horizon-credentials/github.json
   ```

   Result: The directory has mode `700` and the file has mode `600`. Do not show
   the content of the file.

5. Remove the Git entry from the settings file.

   ```sh
   cd <data-home>/.horizon/cloud && jq 'del(.git_credentials)' settings.json > settings.new && chmod 600 settings.new && mv settings.new settings.json
   ```

   Result: The settings file has no Git entry.

6. On the card of `smoke-a`, click **Reconnect cloud**.

   Result: The card shows Ready.

7. In the worker shell, look for the credential file.

   ```sh
   ls /run/horizon-credentials/github.json
   ```

   Result: The command shows that the file does not exist.

8. In Cloud settings, select **Subscription login** for Claude.

   Result: The Claude section shows **Subscription login**.

9. Click **Save settings**.

   Result: Claude has no API key in the settings.

10. On the card of `smoke-a`, click **Reconnect cloud**.

    Result: The card shows Ready.

11. In the worker shell, look for the Claude key file.

    ```sh
    find /workspace -name 'anthropic-api-key*' 2>/dev/null
    ```

    Result: The output is empty. The reconnect removed the unbound key file.

    > **CAUTION:** THE OPERATOR PASTES THE AGENT KEY. Do not type the key with a
    > device action, because characters can change.

12. In Cloud settings, select **API key** for Claude again and ask the operator to paste the key.

    Result: After **Save settings**, Claude shows **Key saved**.

13. On the card of `smoke-a`, click **Reconnect cloud**.

    Result: The card shows Ready. The reconnect sends the saved key to the worker.

14. In the worker shell, look for the Claude key file.

    ```sh
    find /workspace -name 'anthropic-api-key*' 2>/dev/null
    ```

    Result: The output shows one file. E02 can sign in with the key.

    > **CAUTION:** REVOKE ONLY THE GITHUB TOKEN THAT THIS RUN MADE. Other tokens of
    > the account can give access to other work.

15. Ask the operator to revoke the GitHub token at GitHub.

    Result: The token does not give access. Removal from the worker does not revoke it.

16. Mark the GitHub token as revoked in the resource ledger.

    Result: The ledger shows the token of A09 as revoked.

## 7. Pass criteria

- A01 opens the dialog. The dialog does not move after it opens.
- A02 keeps the same key file after a blank replacement.
- A03 saves the token as a file path and keeps the order of the placement fields.
- A04 removes the `hetzner` section.
- A05 saves the agent choices and their sign-in modes.
- A06 opens the repair form with the title and starts no cloud.
- A07 finds only paths, modes `600` and `700`, and no literal secret.
- A08 validates the pull credential, shows the same status in the UI, the CLI and
  MCP, and revokes the provider pull credential.
- A09 puts the Git credential on the worker with mode `600` and removes it after
  the Git entry goes away. The reconnect removes the unbound agent key file.

## 8. Cleanup

1. Make sure that the settings file has no Git entry of this run.

   ```sh
   jq '.git_credentials' <data-home>/.horizon/cloud/settings.json
   ```

   Result: The output is null or empty.

2. Show the registry entries in the settings file.

   ```sh
   jq '[.registries.bindings[]? | .repository]' <data-home>/.horizon/cloud/settings.json
   ```

   Result: The list shows `<build-repository>` and the A08 repository. The cleanup of
   [area L](l-lifecycle.md) revokes the `<build-repository>` entry.

3. Delete `<data-home>/smoke/bin/registry-mcp.sh`.

   Result: No test file of this area stays in the private home.

## 9. Record of results

Write the results in the report of the run. Use the
[report template](../../reports/TEMPLATE.md). Keep private evidence out of the
repository.
