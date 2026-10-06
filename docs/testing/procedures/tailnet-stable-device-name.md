---
procedure: tailnet-stable-device-name
feature: Cloud tailnet device name after stop and resume
platforms: [linux]
cost: rents compute
destructive: yes
secrets: [tailnet auth key in Settings > Tailnets, provider API keys in Cloud settings]
owner: peters
---

# Tailnet stable device name test procedure

## 1. Purpose

This procedure proves that the device name of a cloud in its tailnet does not
change after **Stop worker** and **Resume worker**. A second cloud must reach a
service on the first cloud with the same name before and after the resume.

## 2. Applicability

- Candidate: a Horizon build and a worker image that include the fix for
  [issue #1310](https://github.com/peters/horizon/issues/1310).
- Platforms: Linux. Lanes: cloud A on Hetzner, cloud B on RunPod or Hetzner.
- This procedure does not test:
  - An image without `horizon-tailnet-contract=2`. Its device name changes at
    each resume. Unit tests cover the rename of such a cloud after a rebuild.
  - A device name that a tailnet administrator changed.
  - OAuth enrollment, Remote Hosts or inbound access to the PC.

## 3. Safety

> **CAUTION:** DELETE CLOUD A AND CLOUD B AT THE END OF THE RUN. Each running
> worker and each kept volume costs money until somebody deletes it.

> **CAUTION:** USE ONLY A DEDICATED, PRE-AUTHORIZED TEST AUTH KEY. Do not put
> the key in an issue, a log, a recording or a file in the repository.

> **CAUTION:** DELETE ONLY THE CLOUDS AND TAILNET DEVICES THAT THIS RUN RECORDED.
> Other workers, volumes and devices belong to other people.

> **CAUTION:** BIND THE TEST SERVER TO `127.0.0.1` ONLY. Do not open a port on
> a public address of the worker.

## 4. Equipment and preconditions

- A frozen candidate and its SHA-256.
- A worker image built from the candidate commit.
- A test tailnet with a saved auth key in **Settings > Tailnets**. The key is
  reusable, preauthorized and not ephemeral.
- Provider keys in **Cloud settings…** for each lane.
- A private evidence directory, `<evidence>`.
- A tailnet policy that lets cloud B reach cloud A on TCP `<port>`. If the
  policy blocks this port, T02 and T04 fail for a reason that is not this fix.
- In this procedure:
  - `<tailnet>` is the MagicDNS domain of the test tailnet, for example
    `tail1234.ts.net`.
  - `<port>` is a free port, for example `8765`.

The test server listens on `127.0.0.1` of cloud A only. The userspace
Tailscale daemon of cloud A sends the tailnet connection to that loopback
address. Thus, cloud B reaches the server, but the public network does not.

## 5. Setup

1. Start the candidate on an isolated desktop with a live view.

   Result: The Device panel shows the candidate and the frames advance.

> **CAUTION:** START ONLY THE TWO CLOUDS OF THIS PROCEDURE. Each worker costs
> money until you delete it. Each enrollment adds a device to the tailnet.

2. In **New cloud…**, select the test tailnet for cloud A on Hetzner.

   Result: The dialog shows the test tailnet.

3. Click **Start cloud**.

   Result: The card of cloud A shows Ready.

4. In **New cloud…**, select the same tailnet for cloud B.

   Result: The dialog shows the test tailnet.

5. Click **Start cloud**.

   Result: The card of cloud B shows Ready.

6. Record the names and IDs of cloud A and cloud B in `<evidence>`.

   Result: The record contains the two clouds that cleanup deletes.

## 6. Tasks

### 6.1 T01 — Examine the contract and the first name

1. Open a shell panel on cloud A.

   Result: The shell runs as `horizon-agent`.

2. Type this command.

   ```sh
   horizon-worker-tailnet --stable-name-contract
   ```

   Result: The command shows `horizon-tailnet-contract=2`.

3. Show the device entry of cloud A. The first entry in the file is always the
   worker itself.

   ```sh
   python3 -c 'import json; print(json.load(open("/run/horizon-tailnet-devices/devices.json"))["devices"][0])'
   ```

   Result: The name is `horizon-cloud-<ID>.<tailnet>.`. The entry is online.

4. Record the name without the last dot as `<name>`.

   Result: `<name>` has no random container host name, for example `d1021da2f9b5`.

5. Record the addresses of the entry.

   Result: The record contains the tailnet addresses of cloud A.

6. Type `hostname` and record the output.

   Result: The container host name is different from the first label of `<name>`.

### 6.2 T02 — Reach cloud A by name before the stop

1. On cloud A, make a new nonce.

   ```sh
   mkdir -p ~/tailnet-name-test && cd ~/tailnet-name-test
   python3 -c 'import secrets; print(secrets.token_hex(16))' > nonce
   ```

   Result: The file `nonce` contains 32 hexadecimal characters.

2. On cloud A, start the test server in the background.

   ```sh
   python3 -m http.server --bind 127.0.0.1 <port> > server.log 2>&1 &
   ```

   Result: `curl -s http://127.0.0.1:<port>/nonce` on cloud A shows the nonce.

3. Open a shell panel on cloud B.

   Result: The shell runs as `horizon-agent`.

4. On cloud B, get the nonce from cloud A by name.

   ```sh
   curl -sS --max-time 20 --socks5-hostname 127.0.0.1:1055 http://<name>:<port>/nonce; echo " exit=$?"
   ```

   Result: The output is the nonce of cloud A and `exit=0`.

### 6.3 T03 — Stop and resume cloud A

> **CAUTION:** STOP ONLY CLOUD A. Cloud B must continue to run for T04.

1. On the card of cloud A, click **Stop worker**.

   Result: The card asks **Stop this worker?**.

2. Click **Stop worker**.

   Result: The card shows that the worker stopped.

3. On cloud B, show the device entries.

   ```sh
   python3 -m json.tool /run/horizon-tailnet-devices/devices.json
   ```

   Result: The output contains one entry for `<name>`. No entry has the
   container host name from T01, step 6.

4. If the entry for `<name>` is online, do step 3 again after 1 minute.

   Result: The entry for `<name>` is offline.

5. On the card of cloud A, click **Resume worker**.

   Result: The card shows Ready.

6. Open a new shell panel on cloud A.

   Result: The shell runs as `horizon-agent`.

7. Type `hostname`.

   Result: The container host name is different from the value in T01, step 6.

8. Show the device entry of cloud A again, with the command in T01, step 3.

   Result: The name is `<name>` with a last dot. The addresses are the same as in T01.

### 6.4 T04 — Reach cloud A by the same name after the resume

1. On cloud A, make a new nonce.

   ```sh
   cd ~/tailnet-name-test
   python3 -c 'import secrets; print(secrets.token_hex(16))' > nonce
   ```

   Result: The file `nonce` contains a new value.

2. On cloud A, start the test server again.

   ```sh
   python3 -m http.server --bind 127.0.0.1 <port> > server.log 2>&1 &
   ```

   Result: `curl -s http://127.0.0.1:<port>/nonce` on cloud A shows the new nonce.

3. On cloud B, type the command from T02, step 4, with the same `<name>`.

   Result: The output is the new nonce and `exit=0`. Before the fix, curl failed
   with an exit code that is not 0, for example `exit=97`.

4. On cloud B, show the device entries again.

   Result: Only one entry is for cloud A. Its name is `<name>` and it is online.

### 6.5 T05 — Do a second stop and resume

1. Do T03 again.

   Result: The results are the same as in T03.

2. Do T04 again.

   Result: The results are the same as in T04.

## 7. Pass criteria

- T01 shows `horizon-tailnet-contract=2` and a name `horizon-cloud-<ID>`.
- The container host name changes after each resume.
- The device name and the addresses of cloud A do not change after each resume.
- In T02, T04 and T05, cloud B gets the current nonce from `<name>` with `exit=0`.
- Each resume completes without a new auth key entry.
- The evidence shows no auth key and no private tailnet data.

## 8. Cleanup

> **CAUTION:** DELETE ONLY THE CLOUDS AND DEVICES IN THE RECORD OF THIS RUN.
> Other workers, volumes and devices belong to other people.

1. On cloud A, stop the test server.

   ```sh
   pkill -f 'http.server --bind 127.0.0.1 <port>'
   ```

   Result: `curl -s http://127.0.0.1:<port>/nonce` on cloud A fails.

2. Delete cloud A and its storage.

   Result: The card of cloud A goes away. The provider shows no server or volume for it.

3. Delete cloud B and its storage.

   Result: The card of cloud B goes away. The provider shows no worker or volume for it.

> **CAUTION:** REMOVE ONLY THE DEVICES `<name>` AND THE DEVICE OF CLOUD B.
> Other devices give access to other people.

4. In the Tailscale admin console, remove the two devices that this run recorded.

   Result: The tailnet has no device from this run.

5. Close the Device panel of this run and stop the fixture.

   Result: The candidate stops. The fixture removes its private state.

## 9. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). If not, put the results in the
pull request. Keep private evidence out of the repository. Replace the tailnet
domain, the addresses and the cloud IDs with placeholders in public evidence.
