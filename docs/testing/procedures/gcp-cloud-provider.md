---
procedure: gcp-cloud-provider
feature: Google Cloud (Compute Engine) as a cloud provider in New cloud, the CLI plan runner and the agent tools
platforms: [linux]
cost: rents compute   # tasks GCP-L1, GCP-L4, GCP-L5 and GCP-L6 rent one cloud each
destructive: yes      # the delete tasks and the cleanup remove the instance, the workspace disk and the static address
secrets: [Google Cloud service-account JSON key file of the cloud settings, SSH identity file of the cloud settings]
owner: horizon-maintainers
---

# Google Cloud cloud provider test procedure

## 1. Purpose

This procedure proves that a person can pick Google Cloud in **New cloud**, rent
one cloud there, reach it over SSH, stop it, resume it, rebuild it and delete it.
It proves the same flows through the CLI plan runner and the agent tools. It
proves that the credential is a service-account JSON key that Horizon uses
itself, with no `gcloud` command on this computer. It proves that Horizon mints
the access token, that a cloud which runs more than one hour continues to work,
and that a delete leaves no resource of the cloud through a proof that needs no
token.

## 2. Applicability

- Candidate: a build with the Google Cloud provider of epic #2003. The build
  has the `gcp` settings section, the `gcp.json` journal, and one `Description`
  entry for Google Cloud.
- Platforms: Linux with an isolated X display, from the local smoke fixture.
  Provider: Google Cloud, beside RunPod, Hetzner, the Docker host and Azure.
- Tasks GCP-01 to GCP-06 cost nothing. They use headless tests, a loopback
  endpoint and a refused request. They need no Google Cloud project. Task GCP-07
  presses **Start cloud** one time and expects the refusal of the provider.
- Tasks GCP-L1, GCP-L4, GCP-L5 and GCP-L6 rent one Compute Engine instance, one
  workspace persistent disk and one static address for each cloud that they
  make. Task GCP-L6 keeps its cloud for more than one hour.
- This procedure does not test:
  - The resources of another run in the same project. Use a test project.
  - The tailnet, the browser panels and the agent sessions of the cloud. Other
    procedures test them.
  - The Cloud console. This procedure reads it only in the optional steps.
  - A GPU profile. Google Cloud offers no GPU worker in this candidate.
  - The total cost since creation on the cloud card. `billing::history_supported`
    stays RunPod only, so a Google Cloud card shows no total.
- The epic holds these points as open. This procedure cannot fix them. Where a
  step meets one, the step reads the value from the candidate:
  - The names of the fields of the `gcp` section.
  - The zone. `Placement` holds a region and not a zone, so this procedure calls
    the value that a row shows its location.
  - The price source. A dated table that a person maintains prices the machine
    types, so a price change needs a new table.

## 3. Safety

> **CAUTION:** EVERY LIVE TASK RENTS A COMPUTE ENGINE INSTANCE, A PERSISTENT
> DISK AND A STATIC ADDRESS. They bill from the start until the delete
> completes. A cloud that you leave keeps billing. Delete each cloud in the same
> session, and keep task GCP-L6 in one session of more than one hour.

> **CAUTION:** THE DELETE TASKS REMOVE THE INSTANCE, THE WORKSPACE DISK AND THE
> STATIC ADDRESS. The files on the workspace disk are lost. Use a test
> repository only, and never a cloud that holds work that you must keep.

> **CAUTION:** DO NOT PUT THE SERVICE-ACCOUNT KEY, AN ACCESS TOKEN OR THE SSH
> PRIVATE KEY IN A SCREENSHOT, A RECORDING OR A LOG. The key does not expire. A
> person who gets it can start instances in the project until the key is
> revoked. Keep the key file private with mode `0600`.

> **CAUTION:** DO NOT ADD A ROLE TO THE SERVICE ACCOUNT FOR A FAILED STEP. The
> account needs `roles/compute.instanceAdmin.v1` and no more. A wider role lets
> a defect delete the resources of other people.

> **CAUTION:** DELETE ONLY A RESOURCE THAT THIS RUN MADE. Read the cloud ID and
> the label that names it before you delete anything in the project.

## 4. Equipment and preconditions

- A frozen candidate binary and its SHA-256.
- The fixture `scripts/device-smoke/serve.py` with `--native-view` and
  `--saved-session`, and a Device panel that shows the desktop live.
- A Google Cloud test project with Compute Engine enabled, a billing account
  linked, and quota for one instance of the machine type that the profile needs
  in one zone.
- A service account in that project with the role
  `roles/compute.instanceAdmin.v1`, and its JSON key in a private file outside
  the repository with mode `0600`.
- An SSH identity file with its `.pub` file, outside the repository.
- A cloud settings file, `$CLOUD/settings.json`, that names the key file, the
  project and the SSH identity file. `$CLOUD` is `HORIZON_HOME/cloud` of the
  fixture.
- A test Git repository with this `.horizon/cloud.yml` file:

  ```yaml
  version: 1
  default: gcp-build
  profiles:
    gcp-build:
      provider: gcp
      image: example.invalid/worker:latest
      cpu: 4
      memory_gb: 16
      gpu: false
      storage: {container_gb: 30, volume_gb: 64}
      bootstrap: {readiness_seconds: 900}
      capabilities: {agents: [claude], browsers: [chromium], desktop: true}
  ```

