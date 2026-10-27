---
procedure: browser-vm-deploy
feature: Browser VM rented from Horizon, with a state volume that keeps the sign-ins
platforms: [linux]
cost: rents compute
destructive: yes
secrets: Hetzner Cloud token file and the SSH identity named in the cloud settings
owner: peters
---

# Browser VM deploy test procedure

## 1. Purpose

This procedure proves that an agent rents a browser machine from Horizon with one
tool call. It proves that Horizon writes the ssh entry `horizon-browser-vm` and
trusts the host keys of the machine, so every route reaches the machine with no
copy of a key. It proves that the `Browser VM` section of the Settings window
shows that machine, stops it, resumes it, opens its Device panel and writes the
settings entry `browser_host`. A person signs in to a site on the display of the
machine one time, and a stop and a resume keep that sign-in. A delete from the
Settings window removes the machine and the state volume.

## 2. Applicability

- Candidate: a build of `horizon` from the branch of this procedure or later, with
  the cloud feature enabled, which is the default build.
- Platforms: Linux or macOS on the computer of the person. The rented machine runs
  Linux.
- This procedure does not test: the browser panel backend of another machine, the
  egress profile, and the image itself. `browser-vm-image` tests the image, and
  `browser-vm-egress` tests the egress.

## 3. Safety

> **CAUTION:** THIS PROCEDURE RENTS A DEDICATED HETZNER SERVER AND A STATE VOLUME.
> A `ccx13` server in `fsn1` costs 0.0689 EUR (net) for each hour, which is 42.99
> EUR for each month. A 20 GB state volume costs 1.15 EUR for each month, at
> 0.0572 EUR for each GB. Keep the run short, and complete step 8.

> **CAUTION:** DO NOT SIGN IN TO AN ACCOUNT THAT YOU CANNOT SIGN OUT OF. Use a
> test account, or a site that you can leave. Every cookie of the run stays in the
> state volume until step 8 deletes it.

> **CAUTION:** DELETE ONLY THE BROWSER VM THAT THIS PROCEDURE CREATED. The record
> in `browser-vm.json` names it, and the machine carries the operation label
> `browser-vm`.

## 4. Equipment and preconditions

- A computer that runs Horizon, with an agent panel in a workspace and an open
  Settings window.
- A cloud settings file at `~/.horizon/cloud/settings.json` with a `hetzner`
  section: a token file, at least one location such as `fsn1`, and an SSH identity
  pair whose `.pub` file is beside it. The `hetzner` `server_types` list is not used
  by this procedure.
- An image reference with a digest for the image of `packaging/browser-vm`, on a
  registry that the rented machine can pull. Name it `IMAGE` in this procedure. A
  private image needs the `registry_pull` binding of the cloud settings.
- A site with a sign-in page for step 6.3. Name it `SITE` in this procedure.
- An ssh configuration at `~/.ssh/config` that holds no `Host horizon-browser-vm`
  block of its own. If it holds one, Horizon writes nothing, because that name
  belongs to the person, and the answer holds an `ssh_note` that says so.

## 5. Setup

1. Build the image from the root of the repository, and push it to your registry.

   ```sh
   docker build -f packaging/browser-vm/Dockerfile -t "$MY_REGISTRY/browser-vm:1780" .
   docker push "$MY_REGISTRY/browser-vm:1780"
   ```

   Result: The registry reports the digest of the image. Copy it into `IMAGE` as
   `$MY_REGISTRY/browser-vm@sha256:<digest>`.

2. Start Horizon, and open one agent panel in a workspace.

   Result: The agent reports that it can call the browser tools.

## 6. Tasks

### 6.1 D1: an agent rents the machine

1. Ask the agent to call `browser_vm` with these arguments:

   ```json
   {"operation": "deploy", "image": "<IMAGE>", "server_type": "ccx13", "location": "fsn1"}
   ```

   Result: The tool returns `phase` `deploying` at once. It holds no `ssh_target`
   yet.

2. Ask the agent to call `browser_vm` with `{"operation": "status"}` until the
   phase is `ready`.

   Result: The answer holds `phase` `ready`, `ssh_target` `horizon-browser-vm`,
   `hourly_net`, `device_panel` and `person_step`. The `person_step` says that a
   person must sign in through the entry that Horizon wrote.

3. Examine the record.

   ```sh
   python3 -m json.tool ~/.horizon/cloud/browser-vm.json
   ```

   Result: The file holds the image, the server type, the location, the volume
   size, `ssh_target` `horizon-browser-vm`, the server identity and the volume
   identity. The file holds no `ssh_note`, because the entry is complete.

