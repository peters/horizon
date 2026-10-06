---
procedure: cloud-panels-t-tailnets
feature: Cloud panels smoke test, area T (tailnets)
platforms: [linux]
cost: rents compute
destructive: yes
secrets: [test tailnet auth key in the secret store of the test account]
owner: peters
---

# Cloud panels test procedure, area T: tailnets

## 1. Purpose

This area makes sure that the candidate keeps tailnet auth keys safe and that a
cloud can join a test tailnet. It also makes sure that clouds and the PC can
send TCP traffic to each other over the tailnet.

## 2. Applicability

- Candidate: the frozen candidate of [area S](s-test-fixture.md).
- Platforms: Linux. Providers: Hetzner for `smoke-a` and `smoke-b`, RunPod for
  `smoke-r`.
- Do T12 and T13 after G03 in [area G](g-companions.md). These tasks need a
  checked companion.
- This area does not test: OAuth enrollment, the Tailscale API, Remote Hosts,
  macOS and Windows credential stores, or a kernel VPN interface. See
  [cloud tailnets](../../../cloud-workspaces.md#tailnets-auth-key-mvp).

## 3. Safety

> **CAUTION:** USE ONLY THE AUTH KEY OF THE TEST TAILNET. A key of another
> tailnet gives the worker access to the devices of other people.

> **CAUTION:** DO NOT CHANGE THE ACL POLICY, THE FIREWALL OR THE TAILSCALE SERVE
> SETTINGS OF THE PC OR THE TAILNET. If the policy refuses a connection, record
> the test as blocked.

> **CAUTION:** DO NOT PUT A TAILNET ADDRESS OR A DEVICE NAME IN THE REPOSITORY. Keep
> them in the private evidence.

## 4. Equipment and preconditions

- The equipment in the [main procedure](../cloud-panels.md#4-equipment-and-preconditions).
- The fixture with a Secret Service from S05.
- The test tailnet. Its auth key is reusable, pre-authorized and not ephemeral.
- The PC is a device on the test tailnet. Tests T06 and T07 need this.
- A worker image that reports `horizon-tailnet-contract=1`. Use
  [`check-markers.py`](../../../../examples/cloud-worker/README.md#helpers-from-the-published-artifact)
  in B05 to examine the image.
- A worker shell in each cloud of this area. Until the fix for
  [issue #1301](https://github.com/peters/horizon/issues/1301) is in the
  candidate, examine each typed command on the screen before you press Enter.

## 5. Setup

1. Write the list of the devices on the test tailnet to the private evidence.

   Result: You have the tailnet baseline for X03.

2. Make a directory for the random test values.

   ```sh
   mkdir -p <evidence>/tailnet && chmod 700 <evidence>/tailnet
   ```

   Result: The directory exists and only the operator can read it.

## 6. Tasks

### 6.1 T01 — Add a tailnet and examine the key check

1. Click **Settings** in the toolbar.

   Result: The Settings window opens.

2. Click the **Tailnets** tab.

   Result: The tab shows **Tailnets** and **Private networks for your clouds**.

3. Click **Add tailnet**.

   Result: The form shows the **Name** and **Auth key** fields.

4. Type `Smoke test tailnet` in the **Name** field.

   Result: The field shows the name.

5. Type the synthetic value `tskey-auth-short` in the **Auth key** field.

   Result: **Save tailnet** stays disabled.

6. Clear the **Auth key** field.

   Result: The field is empty.

   > **CAUTION:** THE OPERATOR MUST ENTER THE REAL AUTH KEY. A device `type` action
   > can lose characters, and a recording can show the key.

7. Let the operator paste the auth key of the test tailnet in the **Auth key** field.

   Result: The field shows the key masked.

   > **CAUTION:** SAVE ONLY THE KEY OF THE TEST TAILNET. This step stores a secret
   > that can join devices to the tailnet.

8. Click **Save tailnet**.

   Result: The list shows `Smoke test tailnet` with **Auth key protected in OS keychain**.

9. Search the tailnet catalog file for a key.

   ```sh
   grep -c tskey <data-home>/.horizon/cloud/tailnets.json
   ```

   Result: The output is `0`. The file contains the name and an ID only.

### 6.2 T02 — Replace, remove and refresh a tailnet

1. Add a second tailnet with the name `Smoke spare` and the same auth key, as in T01.

   Result: The list shows two tailnets.

2. Record the ID of each tailnet in `tailnets.json`.

   Result: The evidence contains two IDs.

3. Click **Replace key** on `Smoke spare`.

   Result: The form shows **Replace auth key** and an empty **Auth key** field.

   > **CAUTION:** THE OPERATOR MUST ENTER THE REAL AUTH KEY. A device `type` action
   > can lose characters.

4. Let the operator paste the auth key in the **Auth key** field.

   Result: The field shows the key masked.

   > **CAUTION:** SAVE ONLY THE KEY OF THE TEST TAILNET. This step stores a secret.

5. Click **Save tailnet**.

   Result: The list shows `Smoke spare`. Its ID in `tailnets.json` did not change.

6. Search the settings files for a key.

   ```sh
   grep -rlc tskey <data-home>/.horizon/cloud
   ```

   Result: The output is empty. The key is only in the Secret Service.

   > **CAUTION:** REMOVE ONLY `Smoke spare`. If you remove `Smoke test tailnet`, the
   > other tasks of this area cannot start a cloud on the tailnet.

7. Click **Remove** on `Smoke spare`.

   Result: The list shows only `Smoke test tailnet`.

8. Click **Refresh**.

   Result: The list shows only `Smoke test tailnet`.

9. Close the Settings window.

   Result: The window closes.

### 6.3 T03 — Provision a cloud on the tailnet

1. Open **Cloud › New cloud…** in the workspace of the test.

   Result: The New cloud dialog opens.

2. Type `smoke-a` as the title.

   Result: The dialog shows the title.

3. Select the Hetzner CPU profile.

   Result: The summary shows a Hetzner worker.

4. Click `Smoke test tailnet` in the **Tailnet** chooser.

   Result: The chooser shows **None** and `Smoke test tailnet`. `Smoke test tailnet`
   is selected.

   > **CAUTION:** THIS STEP RENTS COMPUTE AND JOINS A WORKER TO THE TAILNET. Record
   > the server and the volume in the resource ledger.

5. Click **Start cloud**.

   Result: The cloud card shows the deployment stages.

6. Wait until the card shows **Ready**.

   Result: The card shows **Tailnet**, `Smoke test tailnet` and **Selected at provisioning**.

7. Write the server ID and the volume ID in the resource ledger.

   Result: The resource ledger contains the resources of `smoke-a`.

8. If D01 started `smoke-a` on the tailnet, do only step 6 and step 7.

   Result: The card of the current `smoke-a` shows the tailnet.

### 6.4 T04 — Examine the userspace networking of the worker

1. In the worker shell of `smoke-a`, show the TCP listeners.

   ```sh
   ss -ltn
   ```

   Result: The output shows `127.0.0.1:1055` and `127.0.0.1:1056`.

2. Show the proxy variables of the shell.

   ```sh
   env | grep -i _proxy
   ```

   Result: The HTTP and HTTPS variables point to `http://127.0.0.1:1056`.

3. Examine the network interfaces of the worker.

   ```sh
   ip -brief link
   ```

   Result: The output shows no `tailscale0` interface. The worker uses userspace
   networking.

4. Try to read the private tailnet state.

   ```sh
   ls /workspace/.horizon-tailnet
   ```

   Result: The shell refuses access. The agent account cannot read the node state.

### 6.5 T05 — Examine the device list

1. In the worker shell of `smoke-a`, show the device list.

   ```sh
   jq . /run/horizon-tailnet-devices/devices.json
   ```

   Result: The file contains a `devices` list.

2. Examine the fields of each device.

   ```sh
   jq -r '.devices[] | keys | join(",")' /run/horizon-tailnet-devices/devices.json | sort -u
   ```

   Result: The output is `addresses,name,online` only.

3. Find the PC in the list.

   Result: The PC shows with `online: true`. The list contains only the devices
   that the ACL policy lets the worker see.

4. Record the number of devices in the private evidence.

   Result: The evidence contains the count, not the names.

### 6.6 T06 — Send a random value from the cloud to the PC

1. On the PC, make a random value.

   ```sh
   python3 -c 'import secrets; print(secrets.token_hex(16))' > <evidence>/tailnet/nonce-pc
   ```

   Result: The file contains 32 hexadecimal characters.

2. On the PC, find the tailnet address of the PC.

   ```sh
   tailscale ip -4
   ```

   Result: You have the IPv4 tailnet address of the PC.

   > **CAUTION:** BIND THE TEST SERVER ONLY TO THE TAILNET ADDRESS AND STOP IT AFTER
   > THE TEST. If you bind it to all addresses, devices on the local network can read it.

3. On the PC, start a test HTTP server on port 18080.

   ```sh
   python3 -m http.server 18080 --bind <pc-tailnet-address> --directory <evidence>/tailnet
   ```

   Result: The server shows that it listens on port 18080.

4. In the worker shell of `smoke-a`, read the value through the SOCKS5 proxy.

   ```sh
   curl -sS --max-time 20 --socks5-hostname 127.0.0.1:1055 http://<pc-tailnet-address>:18080/nonce-pc
   ```

   Result: The output is the same value as `nonce-pc`.

5. If `curl` stops with a timeout or a refusal, record the test as blocked.

   Result: The report gives the reason. You did not change the ACL policy.

6. On the PC, stop the test server with Ctrl-C.

   Result: The server stops.

### 6.7 T07 — Connect from the PC to the cloud

1. In the worker shell of `smoke-a`, find the tailnet address of the worker.

   ```sh
   jq -r '.devices[0].addresses[0]' /run/horizon-tailnet-devices/devices.json
   ```

   Result: You have the IPv4 tailnet address of `smoke-a`. The first device is the worker.

2. In the worker shell, write a random value to a test directory.

   ```sh
   mkdir -p ~/tailnet-test && python3 -c 'import secrets; print(secrets.token_hex(16))' > ~/tailnet-test/nonce
   ```

   Result: The file contains 32 hexadecimal characters.

3. In the worker shell, start a test HTTP server on the loopback address.

   ```sh
   python3 -m http.server 18081 --bind 127.0.0.1 --directory ~/tailnet-test
   ```

   Result: The server shows that it listens on port 18081.

4. On the PC, read the value from the tailnet address of the worker.

   ```sh
   curl -sS --max-time 20 http://<smoke-a-tailnet-address>:18081/nonce
   ```

   Result: The output is the same value as the file on the worker.

5. If `curl` stops with a timeout or a refusal, record the test as blocked.

   Result: The report gives the reason.

6. In the worker shell, stop the test server with Ctrl-C.

   Result: The server stops.

### 6.8 T08 — Connect from cloud A to cloud B on Hetzner

1. Start `smoke-b` on the test tailnet with the steps 1 to 7 of T03.

   Result: The card of `smoke-b` shows **Ready** and **Selected at provisioning**.
   The resource ledger contains its server and volume.

2. In the worker shell of `smoke-b`, find the tailnet address of the worker.

   ```sh
   jq -r '.devices[0].addresses[0]' /run/horizon-tailnet-devices/devices.json
   ```

   Result: You have the IPv4 tailnet address of `smoke-b`.

3. In the worker shell of `smoke-b`, start the test server as in T07 steps 2 and 3.

   Result: The server on `smoke-b` listens on `127.0.0.1:18081`.

4. In the worker shell of `smoke-a`, read the value from `smoke-b`.

   ```sh
   curl -sS --max-time 20 --socks5-hostname 127.0.0.1:1055 http://<smoke-b-tailnet-address>:18081/nonce
   ```

   Result: The output is the same value as the file on `smoke-b`.

5. In the worker shell of `smoke-b`, stop the test server with Ctrl-C.

   Result: The server stops.

### 6.9 T09 — Connect from Hetzner to RunPod over the tailnet

1. Make sure that `smoke-r` shows `Smoke test tailnet` and **Selected at provisioning**.

   Result: The RunPod cloud is on the test tailnet. D02 started it.

2. In the worker shell of `smoke-r`, start the test server as in T07 steps 2 and 3.

   Result: The server on `smoke-r` listens on `127.0.0.1:18081`.

3. In the worker shell of `smoke-r`, find the tailnet address of the worker.

   ```sh
   jq -r '.devices[0].addresses[0]' /run/horizon-tailnet-devices/devices.json
   ```

   Result: You have the IPv4 tailnet address of `smoke-r`.

4. In the worker shell of `smoke-a`, read the value from `smoke-r`.

   ```sh
   curl -sS --max-time 20 --socks5-hostname 127.0.0.1:1055 http://<smoke-r-tailnet-address>:18081/nonce
   ```

   Result: The output is the same value as the file on `smoke-r`.

5. If RunPod refuses the tailnet or the connection fails, record the test as blocked.

   Result: The report gives the reason. RunPod is not yet qualified for tailnets.

6. In the worker shell of `smoke-r`, stop the test server with Ctrl-C.

   Result: The server stops.

### 6.10 T10 — Keep the node identity after Stop and Resume

1. In the worker shell of `smoke-a`, record the name and the addresses of the worker.

   ```sh
   jq -c '.devices[0] | {name, addresses}' /run/horizon-tailnet-devices/devices.json > ~/node-before.json
   ```

   Result: The file contains the name and the addresses of the node.

2. Copy the content of `~/node-before.json` to the private evidence.

   Result: The evidence contains the node identity before the stop.

   > **CAUTION:** STOP ONLY `smoke-a`. On Hetzner, the stop deletes the server and
   > ends all processes on the worker.

3. Click **Stop worker…** on the card of `smoke-a`.

   Result: The card asks for a second click on **Stop worker**.

4. In the card, click **Stop worker**.

   Result: The card shows **Stopped**. On Hetzner, Horizon deletes the server and
   keeps the volume.

5. Record the deletion of the server in the resource ledger.

   Result: The ledger shows the server of `smoke-a` as deleted.

   > **CAUTION:** THIS STEP RENTS COMPUTE. On Hetzner, the next reconnect makes a new
   > server. Record it in the resource ledger.

6. Click **Resume worker**.

   Result: The card offers **Reconnect cloud**, or it starts the reconnect.

7. If the card shows **Reconnect cloud**, click **Reconnect cloud**.

   Result: The card shows **Ready**.

8. Write the new server ID in the resource ledger.

   Result: The ledger contains the new server of `smoke-a`.

9. In a new worker shell of `smoke-a`, show the name and the addresses of the worker.

   ```sh
   jq -c '.devices[0] | {name, addresses}' /run/horizon-tailnet-devices/devices.json
   ```

   Result: The name and the addresses are the same as in step 2.

10. Examine the device list of the test tailnet.

    Result: The tailnet shows one node for `smoke-a`, not two.

### 6.11 T11 — Restart the tailnet daemon after a kill

This task needs a root shell. Use the SSH route of E09 in
[area E](e-panels.md).

1. In the root shell of `smoke-a`, count the online peers.

   ```sh
   jq '[.devices[] | select(.online)] | length' /run/horizon-tailnet-devices/devices.json
   ```

   Result: You have the online count before the kill. Record it in the evidence.

2. In the root shell of `smoke-a`, show the process ID of `tailscaled`.

   ```sh
   pgrep -x tailscaled
   ```

   Result: The output shows one process ID.

3. Stop the daemon with a kill signal.

   ```sh
   pkill -KILL -x tailscaled
   ```

   Result: The command stops without an error.

4. Wait 30 seconds.

   Result: The supervisor of the worker has time to start the daemon again.

5. Show the process ID of `tailscaled` again.

   ```sh
   pgrep -x tailscaled
   ```

   Result: The output shows one new process ID.

6. In the worker shell of `smoke-a`, show the online peers.

   ```sh
   jq '[.devices[] | select(.online)] | length' /run/horizon-tailnet-devices/devices.json
   ```

   Result: The count is the same as in step 1. The PC and `smoke-b` are online.

7. Do T08 step 4 again with a new test server on `smoke-b`.

   Result: The worker reaches `smoke-b` with the same node identity.

### 6.12 T12 — Refuse a network change after provisioning

1. Open the card of `smoke-a`.

   Result: The card shows `Smoke test tailnet` and **Selected at provisioning**.
   There is no tailnet chooser on the card.

2. In a local agent panel in the workspace of `smoke-a`, call `cloud_companions`.

   Result: The answer lists `smoke-a`, its companion `lib` and the saved tailnet
   with its ID.

3. Record the tailnet choice of `smoke-lib`.

   Result: `smoke-lib` started with **None**.

   > **CAUTION:** MAKE SURE THAT `smoke-lib` RUNS BEFORE THIS STEP. If it is stopped,
   > an accepted request starts it again and rents compute.

4. Call `cloud_companion_ensure_ready` for `smoke-a` and the alias `lib` with the tailnet ID.

   ```json
   {"cloud":"<smoke-a cloud ID>","alias":"lib","tailnet":"<tailnet ID>"}
   ```

   Result: The tool refuses the request. The answer says that the network of a
   provisioned cloud cannot change.

5. Examine the card of `smoke-lib`.

   Result: The card still shows **None** as the tailnet. No operation started.

### 6.13 T13 — Accept a tailnet ID and refuse an auth key in MCP

1. In the local agent panel, call `cloud_companion_ensure_ready` with the current choice of `smoke-lib`.

   ```json
   {"cloud":"<smoke-a cloud ID>","alias":"lib","tailnet":"none"}
   ```

   Result: The tool accepts the request and gives an `operation_id` and a phase.

2. Call `cloud_companion_operation` with the same `cloud`, `alias` and `operation_id`.

   Result: The answer shows `done: true` and a Ready phase.

3. Call `cloud_companion_ensure_ready` with a synthetic auth key as the tailnet.

   ```json
   {"cloud":"<smoke-a cloud ID>","alias":"lib","tailnet":"tskey-auth-SYNTHETICVALUE"}
   ```

   Result: The tool refuses the request. The answer does not contain `SYNTHETICVALUE`.

4. Search the agent log and the candidate log for the synthetic value.

   Result: No log contains `SYNTHETICVALUE`.

### 6.14 T14 — Connect cloud A and cloud B in both directions

1. In the worker shell of `smoke-b`, start the test server as in T07 steps 2 and 3.

   Result: The server on `smoke-b` listens on `127.0.0.1:18081`.

2. In the worker shell of `smoke-a`, read the value from `smoke-b` through the SOCKS5 proxy.

   ```sh
   curl -sS --max-time 20 --socks5-hostname 127.0.0.1:1055 http://<smoke-b-tailnet-address>:18081/nonce
   ```

   Result: The output is the same value as the file on `smoke-b`.

3. In the worker shell of `smoke-a`, read the value through the HTTP proxy.

   ```sh
   curl -sS --max-time 20 --proxy http://127.0.0.1:1056 http://<smoke-b-tailnet-address>:18081/nonce
   ```

   Result: The output is the same value.

4. In the worker shell of `smoke-b`, stop the test server with Ctrl-C.

   Result: The server stops.

5. In the worker shell of `smoke-a`, start the test server as in T07 steps 2 and 3.

   Result: The server on `smoke-a` listens on `127.0.0.1:18081`.

6. In the worker shell of `smoke-b`, read the value from `smoke-a` through the SOCKS5 proxy.

   ```sh
   curl -sS --max-time 20 --socks5-hostname 127.0.0.1:1055 http://<smoke-a-tailnet-address>:18081/nonce
   ```

   Result: The output is the same value as the file on `smoke-a`.

7. In the worker shell of `smoke-b`, read the value through the HTTP proxy.

   ```sh
   curl -sS --max-time 20 --proxy http://127.0.0.1:1056 http://<smoke-a-tailnet-address>:18081/nonce
   ```

   Result: The output is the same value.

8. In the worker shell of `smoke-a`, stop the test server with Ctrl-C.

   Result: The server stops.

## 7. Pass criteria

- T01 and T02 keep the auth key only in the Secret Service. No settings file
  contains `tskey`.
- **Save tailnet** stays disabled for a short key.
- The card of each cloud on the tailnet shows **Selected at provisioning**.
- The worker listens on `127.0.0.1:1055` and `127.0.0.1:1056`, and the device
  list contains only names, addresses and online state.
- Each round trip in T06 to T09 and T14 returns the same random value, or the
  report records a policy block.
- The node identity is the same after Stop and Resume, and after a kill of `tailscaled`.
- The candidate refuses a network change and an auth key in MCP, and no answer
  or log shows the synthetic key.

## 8. Cleanup

1. Make sure that no test HTTP server runs on the PC or on a worker.

   ```sh
   pgrep -af 'http.server 1808'
   ```

   Result: The output is empty on each machine.

2. Delete the random value files on the workers.

   ```sh
   rm -rf ~/tailnet-test ~/node-before.json
   ```

   Result: The files are deleted.

Keep `Smoke test tailnet` and the clouds. Area G, area N and area L use them.
X01 deletes the clouds and X03 removes the tailnet key.

## 9. Record of results

Write the results in the report of the run. Use the
[report template](../../reports/TEMPLATE.md). Keep tailnet addresses, device
names and provider IDs out of the repository.