- No `gcloud` command-line tool. The procedure needs none. Step 4 of task
  GCP-L8 proves that the delete proof holds without a token.
- `jq`, `ssh` and `curl`.
- An MCP client for the Horizon tools, for task GCP-L5.

## 5. Setup

1. Build the candidate and freeze the binary.

   ```sh
   cargo build -p horizon-ui --bin horizon
   mkdir -p "$HOME/horizon-smoke"
   smoke_bin=$(mktemp -d "$HOME/horizon-smoke/gcp-bin.XXXXXX")
   cp "$CARGO_TARGET_DIR/debug/horizon" "$smoke_bin/horizon"
   (cd "$smoke_bin" && sha256sum horizon > SHA256SUMS)
   ```

   Result: `$smoke_bin/SHA256SUMS` holds one SHA-256 value.

2. Start the fixture with a private state directory that it keeps.

   `--reuse-state` is required. Without it the fixture deletes `data/` when it
   stops, which removes `settings.json` and the provider journals. A later task
   then cannot delete a cloud that still bills, or prove its resources gone.
   `--reuse-state` also lets a task stop the candidate and start it again in the
   same state, which tasks GCP-01 and the restart tasks need.

   Do not put the state in `/tmp`. On this computer `/tmp` is a tmpfs with a
   per-user quota, and a full `/tmp` makes every command fail.

   ```sh
   export SMOKE_STATE="$HOME/horizon-smoke/gcp-run"
   mkdir -p "$SMOKE_STATE"
   python3 scripts/device-smoke/serve.py --horizon "$smoke_bin/horizon" \
     --native-view --saved-session --reuse-state --state "$SMOKE_STATE"
   ```

   Result: The fixture prints a manifest with `vnc_address`. `$SMOKE_STATE/data`
   exists and stays after a stop.

3. Open a Device panel on `vnc_address`. Examine the viewer health.

   Result: `image_received` and `image_displayed` are true. `frame_sequence`
   increases while the desktop changes.

4. Find the Horizon child of the fixture. Compare its executable with the frozen
   value.

   ```sh
   sha256sum /proc/<pid>/exe
   ```

   Result: The two values are the same.

5. Make a private directory for the run, and declare the helpers of this
   procedure.

   ```sh
   umask 077
   export GCPR="$HOME/gcp-run"
   mkdir -p "$GCPR"
   cdx() { cargo run -q -p horizon-core --example cloud_deploy -- "$@"; }
   export CLOUD="$SMOKE_STATE/data/home/.horizon/cloud"
   ```

   Result: `cdx offers "$CLOUD/settings.json"` prints a JSON answer.

6. Copy the service-account key into the private directory and read its project.

   ```sh
   cp <key file> "$GCPR/service-account.json"
   chmod 600 "$GCPR/service-account.json"
   jq '{project: .project_id, client: .client_email | length}' "$GCPR/service-account.json"
   ```

   Result: The copy has mode `0600`. The command prints the project ID and the
   length of the service-account name, and it prints no secret. Record the
   project ID.

7. Write the `gcp` section of the settings file with the absolute path of the
   key file, the project, the name that the candidate gives each field, and the
   SSH identity file.

   Result: The file is valid JSON. **Menu › Cloud settings** names the Google
   Cloud credential, and **New cloud** shows a **Google Cloud** entry.

## 6. Tasks

Do the tasks in order. Each task has a stable ID for the report. Tasks GCP-L1 to
GCP-L3 and GCP-L7 use one cloud, `$CLOUD/$ID`, which task GCP-L1 makes. Task
GCP-L4 makes a second cloud in its own state root. Task GCP-L5 makes a third
cloud for the agent panel. Task GCP-L6 makes a fourth cloud and keeps it for
more than one hour. Task GCP-L8 deletes the cloud of task GCP-L1.

### 6.1 GCP-01 — Record the baseline before the change

1. Start the fixture with the build of the commit before the Google Cloud
   provider landed, and the state directory of section 5.

   Result: **New cloud** shows a RunPod entry, a Hetzner entry and an Azure
   entry. It shows no **Google Cloud** entry.

2. Read the offers of that build.

   ```sh
   cdx offers "$CLOUD/settings.json" | jq -r '.other_providers[].provider' | sort -u
   ```

   Result: The list names no Google Cloud provider. The state root of each cloud
   has no `gcp.json`.

3. Stop the fixture with `Ctrl+C`.

   > **CAUTION:** STOP THE FIXTURE ONLY WHILE NO GOOGLE CLOUD CLOUD EXISTS. A
   > stop with `--reuse-state` keeps `data/`, so `gcp.json` and
   > `deployment.json` survive. Without them a disk and a static address
   > continue to bill with nothing left that can delete them or prove them gone.

   Result: The fixture stops its display, its window manager and its Horizon.

4. Start the fixture with the candidate binary and the same state directory.

   Result: **New cloud** shows a **Google Cloud** entry beside the other
   entries.

5. Read the offers of the candidate.

   ```sh
   cdx offers "$CLOUD/settings.json" | jq '.other_providers[] | select(.provider=="Google Cloud")'
   ```

   Result: The answer has a Google Cloud section. It names the deployable state
   of the provider and one or more offers with a price in US dollars. Record the
   field names of the section.

### 6.2 GCP-02 — Run the headless tests

