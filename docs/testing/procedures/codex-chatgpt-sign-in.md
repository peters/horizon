---
procedure: codex-chatgpt-sign-in
feature: Sign in with ChatGPT for Codex (Cloud settings sign-in, login given to each cloud)
platforms: [linux, macos]
cost: rents compute
destructive: yes
secrets: [a ChatGPT account with a plan that includes Codex, a RunPod or Hetzner key in Cloud settings]
owner: peters
---

# Sign in with ChatGPT for Codex test procedure

## 1. Purpose

This procedure proves that Cloud settings signs Codex in with ChatGPT once on this
computer, that each new or reconnected cloud starts Codex signed in to that account,
and that a renewal on a worker comes back to this computer.

## 2. Applicability

- Candidate: a Horizon build with **ChatGPT sign-in** for Codex in Cloud settings.
- Platforms: Linux and macOS. Provider: RunPod or Hetzner.
- This procedure does not test: Codex API keys, Claude sign-in or the renewal of a
  login by Horizon while a cloud stays connected.

## 3. Safety

> **CAUTION:** DELETE THE TEST CLOUDS IN THE CLEANUP. Each cloud rents compute until
> somebody deletes it.

> **CAUTION:** DO NOT RUN `codex logout` OR `codex login` ON A WORKER. Both revoke the
> login that every cloud of this computer shares.

> **CAUTION:** DO NOT SHOW `auth.json` IN A RECORDING OR A REPORT. It holds the
> tokens of the ChatGPT account.

## 4. Equipment and preconditions

- The Codex CLI is installed on this computer. `codex --version` shows a version.
- A browser on this computer can open the ChatGPT sign-in page.
- A ChatGPT account. The operator signs in. Do not write its password.
- Cloud settings has a RunPod or Hetzner key and a working SSH identity.
- A test repository with a `.horizon/cloud.yml` profile that permits Codex, or quick
  start.

## 5. Setup

1. Record the hash of `~/.codex/auth.json`, if the file exists. Use
   `sha256sum ~/.codex/auth.json`.

   Result: You have a hash, or the file does not exist.

2. Make sure that `~/.horizon/cloud/credentials/codex/auth.json` does not exist.

   Result: The file does not exist.

## 6. Tasks

### 6.1 C01 — The settings ask for a sign-in

1. Open **Cloud › Cloud settings…**.

   Result: The Coding agents card shows **Codex**.

2. Select **Codex**. Select **ChatGPT sign-in**.

   Result: The card shows **Sign in with ChatGPT** and the text "Sign in once with
   your ChatGPT plan". The card status is **Sign in to ChatGPT**. The banner says
   "Sign in with ChatGPT so each cloud starts Codex signed in."

3. Select **API key**, then select **ChatGPT sign-in** again.

   Result: The API key field goes, then the sign-in button comes back.

### 6.2 C02 — Cancel a sign-in

1. Click **Sign in with ChatGPT**.

   Result: The browser opens the ChatGPT sign-in page. The card says "Finish in your
   browser". **Save** is not available.

2. Do not sign in. Click **Cancel sign-in**.

   Result: The card shows **Sign in with ChatGPT** again, without an error.
   `~/.horizon/cloud/credentials/codex/auth.json` does not exist.

### 6.3 C03 — Sign in

1. Click **Sign in with ChatGPT**.

2. Ask the operator to sign in with the ChatGPT account in the browser.

   Result: The browser shows that the sign-in is complete. The card shows
   "Signed in as" with the email of the account and its plan, and **Sign out**. The
   card status is **Ready**.

3. Examine `~/.horizon/cloud/credentials/codex`. Use `ls -la`.

   Result: The directory has mode `drwx------`. `auth.json` has mode `-rw-------`.

4. Examine the hash of `~/.codex/auth.json` again.

   Result: The hash is the same as in the setup, or the file still does not exist.

5. Click **Save**. Close and open **Cloud settings** again.

   Result: Codex shows **ChatGPT sign-in** and "Signed in as" with the same email.

### 6.4 C04 — A new cloud starts Codex signed in

1. Create a new cloud with Codex from the test repository.

   Result: The cloud becomes ready.

2. Open a Codex panel on the cloud.

   Result: Codex starts without a sign-in prompt and accepts a short prompt, for
   example "Say hello".

3. In a Shell panel on the cloud, run `ls -l /workspace/home/.codex/auth.json`.

   Result: The file has mode `-rw-------`. Its owner is the agent user when the
   worker isolates agents, or `root` when it does not.

### 6.5 C05 — A worker renewal comes back

1. On this computer, record the `last_refresh` value of the login. Use
   `python3 -c 'import json,os; print(json.load(open(os.path.expanduser("~/.horizon/cloud/credentials/codex/auth.json")))["last_refresh"])'`.

   Result: You have a time.

2. In the Shell panel on the cloud, set a later `last_refresh` in the worker's login.
   Use `python3 -c 'import json; p="/workspace/home/.codex/auth.json"; a=json.load(open(p)); a["last_refresh"]="2099-01-01T00:00:00Z"; open(p,"w").write(json.dumps(a))'`.

   Result: The command ends without an error.

3. Reconnect the cloud.

   Result: The cloud becomes ready. The `last_refresh` of the login on this computer
   is `2099-01-01T00:00:00Z`.

4. Sign out in Cloud settings and sign in again with the same account.

   Result: The login on this computer has a new `last_refresh`, earlier than 2099.

5. Reconnect the cloud.

   Result: The worker's `/workspace/home/.codex/auth.json` has the same `last_refresh`
   as the login on this computer. Codex on the cloud still accepts a prompt.

### 6.6 C06 — Sign out

1. In Cloud settings, click **Sign out**.

   Result: The card shows **Sign in with ChatGPT**. The status is **Sign in to
   ChatGPT**. `~/.horizon/cloud/credentials/codex/auth.json` does not exist.

2. Create a second cloud with Codex.

   Result: Codex on the new cloud asks for a sign-in in its own terminal.

## 7. Pass criteria

- Each task gives the result that it shows.
- The login never appears in progress output, a log or a command line.
- `~/.codex/auth.json` does not change.

## 8. Cleanup

1. Delete the test clouds.

   Result: The clouds go from the cloud list, and the provider shows no server or pod
   for them.

2. Sign out in Cloud settings if the login is still there.

   Result: `~/.horizon/cloud/credentials/codex/auth.json` does not exist.

## 9. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). If not, put the results in the
pull request. Keep private evidence out of the repository.