4. Examine the ssh entry that Horizon wrote.

   ```sh
   head -8 ~/.ssh/config
   ls -l ~/.ssh/config ~/.horizon/cloud/browser-vm-known-hosts
   ```

   Result: The first lines of `~/.ssh/config` are the block of this procedure,
   below. The folder `~/.ssh` is `drwx------`, and both files are `-rw-------`.
   The managed file holds one line for each host key, and each line starts with
   `horizon-browser-vm`.

   ```text
   # >>> horizon browser vm; written by Horizon, do not edit
   Host horizon-browser-vm
     HostName <address>
     User root
     IdentityFile <absolute ssh_identity_file of the cloud settings>
     IdentitiesOnly yes
     UserKnownHostsFile <absolute path of ~/.horizon/cloud/browser-vm-known-hosts>
   # <<< horizon browser vm
   ```

5. Ask the agent to run a command on the machine through the entry.

   ```sh
   ssh horizon-browser-vm true
   ```

   Result: The command exits with the code 0. It asked for no key and no host
   key, because the entry names both.

### 6.2 D2: the person drives the machine from the Settings window

1. Open the Settings window with `Ctrl+Shift+Comma`, and click the `Browser VM`
   tab.

   Result: The section shows the phase `Ready`, the machine size `ccx13`, the
   location, the SSH target `horizon-browser-vm` and the hourly cost. The `State
   volume` line shows `20 GB`.

2. Read the hourly cost line.

   Result: The line ends with `/h net`. The amount is the Hetzner price of the
   server type, without VAT. The note under it says that the state volume, the
   primary IPv4 address and traffic bill separately.

3. Click `Use this host for new panels`, then click `Save`.

   Result: The button becomes unavailable, the card says that new browser panels
   already use this machine, and the `YAML` tab shows `browser_host:
   ssh_target: horizon-browser-vm`. The entry applies at the next start of
   Horizon. The entry names the machine, so it also applies after a resume with
   a new address.

4. Click `Open Device panel`.

   Result: A native Device panel named `Browser VM` shows `127.0.0.1:5900 via
   root@horizon-browser-vm`, the connection state `Connected` and the VNC name of
   the machine. The panel asks for no key and for no host key, because the entry
   carries both. The desktop of the machine is a plain window manager until a
   page opens there.

5. Click `Stop`.

   Result: The section shows `Stopping`, and Start, Stop, Resume and Delete are
   not available while the run continues. The phase becomes `Stopped` by itself.
   The SSH target line says `none yet`, and the state volume line still shows
   `20 GB`.

6. Examine the ssh entry after the stop.

   ```sh
   grep -c 'horizon browser vm' ~/.ssh/config
   ls ~/.horizon/cloud/browser-vm-known-hosts
   ```

   Result: The first command prints `0`, and the second reports that the file
   does not exist. The other hosts and comments of `~/.ssh/config` are as they
   were before the deploy.

7. Click `Resume`, and wait until the phase is `Ready`.

   Result: The section shows the SSH target `horizon-browser-vm` again, and the
   hourly cost of its server type. The entry names the new address of the
   machine. The state volume is the same volume, so the sites that a person
   signed in to are still signed in.

   > **CAUTION:** Resume rents a machine. Stop it or delete it when the check is
   > complete.

8. Click `Delete`, then `Delete the machine and its volume`.

   Result: The section shows `Deleting` and then `Deleted`. The warning before
   the second press says that every saved sign-in is lost. The account then
   shows no server and no volume.

   > **CAUTION:** Delete removes the server and the state volume of the machine.
   > Every saved sign-in is lost.

### 6.3 D3: a person signs in one time

1. Ask an agent to call `device_panel` with the ssh route of the answer.

   ```json
   {"ssh": {"host": "horizon-browser-vm", "user": "root", "vnc": "127.0.0.1:5900"}}
   ```

   Result: The panel shows the desktop of the machine and the connection state
   `Connected`. The tool needs no key, because the entry carries the key that
   the deploy installed and the host keys of the machine. `Open Device panel` in
   the section opens the same route.

2. Ask an agent to open `SITE` on the browser host of the machine, so that its
   browser window appears on that desktop. The desktop has a window manager and
   Chromium, and no window until a page opens.

   Result: A browser window appears on the desktop of the machine, and the frame
   counter of the panel advances.

3. Sign in to `SITE` in that window.

   Result: The site shows the signed-in page.

4. Write a file in the state volume:

   ```sh
   touch /state/signin-marker
   ```

   Result: The file exists in `/state` of the machine.

### 6.4 D4: a stop and a resume keep the sign-in