1. Run the tests of the Google Cloud client and of the settings section.

   ```sh
   cargo test -p horizon-cloud --lib -- gcp
   cargo test -p horizon-core --lib -- cloud_runtime::settings cloud_runtime::providers
   cargo test -p horizon-core --lib -- cloud_runtime::deployment
   ```

   Result: All tests pass. Record the count of each command. The tests examine
   these items:
   - A signed assertion that a recorded token endpoint accepts, byte for byte,
     and an access token that Horizon keeps and renews before it expires.
   - A key file that another user can read, a project that the key names not,
     and a profile that the catalog cannot place, each refused before a request.
   - A create behind a durable fence, a lost answer, and a read-only retry.
   - A stop that is reported only from the instance state `TERMINATED`.
   - A delete that proves the instance, the workspace disk, the static address
     and the label sweep absent.
   - A release that keeps the workspace disk, and its refusal when a release
     would take the workspace.

   Record which of these items a test examines. An item that no test examines
   is a finding for the pull request that adds the adapter.

2. Prove that Horizon calls no Google Cloud command-line tool.

   ```sh
   grep -rn "Command::new" crates/horizon-cloud/src/ | grep -v docker_host
   ```

   Result: The command prints nothing.

### 6.3 GCP-03 — Examine the dialog without a `gcp` section

1. Close the dialog. Rename the `gcp` section out of the settings file, so that
   the file holds no `gcp` section.

   Result: The file is valid JSON.

2. Open the test repository in **New cloud**.

   Result: The heading shows one dimmed **Google Cloud** entry. **All machines
   and filters** shows no **Google Cloud** filter. No row starts with
   `Google Cloud ·`.

3. Click the **Google Cloud** entry in the heading.

   Result: The entry is marked as the cloud's provider. One line under the title
   names the `gcp` section and what to add to it. No Google Cloud row shows.
   **Start cloud** stays disabled.

4. Put the `gcp` section back, with the key file of section 5.

   Result: The file is valid JSON, and the key file has mode `0600`.

### 6.4 GCP-04 — Choose a Google Cloud row

1. Open the test repository in **New cloud**. Keep the profile `gcp-build`.

   Result: The **Google Cloud** entry is no longer dimmed. **All machines and
   filters** shows a **Google Cloud** filter.

2. Click the **Google Cloud** filter.

   Result: The list shows rows that start with `Google Cloud ·`. Each row names
   one machine type, one location and a price in `$`. No row shows a GPU type.

3. Click one row. Read the summary.

   Result: The summary names the machine type, the location, the compute price
   in `$`/hr and the run estimate in `$`. It names the two storage fields:
   **Workspace disk** and **Boot disk**. The note under **Boot disk** says that
   the boot disk goes with the instance. **Start cloud** is enabled.

4. Close the dialog and open it again. Click the **Google Cloud** filter before
   the prices load.

   Result: The rows fill in when the prices arrive, with no second click.

### 6.5 GCP-05 — Examine a credential that Horizon cannot use

1. Close the dialog. Make the key file readable by others.

   ```sh
   chmod 644 "$GCPR/service-account.json"
   touch "$CLOUD/settings.json"
   ```

   Result: The mode of the file is `0644`.

2. Open the test repository in **New cloud**.

   Result: The provider check fails with the reason that the file is not
   private. No row starts with `Google Cloud ·`. **Start cloud** stays disabled.

3. Close the dialog. Set the mode back, and keep a copy of the good key.

   ```sh
   chmod 600 "$GCPR/service-account.json"
   cp "$GCPR/service-account.json" "$GCPR/service-account-good.json"
   ```

   Result: The mode of the file is `0600`, and the copy exists.

4. Use a key that the token endpoint rejects, for example a key that you
   revoked in the project.

   ```sh
   cp <revoked key file> "$GCPR/service-account.json"
   chmod 600 "$GCPR/service-account.json"
   touch "$CLOUD/settings.json"
   ```

   Result: The key file holds a key that the project revoked.

5. Open the test repository in **New cloud**. Wait for the price answer.

   Result: The check fails with the reason that the token request gave, such as
   an invalid grant. The dialog names the reason, and no instance row shows.
   **Start cloud** stays disabled. Nothing is rented.

6. Close the dialog. Put the good key back.

   ```sh
   cp "$GCPR/service-account-good.json" "$GCPR/service-account.json"
   ```

   Result: The rows come back with no restart.

### 6.6 GCP-06 — Examine a profile that Google Cloud cannot place

1. Change the profile to ask for a GPU.

   ```sh
   cp <repo>/.horizon/cloud.yml "$GCPR/cloud.yml.save"
   sed -i 's/gpu: false/gpu: true/' <repo>/.horizon/cloud.yml
   ```

   Result: The profile asks for a GPU.

2. Open the test repository in **New cloud**.

   Result: The **Google Cloud** entry is absent for this profile, because Google
   Cloud supports no GPU worker. **Start cloud** stays disabled. No instance is
   rented.

3. Change the memory of the profile above what a machine type of the project
   has.

   Result: **Start cloud** stays disabled, and the check names the reason.

4. Put the saved profile back.

   ```sh
   cp "$GCPR/cloud.yml.save" <repo>/.horizon/cloud.yml
   ```

   Result: `git -C <repo> status --short` shows no change of the profile.

### 6.7 GCP-07 — Examine a provider refusal for capacity

> **CAUTION:** THIS TASK CHANGES THE QUOTA OR THE ZONE OF THE TEST PROJECT. Use a
> quota that refuses the request, or a zone without stock. Do not change a quota
> that another person uses, and put the value back at the end of the task.

