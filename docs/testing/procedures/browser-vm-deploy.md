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
tool call. It proves that the `Browser VM` section of the Settings window shows
that machine, stops it, resumes it, opens its Device panel and writes the settings
entry `browser_host`. A person signs in to a site on the display of the machine
one time, and a stop and a resume keep that sign-in. A delete from the Settings
window removes the machine and the state volume.

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

   Result: The answer holds `phase` `ready`, `ssh_target` `root@<address>`,
   `hourly_net`, `device_panel` and `person_step`. The `person_step` says that a
   person must sign in.

3. Examine the record.

   ```sh
   python3 -m json.tool ~/.horizon/cloud/browser-vm.json
   ```

   Result: The file holds the image, the server type, the location, the volume
   size, the SSH target, the server identity and the volume identity.

### 6.2 D2: the person drives the machine from the Settings window

1. Open the Settings window, and click the `Browser VM` tab.

   Result: The section shows the phase `Ready`, the machine size `ccx13`, the
   location `fsn1`, the SSH target `root@<address>` and the hourly cost. The State
   volume line shows `20 GB`.

2. Read the hourly cost line.

   Result: The line ends with `/h net`. The amount is the Hetzner price of the
   server type, without VAT.

3. Click `Stop`.

   Result: The section shows `Stopping`. Start, Stop, Resume and Delete are not
   available while the run continues. The phase becomes `Stopped` by itself, and
   the SSH target line says `none yet`.

4. Click `Resume`, and wait until the phase is `Ready`.

   Result: The section shows the new SSH target of the machine, and the hourly
   cost of its server type.

5. Click `Open Device panel`.

   Result: A native Device panel shows the desktop of the machine. The panel
   reaches the display through the SSH route that the section shows.

6. Close the Device panel of step 5.

   Result: The panel closes, and the machine keeps running.

7. Click `Use this host for new panels`, then click `Save`.

   Result: The settings entry `browser_host.ssh_target` holds `root@<address>`.
   Open the `YAML` tab to see it. The entry applies at the next start of Horizon.

### 6.3 D3: a person signs in one time

1. Call `device_panel` with the SSH route of the machine. The `SSH target` line of
   the `Browser VM` section shows its address, after the resume of step 6.2.4:

   ```json
   {"ssh": {"host": "<address>", "user": "root", "vnc": "127.0.0.1:5900"}}
   ```

   Result: A native Device panel shows the desktop of the machine.

2. In that desktop, open the browser and sign in to `SITE`.

   Result: The site shows the signed-in page. The frame counter of the panel
   advances while you click.

3. In the browser of that desktop, open the file browser and write a file in the
   profile folder:

   ```sh
   touch /state/signin-marker
   ```

   Result: The command creates the file on the state volume.

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

5. Read the marker from the new machine through its SSH route.

   ```sh
   ssh root@<new-address> ls /state/signin-marker
   ```

   Result: The command prints the path, so the state volume kept the file across
   the stop and the resume.

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

- D1 rents one machine, and its state volume is mounted at `/state`.
- D2 shows the phase, the machine and the net hourly cost in the Settings window,
  stops and resumes the machine there, opens the Device panel there and writes
  the settings entry.
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

## 9. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). If not, put the results in the
pull request. Keep private evidence out of the repository.
