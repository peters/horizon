---
procedure: claude-setup-token
feature: Claude with a setup token (Cloud settings choice, token given to each cloud)
platforms: [linux, macos]
cost: rents compute
destructive: yes
secrets: [a Claude setup token made with `claude setup-token` by the operator, a RunPod or Hetzner key in Cloud settings]
owner: peters
---

# Claude with a setup token test procedure

## 1. Purpose

This procedure proves that Cloud settings keeps a Claude setup token like an agent API
key, shows the day that it runs out, and gives it to each cloud that runs Claude, so
that Claude on the worker starts signed in.

## 2. Applicability

- Candidate: a Horizon build with **Setup token** for Claude in Cloud settings, and a
  worker image built from the same commit or later.
- Platforms: Linux and macOS. Provider: RunPod or Hetzner.
- This procedure does not test: the end of a token after one year, or a token that
  Claude refuses. Unit tests cover the expiry date.

## 3. Safety

> **CAUTION:** DO NOT SHOW THE SETUP TOKEN IN A RECORDING, A SCREENSHOT OR A REPORT.
> It gives access to the Claude subscription of the operator for one year.

> **CAUTION:** DELETE THE TEST CLOUD IN THE CLEANUP. The cloud rents compute until
> somebody deletes it.

## 4. Equipment and preconditions

- The operator has a Claude subscription and Claude Code on a computer.
- Cloud settings has a RunPod or Hetzner key and a working SSH identity.
- A test repository with a `.horizon/cloud.yml` profile that permits Claude, or quick
  start.
- `jq` is installed on this computer.

## 5. Setup

1. Ask the operator to run `claude setup-token` on their computer and to keep the
   token ready. Do not write the token.

   Result: The operator has a token.

2. Keep a copy of the Claude bindings of this computer, so that the cleanup can put
   them back. The copy holds only file paths and a date, not a key or a token.

   ```sh
   (umask 077; jq '{claude_token, anthropic_api_key_file}' ~/.horizon/cloud/settings.json > ~/.horizon/cloud/claude-bindings-before.json)
   ```

   Result: The command stops with exit code 0.

## 6. Tasks

### 6.1 T01 — The settings ask for a token

1. Open **Menu › Cloud settings**.

   Result: The Coding agents card shows **Claude**.

2. Make sure that **Claude** is selected. Select **Setup token**.

   Result: The card shows the choices **API key**, **Setup token** and **Sign in on
   the worker**. The field shows **Paste setup token**. The text below the field
   tells you to run `claude setup-token`. The card status is **Needs a key**. The
   banner says "Paste the Claude setup token, or choose another sign-in."

> **CAUTION:** THE OPERATOR PASTES THE TOKEN. Do not type the token with a device
> action, because characters can change.

3. Ask the operator to paste the token.

   Result: The field shows a masked value. The card status is **Unsaved key**.

### 6.2 T02 — Save the token

1. Click **Save settings**.

   Result: The dialog closes.

2. Look at the saved settings.

   ```sh
   jq '{claude_token, anthropic_api_key_file}' ~/.horizon/cloud/settings.json
   ```

   Result: `claude_token.file` is a path under `~/.horizon/cloud/credentials`.
   `claude_token.saved_at` is the time of the save, in seconds since 1970.
   `anthropic_api_key_file` is `null`. The output does not contain the token. Write
   down the path of `claude_token.file` for the cleanup.

3. Look at the mode of the token file. Do not show its contents.

   ```sh
   ls -l "$(jq -r .claude_token.file ~/.horizon/cloud/settings.json)" | cut -c1-10
   ```

   Result: The output is `-rw-------`, on Linux and on macOS.

4. Open **Menu › Cloud settings** again.

   Result: Claude shows **Setup token** and **Saved on this computer**. The text
   below says "Saved <today>. Runs out about <today one year later>."

### 6.3 T03 — A cloud starts Claude signed in

> **CAUTION:** THIS TASK RENTS COMPUTE AND SENDS THE SETUP TOKEN TO THE WORKER. Use
> a test cloud.

1. Start a cloud from the test repository with a profile that permits Claude.

   Result: The card shows Ready.

2. Open a Shell panel on the cloud. Look for the token file. Do not show its
   contents.

   ```sh
   ls -l /workspace/credentials/claude-oauth-token; ls /workspace/credentials/anthropic-api-key
   ```

   Result: The first file exists, with mode `-rw-------`. The second file does not
   exist.

3. In the same shell, count the token variable. Do not show its value.

   ```sh
   env | grep -c '^CLAUDE_CODE_OAUTH_TOKEN='
   ```

   Result: The output is `1`.

4. Open a Claude panel on the cloud. Ask Claude to say "ready".

   Result: Claude answers. Claude does not ask you to sign in.

### 6.4 T04 — Resume refuses a missing token file

1. On the card of the test cloud, click **Stop worker**.

   Result: The card shows that the cloud is stopped.

2. Move the token file away. Use the path that you wrote down in T02 step 2.

   ```sh
   mv <path of T02 step 2> <path of T02 step 2>.away
   ```

   Result: The command stops with exit code 0.

3. Click **Resume worker**.

   Result: The card shows a refusal that names the setup-token file and says that
   the file is missing. The card still shows that the cloud is stopped. The provider
   console shows no started worker for this cloud.

4. Move the token file back. Click **Resume worker**.

   ```sh
   mv <path of T02 step 2>.away <path of T02 step 2>
   ```

   Result: The card shows Ready.

### 6.5 T05 — Another sign-in removes the token from the worker

1. In Cloud settings, select **Sign in on the worker** for Claude. Click **Save
   settings**.

   Result: `jq .claude_token ~/.horizon/cloud/settings.json` shows `null`.

2. On the card of the test cloud, click **Reconnect cloud**.

   Result: The card shows Ready.

3. In a new Shell panel on the cloud, look for the token file.

   ```sh
   ls /workspace/credentials/claude-oauth-token
   ```

   Result: The file does not exist.

## 7. Pass criteria

- T01 to T05 give the results above.
- No screenshot, recording, log or report contains the token.

## 8. Cleanup

> **CAUTION:** DELETE ONLY THE TEST CLOUD OF THIS RUN. Other clouds can hold work.

1. Delete the test cloud with **Delete resources permanently**.

   Result: The cloud is not in the cloud list.

2. Put back the Claude bindings of setup step 2. Then remove the copy.

   ```sh
   cd ~/.horizon/cloud && jq --slurpfile before claude-bindings-before.json '.claude_token = $before[0].claude_token | .anthropic_api_key_file = $before[0].anthropic_api_key_file | if .claude_token == null then del(.claude_token) else . end' settings.json > settings.new && chmod 600 settings.new && mv settings.new settings.json && rm claude-bindings-before.json
   ```

   Result: `jq '{claude_token, anthropic_api_key_file}' ~/.horizon/cloud/settings.json`
   shows the values from before setup step 2.

3. Remove the token file that T02 saved. Use the path that you wrote down in T02
   step 2. Do not show the contents of the file.

   ```sh
   rm <path of T02 step 2>
   ```

   Result: The file does not exist.

4. Ask the operator if they made the token only for this test. If so, ask the
   operator to revoke it in their Claude account.

   Result: The operator kept or revoked the token.