1. Set the quota of the machine type to zero in the test region, or choose a
   location that the project has no stock for. Do this in the Cloud console and
   keep the previous value.

   Result: The project cannot make the machine type of the profile in that
   location.

> **CAUTION:** THIS STEP RENTS COMPUTE. A refused request usually makes no
> resource, but a partial resource that it leaves bills until you delete it.

2. Open the test repository in **New cloud**. Choose the Google Cloud row and
   click **Start cloud**. Record the cloud ID of the card as `$ID_REFUSED`.

   Result: The card shows the provider refusal with the reason that Google Cloud
   gave, such as an exhausted resource pool or a quota reason. The fence of the
   deployment record is not `terminated`. Record the reason and the stage.

3. Examine the records and prove that no instance runs.

   ```sh
   jq '{stage, operation, worker}' "$CLOUD/$ID_REFUSED/deployment.json"
   jq . "$CLOUD/$ID_REFUSED/gcp.json"
   cdx reconcile "$CLOUD/settings.json" "$CLOUD/$ID_REFUSED"
   ```

   Result: The reconcile report names no running instance. `gcp.json` names what
   Horizon recorded, and no labeled resource that it made stays.

> **CAUTION:** THIS STEP DELETES THE CLOUD OF THE REFUSED REQUEST. A partial
> resource that the refusal left bills until you delete it.

4. Delete the cloud of the refused request, as task GCP-L8 describes.

   Result: The card reads **Nothing billing now**, and the delete proof holds.

> **CAUTION:** THE PREVIOUS VALUE AGAIN ALLOWS INSTANCES OF THAT TYPE IN THE
> LOCATION. The project can then bill for them.

5. Put the quota or the location back to its previous value.

   Result: The console shows the previous value.

### 6.8 GCP-L1 — Rent one Google Cloud cloud and reach it over SSH

> **CAUTION:** THIS TASK RENTS ONE COMPUTE ENGINE INSTANCE, ONE PERSISTENT DISK
> AND ONE STATIC ADDRESS. They bill from the start until the delete completes.
> Do not end the run before task GCP-L8 deletes the cloud.

1. Open **New cloud** for the test repository, keep the profile `gcp-build`, and
   click the **Google Cloud** filter. Read the checks under **BEFORE YOU START**.

   Result: The checks name the repository, the service-account key and the SSH
   identity. The list holds at least one row.

2. Click one row. Read the summary, and press **Start cloud**.

   Result: The card shows **Validate**, then **Build locally**, **Push image**,
   **Provision worker**, **Check readiness**, **Prepare worktrees**, **Start
   sessions** and **Ready**. Record the ready time, the instance name and the
   location. Record the cloud ID as `$ID`.

3. Read the SSH endpoint of the cloud.

   ```sh
   export CLOUD_DIR="$CLOUD/$ID"
   cdx endpoint "$CLOUD/settings.json" "$CLOUD_DIR" > "$GCPR/endpoint-1.json"
   jq . "$GCPR/endpoint-1.json"
   ```

   Result: The answer names `host`, `port`, `user`, `host_key_alias`,
   `known_hosts` and `identity_file`.

4. Reach the worker over SSH.

   ```sh
   ssh -i "$(jq -r .identity_file "$GCPR/endpoint-1.json")" \
     -o UserKnownHostsFile="$(jq -r .known_hosts "$GCPR/endpoint-1.json")" \
     -o StrictHostKeyChecking=yes \
     "$(jq -r .user "$GCPR/endpoint-1.json")@$(jq -r .host "$GCPR/endpoint-1.json")" \
     'hostname; grep PRETTY_NAME /etc/os-release; findmnt -no TARGET,SOURCE,FSTYPE /workspace; ls /etc/horizon-worker'
   ```

   Result: SSH answers with no prompt. The first line is the worker identity,
   `/workspace` is a mount of the workspace disk, and `/etc/horizon-worker` holds
   `agent-versions.json` and `capabilities.json`.

5. Write a marker on the workspace disk and record the identity of the
   resources.

   ```sh
   ssh -i "$(jq -r .identity_file "$GCPR/endpoint-1.json")" \
     -o UserKnownHostsFile="$(jq -r .known_hosts "$GCPR/endpoint-1.json")" \
     "$(jq -r .user "$GCPR/endpoint-1.json")@$(jq -r .host "$GCPR/endpoint-1.json")" \
     'mkdir -p /workspace/checkout && printf gcp-marker > /workspace/checkout/gcp-marker && sha256sum /workspace/checkout/gcp-marker'
   jq '{worker: .worker.id, address: .worker.address}' "$CLOUD_DIR/deployment.json"
   jq . "$CLOUD_DIR/gcp.json"
   ```

   Result: The first command prints one SHA-256. Record it as `$MARKER`. The
   record names the instance name and the address. `gcp.json` names the
   instance, the workspace disk and the static address of the cloud, each with
   the label that names the cloud. Record the disk name as `$DISK` and the
   address name as `$ADDRESS`.

6. Open a Shell panel on the cloud from the card.

   Result: The panel runs on the worker. `findmnt -no SOURCE /workspace` names
   the same disk as `$DISK`.

### 6.9 GCP-L2 — Stop, examine the reported state, and resume

> **CAUTION:** THE STOP ENDS EVERY PROCESS IN THE CLOUD. An agent session, a
> terminal command and a build that runs on the worker do not continue.