1. Ask the agent to call `browser_vm` with `{"operation": "stop"}`.

   Result: The tool returns `phase` `stopping`.

2. Ask the agent to call `browser_vm` with `{"operation": "status"}` until the
   phase is `stopped`.

   Result: The answer holds `phase` `stopped` and no `ssh_target`.

3. Ask the agent to call `browser_vm` with `{"operation": "resume"}`, then poll
   `status` until the phase is `ready`.

   Result: The answer holds a new `ssh_target` for a new address.

4. Open the `device_panel` route of the new answer, and open `SITE` in the browser
   of the desktop.

   Result: The site shows the signed-in page without a new sign-in.

5. Read the marker from the new machine through its SSH entry.

   ```sh
   ssh horizon-browser-vm ls /state/signin-marker
   ```

   Result: The command prints the path, so the state volume kept the file across
   the stop and the resume, and the entry names the new address.

### 6.5 D5: a delete from the Settings window removes the machine and the volume

> **CAUTION:** STEP 6.5.3 DELETES THE SERVER AND THE STATE VOLUME OF THIS
> PROCEDURE. Every saved sign-in is lost. Delete must leave no server and no
> volume in the Hetzner project.

1. Open the `Browser VM` tab, and click `Delete`.

   Result: The section asks again, in the section and not in a modal window. The
   machine still runs.

2. Click `Keep the machine`.

   Result: The question closes, and the phase stays `Ready`.

3. Click `Delete`, then click `Delete the machine and its volume`.

   Result: The phase is `Deleting`, and then `Deleted`. The SSH target line says
   `none yet`, and Start is available again.

4. Examine the Hetzner project with the token file of the cloud settings.

   ```sh
   curl --silent --header "Authorization: Bearer $(cat <token file>)" \
     https://api.hetzner.cloud/v1/servers | python3 -c 'import json,sys; print(json.load(sys.stdin)["servers"])'
   curl --silent --header "Authorization: Bearer $(cat <token file>)" \
     https://api.hetzner.cloud/v1/volumes | python3 -c 'import json,sys; print(json.load(sys.stdin)["volumes"])'
   ```

   Result: Both commands print `[]`. No server and no volume of the run remains.

### 6.6 D6: a mutable tag is refused

> **CAUTION:** STEP 6.6.1 PRESSES START, WHICH RENTS A DEDICATED SERVER AND A
> STATE VOLUME. The request carries a tag and not a digest, so Horizon must
> refuse it and rent nothing. If the phase changes to `Starting`, click `Delete`
> at once and do step 6.5.3.

1. In the `Browser VM` tab, type `example/browser-vm:1780` into `Image`, and click
   `Start`.

   Result: The section shows the refusal that names the digest. The phase stays
   `Deleted`, and nothing is rented.

2. Ask the agent to call `browser_vm` with `{"operation": "deploy", "image":
   "example/browser-vm:1780", "server_type": "ccx13"}`.

   Result: The tool refuses with the same reason, and the Hetzner project holds
   no new server.

## 7. Pass criteria

- D1 rents one machine, and its state volume is mounted at `/state`. Horizon
  writes the ssh entry `horizon-browser-vm` with the key of the deploy and the
  host keys of the machine, and `ssh horizon-browser-vm` reaches the machine
  without a copy of a key and without a host-key step.
- D2 shows the phase, the machine and the net hourly cost in the Settings window,
  stops and resumes the machine there, opens the Device panel there and writes
  the settings entry. A stop removes the ssh entry and a resume writes it again.
- D3 signs in one time, and the Device panel shows the desktop.
- D4 keeps the sign-in and the file across a stop and a resume.
- D5 deletes the machine from the Settings window, and leaves no server and no
  volume in the Hetzner project.
- D6 refuses a mutable tag in the Settings window and in the tool, before any
  request.

## 8. Cleanup

1. If any step of task D5 did not reach `deleted`, click `Delete` and then
   `Delete the machine and its volume` again, and wait until the phase is
   `Deleted`.

   Result: The phase is `deleted`.

2. Examine the Hetzner project as step 6.5.4 does.

   Result: Both commands print `[]`.

3. Close the Device panel of the machine.

   Result: The panel closes. The machine is already gone.

4. Examine the ssh entry after the delete.

   ```sh
   grep -c 'horizon browser vm' ~/.ssh/config
   ls ~/.horizon/cloud/browser-vm-known-hosts
   ```

   Result: The first command prints `0`, and the second reports that the file
   does not exist. The other hosts and comments of `~/.ssh/config` are as they
   were before the deploy.

## 9. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). If not, put the results in the
pull request. Keep private evidence out of the repository.