1. Record the reported state before the stop.

   ```sh
   cdx reconcile "$CLOUD/settings.json" "$CLOUD_DIR"
   ```

   Result: The report names a running instance. Record the time.

2. Press **Stop…** on the card, then press the button of the confirmation.

   Result: The card reads **Stop requested**. The processes in the cloud end.

3. Read the reported state twice while the stop runs.

   ```sh
   cdx reconcile "$CLOUD/settings.json" "$CLOUD_DIR"
   sleep 20
   cdx reconcile "$CLOUD/settings.json" "$CLOUD_DIR"
   ```

   Result: The first report names a stop that is under way, and it is not
   `Inactive`. The second report is `Inactive` only after the provider reports
   the instance `TERMINATED`. Record the time of each report and the time that
   the card first reads **Stopped**.

   The card reads **Stopped** only after a report shows `Inactive`. A card that
   reads **Stopped** before the provider reports the stop is a finding.

4. Examine the resources that stay.

   ```sh
   jq . "$CLOUD_DIR/gcp.json"
   jq '{stage, stop_requested: .stop_requested, worker: .worker}' "$CLOUD_DIR/deployment.json"
   ```

   Result: The instance is `TERMINATED`. The workspace disk `$DISK` stays. The
   static address `$ADDRESS` stays, with the same public address. The record has
   `stop_requested` true and the stage `Stopped`.

> **CAUTION:** THE RESUME RENTS THE INSTANCE AGAIN. The instance bills from
> the start until the next stop or delete.

5. Press **Resume** on the card.

   Result: The card shows **Check readiness**, then **Start sessions**, then
   **Ready**.

6. Make sure that the same instance, disk and address run again.

   ```sh
   jq '{worker: .worker.id, address: .worker.address}' "$CLOUD_DIR/deployment.json"
   cdx endpoint "$CLOUD/settings.json" "$CLOUD_DIR" > "$GCPR/endpoint-2.json"
   ssh -i "$(jq -r .identity_file "$GCPR/endpoint-2.json")" \
     -o UserKnownHostsFile="$(jq -r .known_hosts "$GCPR/endpoint-2.json")" \
     -o StrictHostKeyChecking=yes \
     "$(jq -r .user "$GCPR/endpoint-2.json")@$(jq -r .host "$GCPR/endpoint-2.json")" \
     'sha256sum /workspace/checkout/gcp-marker; findmnt -no SOURCE /workspace'
   ```

   Result: The instance name, the address and the disk name are the values of
   task GCP-L1 step 5. The SHA-256 is `$MARKER`. SSH accepts the same host key.

### 6.10 GCP-L3 — Rebuild on a new instance and keep the workspace disk

1. Record the state before the rebuild.

   ```sh
   jq '{worker: .worker.id, address: .worker.address}' "$CLOUD_DIR/deployment.json"
   jq -r .workspace "$CLOUD_DIR/gcp.json"
   ```

   Result: Record the instance name as `$INSTANCE`, the address and `$DISK`.

2. Change the recipe of the image and commit the change.

   ```sh
   echo 'RUN echo gcp-l3 > /etc/gcp-l3' >> <repo>/<path of the Dockerfile>
   git -C <repo> commit -qam "test: gcp rebuild"
   ```

   Result: `git -C <repo> log -1 --oneline` shows the commit.

> **CAUTION:** THE REBUILD DELETES THE INSTANCE AND RENTS A NEW ONE. The
> processes in the container end. The workspace disk stays.

3. Open the chevron next to **Stop…** on the card and select **Rebuild image &
   restart…**. Read the confirmation, then press its button.

   Result: The confirmation names the instance, the same workspace disk and the
   same address. The card shows **Replace image**, then **Provision worker**,
   **Check readiness** and **Ready**.

4. Make sure that the new instance holds the same disk and the same address.

   ```sh
   jq '{worker: .worker.id, address: .worker.address}' "$CLOUD_DIR/deployment.json"
   jq . "$CLOUD_DIR/gcp.json"
   jq . "$CLOUD_DIR/gcp-release.json"
   ```

   Result: The instance name is not `$INSTANCE`. The address is the value before
   the rebuild. The workspace disk is `$DISK`. `gcp-release.json` holds
   `{"held": true}`.

5. Make sure that the workspace and the new image are in place.

   ```sh
   cdx endpoint "$CLOUD/settings.json" "$CLOUD_DIR" > "$GCPR/endpoint-3.json"
   ssh -i "$(jq -r .identity_file "$GCPR/endpoint-3.json")" \
     -o UserKnownHostsFile="$(jq -r .known_hosts "$GCPR/endpoint-3.json")" \
     -o StrictHostKeyChecking=yes \
     "$(jq -r .user "$GCPR/endpoint-3.json")@$(jq -r .host "$GCPR/endpoint-3.json")" \
     'sha256sum /workspace/checkout/gcp-marker; cat /etc/gcp-l3'
   ```

   Result: The SHA-256 is `$MARKER`. The last line is `gcp-l3`.

> **CAUTION:** A DISK WITH THIS ATTRIBUTE IS DELETED WHEN THE INSTANCE IS
> DELETED. Put the attribute back directly after the examination in step 8.

6. Make the release unsafe: set the workspace disk to be deleted with the
   instance. Do this in the Cloud console on the disk `$DISK`, and keep the
   previous value.

   Result: The disk shows that it is deleted with the instance.

7. Request a rebuild again, as in step 3.

   Result: Horizon refuses the rebuild before it deletes the instance. The
   message names the workspace disk. The instance, the address and the disk
   stay. The image is not built again. Record the text.

8. Put the attribute of the disk back.

   Result: The disk shows again that it stays when the instance is deleted.

### 6.11 GCP-L4 — Create, stop, resume and delete from the CLI plan runner

> **CAUTION:** THIS TASK RENTS A SECOND COMPUTE ENGINE INSTANCE, A PERSISTENT
> DISK AND A STATIC ADDRESS. It bills until step 5 deletes it.

1. Print the plan of the deployment.

   ```sh
   mkdir -p "$GCPR/cli"
   cdx plan-deploy "$CLOUD/settings.json" <repo> gcp-build "$GCPR/cli" gcp-l4-01
   ```

   Result: The command prints the profile, the location, the machine type, the
   price, the files of the working copy, and a plan token. Record the token.

2. Deploy with that token.

   ```sh
   cdx deploy "$CLOUD/settings.json" <repo> gcp-build "$GCPR/cli" gcp-l4-01 --plan-token <token>
   ```

   Result: The command prints the stage lines of task GCP-L1 step 2 and ends
   with `Stage: Ready` and `Ready: worker=<instance name>`. Record the instance
   name.

3. Reach the cloud over SSH and write a marker.

   ```sh
   cdx endpoint "$CLOUD/settings.json" "$GCPR/cli" > "$GCPR/cli-endpoint.json"
   ssh -i "$(jq -r .identity_file "$GCPR/cli-endpoint.json")" \
     -o UserKnownHostsFile="$(jq -r .known_hosts "$GCPR/cli-endpoint.json")" \
     -o StrictHostKeyChecking=yes \
     "$(jq -r .user "$GCPR/cli-endpoint.json")@$(jq -r .host "$GCPR/cli-endpoint.json")" \
     'printf gcp-cli-marker > /workspace/checkout/cli-marker && sha256sum /workspace/checkout/cli-marker'
   ```

   Result: SSH answers and prints one SHA-256. Record it as `$CLI_MARKER`.

> **CAUTION:** THE STOP ENDS EVERY PROCESS IN THE CLOUD, AND THE RESUME RENTS
> THE INSTANCE AGAIN. The instance bills from the resume until the delete.

4. Stop the cloud, examine the reported state, then resume it.

   ```sh
   cdx stop "$CLOUD/settings.json" "$GCPR/cli"
   cdx reconcile "$CLOUD/settings.json" "$GCPR/cli"
   cdx resume "$CLOUD/settings.json" "$GCPR/cli"
   cdx reconnect "$CLOUD/settings.json" "$GCPR/cli"
   ```

   Result: `stop` ends with no error. `reconcile` reports `Inactive` and prints
   `Stopped. Resume starts the same worker again.` `resume` asks for the
   reconnect, and `reconnect` ends with `Stage: Ready`. The record names the same
   instance, and the marker hash is `$CLI_MARKER`.

> **CAUTION:** THIS DELETE REMOVES THE INSTANCE, THE WORKSPACE DISK AND THE
> STATIC ADDRESS OF CLOUD `gcp-l4-01`. The files on its disk are lost.

5. Request the delete.

   ```sh
   cdx delete "$CLOUD/settings.json" "$GCPR/cli"
   ```

   Result: The command prints the deletion stages and ends with no error.

6. Prove that the resources are gone.

   ```sh
   jq '{stage, operation}' "$GCPR/cli/deployment.json"
   jq . "$GCPR/cli/gcp.json"
   cdx delete "$CLOUD/settings.json" "$GCPR/cli"
   ```

   Result: The stage is `Deleted`. `gcp.json` shows `deleting`, `swept`, and the
   instance, the disk and the address terminated. The second delete ends at once
   and sends no provider request.

### 6.12 GCP-L5 — Create and delete from the agent tools

> **CAUTION:** THIS TASK RENTS A THIRD COMPUTE ENGINE INSTANCE, A PERSISTENT
> DISK AND A STATIC ADDRESS. Step 5 deletes them. Do not end the run before it.

> **CAUTION:** THIS STEP WRITES AN AGENT POLICY THAT ALLOWS SPENDING. Copy the
> settings file first, and put the copy back in step 6. A wide policy allows a
> create that the person did not approve.

1. Add an `agent_lifecycle` section to the settings file that allows Google
   Cloud, the test repository and the hourly rate of the profile, and copy the
   settings file first.

   Result: **Menu › Cloud settings** shows the agent policy of Google Cloud as
   allowed.

2. From the agent panel, call `cloud_offers`.

   Result: The answer has a Google Cloud section with one or more offers with a
   price in US dollars. Record the field names.

3. Call `cloud_create` with `operation submit`, a lowercase UUID
   `operation_id`, the absolute `repository` and `quick_start: true`. Then call
   it again with `operation status` and the same `operation_id`, until the phase
   is not pending.

   Result: The answer reaches `phase: ready` and names one cloud ID. A Google
   Cloud cloud appears on the card of the agent's workspace.

4. Call `cloud_list`, then `cloud_exec` with that cloud ID.

   ```sh
   hostname; findmnt -no SOURCE /workspace
   ```

   Result: `cloud_list` names the cloud with its Google Cloud placement and a
   ready state. `cloud_exec` prints the worker identity and the workspace disk.

> **CAUTION:** THIS DELETE REMOVES THE INSTANCE, THE WORKSPACE DISK AND THE
> STATIC ADDRESS OF THE CLOUD THAT THE AGENT MADE.

5. Call `cloud_delete` with the cloud ID and a new lowercase UUID
   `operation_id`.

   Result: The answer reports the delete. `cloud_list` no longer names the
   cloud, or it names it as deleted.

6. Put the copied settings file back.

   Result: The file has the sections that it had before step 1.

### 6.13 GCP-L6 — Run for more than one hour and prove the token renewal

> **CAUTION:** THIS TASK RENTS A FOURTH COMPUTE ENGINE INSTANCE, A PERSISTENT
> DISK AND A STATIC ADDRESS FOR MORE THAN ONE HOUR. Keep the same session for
> the whole task, and delete the cloud in step 6.

1. Note the time, then print the plan and deploy a cloud.

   ```sh
   date -u +%FT%TZ
   mkdir -p "$GCPR/long"
   cdx plan-deploy "$CLOUD/settings.json" <repo> gcp-build "$GCPR/long" gcp-l6-01
   cdx deploy "$CLOUD/settings.json" <repo> gcp-build "$GCPR/long" gcp-l6-01 --plan-token <token>
   ```

   Result: The deploy ends with `Stage: Ready`. Record the time as `$T0`.

2. Do nothing to the cloud for 65 minutes. Do not press Resume, Stop or any
   action on the card.

   Result: The card stays **Ready**. Record the time as `$T1`.

3. Make sure that the cloud still answers with the same instance and the same
   workspace.

   ```sh
   cdx endpoint "$CLOUD/settings.json" "$GCPR/long" > "$GCPR/long-endpoint.json"
   ssh -i "$(jq -r .identity_file "$GCPR/long-endpoint.json")" \
     -o UserKnownHostsFile="$(jq -r .known_hosts "$GCPR/long-endpoint.json")" \
     -o StrictHostKeyChecking=yes \
     "$(jq -r .user "$GCPR/long-endpoint.json")@$(jq -r .host "$GCPR/long-endpoint.json")" \
     'printf gcp-long > /workspace/checkout/long-marker && sha256sum /workspace/checkout/long-marker'
   ```

   Result: SSH answers, and the command prints one SHA-256. Record it.

4. Examine the log of the run for the second access token.

   Result: The log records a token that Horizon made after the first one, before
   step 5. The second token request is later than `$T0`, and it is earlier than
   the stop request of step 5. The log shows no token value and no key.

   If the candidate records no token request, record that as a finding. The
   renewal then rests on step 5 and step 6 alone.

> **CAUTION:** THE STOP ENDS EVERY PROCESS IN THE CLOUD.

5. Stop the cloud.

   ```sh
   cdx stop "$CLOUD/settings.json" "$GCPR/long"
   cdx reconcile "$CLOUD/settings.json" "$GCPR/long"
   ```

   Result: Both commands end with no credential error. `reconcile` reports
   `Inactive` after the instance reaches `TERMINATED`, and it prints
   `Stopped. Resume starts the same worker again.`

> **CAUTION:** THIS DELETE REMOVES THE INSTANCE, THE WORKSPACE DISK AND THE
> STATIC ADDRESS OF CLOUD `gcp-l6-01`. The files on its disk are lost.

6. Delete the cloud.

   ```sh
   cdx delete "$CLOUD/settings.json" "$GCPR/long"
   jq '{stage, operation}' "$GCPR/long/deployment.json"
   jq . "$GCPR/long/gcp.json"
   ```

   Result: The delete ends with no credential error. The stage is `Deleted`.
   `gcp.json` shows `deleting`, `swept`, and the instance, the disk and the
   address terminated.

   The elapsed time from `$T0` to `$T1` is more than one hour, and no step of the
   task asked a person for a token or a key.

### 6.14 GCP-L7 — Examine what a restart preserves

> **CAUTION:** THIS STEP STOPS THE CLOUD. Every process in the cloud ends, and
> an agent session that runs on the worker does not continue.

1. Stop the cloud of task GCP-L1 from its card, and record its state. Do not
   delete it.

   Result: The card reads **Stopped**. Record the cloud ID, the instance name,
   the workspace disk `$DISK` and the address `$ADDRESS`.

2. Record the files of the cloud and the sections of the settings file.

   ```sh
   sha256sum "$CLOUD_DIR/gcp.json" "$CLOUD_DIR/deployment.json"
   jq 'keys' "$CLOUD/settings.json"
   ls "$CLOUD"
   ```

   Result: Record the two hashes. The settings file holds the `gcp` section.

3. Stop the candidate, and start the same frozen binary in the same fixture
   state.

   Result: The fixture prints a new manifest, and the desktop shows the same
   workspace.

4. Compare the workspace and the records with step 1 and step 2.

   ```sh
   sha256sum "$CLOUD_DIR/gcp.json" "$CLOUD_DIR/deployment.json"
   ```

   Result: Each cloud card comes back with the same cloud ID. The card reads
   **Stopped**, with the same instance name, the same workspace disk and the same
   address. The two hashes are the values of step 2, so no provider request
   changed a record.

> **CAUTION:** THE RESUME RENTS THE INSTANCE AGAIN. The instance bills from
> the start until the next stop or delete.

5. Press **Resume** on the card.

   Result: The card reaches **Ready** with the same instance and the same
   address. SSH answers with the same host key, and the marker hash is
   `$MARKER`. Horizon minted a new access token at this start, with no action by
   a person.

6. Examine the settings and the state of another provider.

   ```sh
   jq 'keys' "$CLOUD/settings.json"
   ls "$CLOUD"
   ```

   Result: The `gcp` section is unchanged. The state directory of a cloud of
   another provider keeps its bytes.

### 6.15 GCP-L8 — Delete the cloud and prove the absence with no token

> **CAUTION:** THIS DELETE REMOVES THE INSTANCE, THE WORKSPACE DISK `$DISK`
> AND THE STATIC ADDRESS `$ADDRESS`. The files on the disk, including
> `$MARKER`, are lost.

1. Delete the cloud of task GCP-L1 from its card.

   Result: The card reads **Worker deleted**, **Workspace storage cleaned up**
   and **Nothing billing now**.

2. Examine the records of the cloud.

   ```sh
   jq '{stage, operation}' "$CLOUD_DIR/deployment.json"
   jq . "$CLOUD_DIR/gcp.json"
   ```

   Result: The stage is `Deleted`. The operation is `terminated` with the
   instance name. `gcp.json` shows `deleting`, `swept`, and the instance, the
   workspace disk and the static address terminated. The label sweep is empty.

> **CAUTION:** THIS STEP REMOVES THE ACCESS OF THIS MACHINE TO GOOGLE CLOUD.
> Keep one copy of the settings file outside the repository, or Horizon
> cannot reach the project again in this run.

3. Remove the credential from the machine, and prove that the absence proof
   still holds.

   ```sh
   mv "$CLOUD/settings.json" "$GCPR/settings-with-gcp.json"
   jq 'del(.gcp)' "$GCPR/settings-with-gcp.json" > "$CLOUD/settings.json"
   jq 'has("gcp")' "$CLOUD/settings.json"
   ```

   Result: The settings file has no `gcp` section. It is valid JSON.

4. Open the cloud card of the deleted cloud. Select **Remove cloud**.

   Result: The card goes away. Horizon read the proof from the journals and sent
   no provider request, because the settings hold no `gcp` section and no token.

5. Run the headless proofs of the deletion and the removal.

   ```sh
   cargo test -p horizon-core --lib -- cloud_runtime::providers::deletion cloud_runtime::deployment::reconnect
   ```

   Result: All selected tests pass, including the test that reads the deletion
   proof of a `gcp.json` with no token.

6. Put the settings file with the `gcp` section back.

   Result: The Google Cloud entry in **New cloud** is ready again.

## 7. Pass criteria

- The baseline build shows no Google Cloud entry and no Google Cloud offer. The
  candidate shows both, and the offers name a price in US dollars.
- Without a `gcp` section the dialog names the section, and **Start cloud** stays
  disabled.
- A key file that others can read, and a key that the token endpoint rejects,
  each show their own reason, and nothing is rented.
- A GPU profile and a profile that no machine type of the project fits are each
  refused before a request.
- A provider refusal for capacity leaves no running instance and no labeled
  resource of the run.
- A person rents one Google Cloud cloud from **New cloud** without a `gcloud`
  command on the computer, reaches it over SSH, and writes on the workspace
  disk.
- The card reads **Stopped** only after a reconcile report shows `Inactive`, and
  the resume starts the same instance on the same workspace disk and the same
  address.
- A rebuild keeps the workspace disk `$DISK` and the static address `$ADDRESS`,
  and the marker keeps its SHA-256.
- A release that would take the workspace is refused before the instance is
  deleted, and no image is built.
- The CLI plan runner and the agent tools create, stop, resume and delete a
  Google Cloud cloud, and `cloud_exec` runs a command on the worker.
- A cloud that runs for more than one hour stops and deletes with no action by a
  person, and Horizon minted a new access token in that time.
- The delete proof holds in `gcp.json` when the settings file has no `gcp`
  section, and it names the instance, the disk, the address and an empty label
  sweep.
- A restart keeps each cloud ID, each reported state, each record and the `gcp`
  section, and makes no new resource.

## 8. Cleanup

> **CAUTION:** THIS STEP DELETES THE CLOUDS OF THIS RUN, WITH THEIR WORKSPACE
> DISKS AND THEIR STATIC ADDRESSES. Read the cloud ID of each card before you
> delete it.

1. Delete every cloud that this run made and that still runs or bills.

   Result: Each card reads **Nothing billing now**.

2. Examine the instance, disk and address lists of the test project in the Cloud
   console. Compare the label of each resource with the cloud IDs of the run
   before you delete anything.

   Result: The console holds no instance, disk or address of any cloud ID of the
   run.

3. Stop the fixture with `Ctrl+C`.

   Result: The fixture stops its display, its window manager and Horizon.

4. Remove the state directory, the frozen binary, the key copies and the markers
   of the run.

   ```sh
   rm -rf "$SMOKE_STATE" "$smoke_bin" "$GCPR"
   ```

   Result: The directories do not exist.

5. Remove the test commits of task GCP-L3 from `<repo>`, if you do not keep
   them.

   ```sh
   git -C <repo> log --oneline -2
   git -C <repo> reset --hard HEAD~1
   ```

   Result: The log shows no commit `test: gcp rebuild`.

## 9. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). If not, put the results in the
pull request. Keep private evidence out of the repository.
