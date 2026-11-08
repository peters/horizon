---
procedure: aws-cloud-provider
feature: AWS (EC2) as a cloud provider in New cloud, the CLI plan runner and the agent tools
platforms: [linux]
cost: rents compute   # tasks AWS-L1, AWS-L4 and AWS-L5 rent one cloud each
destructive: yes      # the delete tasks and the cleanup remove the instance, the workspace volume and the Elastic IP
secrets: [AWS access key id file of the cloud settings, AWS secret access key file of the cloud settings, SSH identity file of the cloud settings]
owner: horizon-maintainers
---

# AWS cloud provider test procedure

## 1. Purpose

This procedure proves that a person can pick AWS in **New cloud**, rent one AWS
cloud there, reach it over SSH, stop it, resume it, rebuild it and delete it.
It proves the same flows through the CLI plan runner and the agent tools. It
proves that the credential is an access key id and a secret access key that
Horizon uses itself, with no `aws` command on this computer. It proves that a
delete leaves no resource of the cloud, and that this proof needs no
credential.

## 2. Applicability

- Candidate: a build with the AWS provider of epic #2002. The build has the
  `aws` settings section, the `aws.json` journal, and one `Description` entry
  for AWS.
- Platforms: Linux with an isolated X display, from the local smoke fixture.
  Provider: AWS, beside RunPod, Hetzner, the Docker host and Azure.
- Tasks AWS-01 to AWS-06 cost nothing. They use headless tests, a loopback
  endpoint and a refused request. They need no AWS account. Task AWS-07 presses
  **Start cloud** one time and expects the refusal of the provider.
- Tasks AWS-L1, AWS-L4 and AWS-L5 rent one EC2 instance, one gp3 workspace
  volume and one Elastic IP for each cloud that they make. Task AWS-L2, task
  AWS-L3, task AWS-L6 and task AWS-L7 reuse the cloud of task AWS-L1.
- This procedure does not test:
  - The resources of another run in the same account. Use a test account.
  - The tailnet, the browser panels and the agent sessions of the cloud. Other
    procedures test them.
  - The AWS console and the AWS Price List console. This procedure reads them
    only in an optional step.
  - A GPU profile. AWS offers no GPU worker in this candidate.
  - The total cost since creation on the cloud card. `billing::history_supported`
    stays RunPod only, so an AWS card shows no total.
- The epic holds these points as open. This procedure cannot fix them. Where a
  step meets one, the step reads the value from the candidate:
  - The names of the two fields of the `aws` section.
  - The owner of the security group of a cloud.
  - The raw user-data limit of EC2, and the source of the worker image.

## 3. Safety

> **CAUTION:** EVERY LIVE TASK RENTS AN EC2 INSTANCE, A GP3 WORKSPACE VOLUME AND
> AN ELASTIC IP. They bill from the start until the delete completes. A cloud
> that you leave keeps billing. Delete each cloud in the same session.

> **CAUTION:** THE DELETE TASKS REMOVE THE INSTANCE, THE WORKSPACE VOLUME AND
> THE ELASTIC IP. The files on the workspace volume are lost. Use a test
> repository only, and never a cloud that holds work that you must keep.

> **CAUTION:** DO NOT PUT THE ACCESS KEY ID, THE SECRET ACCESS KEY, A SESSION
> TOKEN OR THE SSH PRIVATE KEY IN A SCREENSHOT, A RECORDING OR A LOG. A person
> who gets them can start instances in the account. Keep each file private with
> mode `0600`.

> **CAUTION:** DO NOT WIDEN THE IAM POLICY FOR A FAILED STEP. Horizon may use a
> destructive action only on a resource that carries the tag `ManagedBy=horizon`.
> A wider policy lets a defect delete the resources of other people.

> **CAUTION:** DELETE ONLY A RESOURCE THAT THIS RUN MADE. Read the cloud ID, the
> tag `ManagedBy=horizon` and the instance tag of the run before you delete
> anything in the account.

## 4. Equipment and preconditions

- A frozen candidate binary and its SHA-256.
- The fixture `scripts/device-smoke/serve.py` with `--native-view` and
  `--saved-session`, and a Device panel that shows the desktop live.
- An AWS test account with quota for one instance of the type that the profile
  needs, in one region of the account.
- An IAM user for the test with this policy, and no other permission:
  - The read actions that the adapter needs, such as `ec2:DescribeInstances`,
    `ec2:DescribeVolumes`, `ec2:DescribeAddresses` and `ec2:DescribeRegions`.
  - The create actions that the adapter needs, such as `ec2:RunInstances`,
    `ec2:CreateVolume`, `ec2:AttachVolume` and `ec2:AllocateAddress`.
  - Each destructive action with the condition `aws:ResourceTag/ManagedBy`
    equal to `horizon`. Put the tag condition on `instance/*` only.
    `RunInstances` takes no request tag for a root volume or a network
    interface, so a condition on those types refuses every launch.
- Two private files outside the repository, each with mode `0600`: one holds the
  access key id of the test user, one holds its secret access key.
- An SSH identity file with its `.pub` file, outside the repository.
- A cloud settings file, `$CLOUD/settings.json`, that names the two credential
  files and the SSH identity file. `$CLOUD` is
  `HORIZON_HOME/cloud` of the fixture.
- A test Git repository with this `.horizon/cloud.yml` file:

  ```yaml
  version: 1
  default: aws-build
  profiles:
    aws-build:
      provider: aws
      image: example.invalid/worker:latest
      cpu: 4
      memory_gb: 16
      gpu: false
      storage: {container_gb: 30, volume_gb: 64}
      bootstrap: {readiness_seconds: 900}
      capabilities: {agents: [claude], browsers: [chromium], desktop: true}
  ```

- No AWS command-line tool. The procedure needs none. Step 4 of task AWS-L7
  proves that the delete proof holds without a credential.
- `jq`, `ssh` and `curl`.
- An MCP client for the Horizon tools, for task AWS-L5.

## 5. Setup

1. Build the candidate and freeze the binary.

   ```sh
   cargo build -p horizon-ui --bin horizon
   smoke_bin=$(mktemp -d /tmp/horizon-aws-bin.XXXXXX)
   cp "$CARGO_TARGET_DIR/debug/horizon" "$smoke_bin/horizon"
   (cd "$smoke_bin" && sha256sum horizon > SHA256SUMS)
   ```

   Result: `$smoke_bin/SHA256SUMS` holds one SHA-256 value.

2. Start the fixture with a private state directory.

   ```sh
   python3 scripts/device-smoke/serve.py --horizon "$smoke_bin/horizon" \
     --native-view --saved-session --state /tmp/horizon-aws-run
   ```

   Result: The fixture prints a manifest with `vnc_address`.

3. Open a Device panel on `vnc_address`. Examine the viewer health.

   Result: `image_received` and `image_displayed` are true. `frame_sequence`
   increases while the desktop changes.

4. Find the Horizon child of the fixture. Compare its executable with the
   frozen value.

   ```sh
   sha256sum /proc/<pid>/exe
   ```

   Result: The two values are the same.

5. Make a private directory with two synthetic credential files. Do this
   before any live task, so that a mistake in a step cannot reach the account.

   ```sh
   umask 077
   export AWSS="$HOME/aws-run"
   mkdir -p "$AWSS"
   printf 'synthetic-access-key-id\n' > "$AWSS/access-key-id"
   printf 'synthetic-secret-value\n' > "$AWSS/secret-access-key"
   ```

   Result: The two files have mode `0600`.

6. Write the `aws` section of the settings file. Use the absolute paths of the
   two files, the name that the candidate gives each field, and the SSH identity
   file.

   Result: The file is valid JSON. **Menu › Cloud settings** names the AWS
   credential, and **New cloud** shows an **AWS** entry.

7. Declare the helpers of this procedure.

   ```sh
   cdx() { cargo run -q -p horizon-core --example cloud_deploy -- "$@"; }
   export CLOUD=/tmp/horizon-aws-run/data/home/.horizon/cloud
   ```

   Result: `cdx offers "$CLOUD/settings.json"` prints a JSON answer.

## 6. Tasks

Do the tasks in order. Each task has a stable ID for the report. Tasks AWS-L1 to
AWS-L3 and AWS-L6 use one cloud, `$CLOUD/$ID`, which task AWS-L1 makes. Task
AWS-L4 makes a second cloud in its own state root. Task AWS-L5 makes a third
cloud for the agent panel. Task AWS-L7 deletes the cloud of task AWS-L1.

### 6.1 AWS-01 — Record the baseline before the change

1. Start the fixture with the build of the commit before the AWS provider
   landed, and the state directory of section 5.

   Result: **New cloud** shows a RunPod entry, a Hetzner entry and an Azure
   entry. It shows no **AWS** entry.

2. Read the offers of that build.

   ```sh
   cdx offers "$CLOUD/settings.json" | jq -r '.other_providers[].provider' | sort -u
   ```

   Result: The list names no `AWS`. The state root of each cloud has no
   `aws.json`.

3. Stop the fixture with `Ctrl+C`.

   Result: The fixture stops its display, its window manager and its Horizon.

4. Start the fixture with the candidate binary and the same state directory.

   Result: **New cloud** shows an **AWS** entry beside the other entries.

5. Read the offers of the candidate.

   ```sh
   cdx offers "$CLOUD/settings.json" | jq '.other_providers[] | select(.provider=="AWS")'
   ```

   Result: The answer has an AWS section. It names the deployable state of AWS
   and one or more offers with a price in US dollars. Record the field names of
   the section.

### 6.2 AWS-02 — Run the headless tests

1. Run the tests of the AWS client and of the settings section.

   ```sh
   cargo test -p horizon-cloud --lib -- aws
   cargo test -p horizon-core --lib -- cloud_runtime::settings cloud_runtime::providers
   cargo test -p horizon-core --lib -- cloud_runtime::deployment
   ```

   Result: All tests pass. Record the count of each command. The tests examine
   these items:
   - A signature that the published test vectors give, byte for byte.
   - A refused credential, a profile that the catalog cannot place, and a
     workspace volume below the provider minimum, each before a request.
   - A create behind a durable fence, a lost answer, and a read-only retry.
   - A stop that is reported only from the provider's stopped state.
   - A delete that proves the instance, the volume, the address and the tag
     sweep absent.
   - A release that keeps the workspace volume, and its refusal when a release
     would take the workspace.

   Record which of these items a test examines. An item that no test examines
   is a finding for the pull request that adds the adapter.

2. Prove that Horizon calls no AWS command-line tool.

   ```sh
   grep -rn "Command::new" crates/horizon-cloud/src/ | grep -v docker_host
   ```

   Result: The command prints nothing.

### 6.3 AWS-03 — Examine the dialog without an `aws` section

1. Close the dialog. Rename the `aws` section out of the settings file, so that
   the file holds no `aws` section.

   Result: The file is valid JSON.

2. Open the test repository in **New cloud**.

   Result: The heading shows one dimmed **AWS** entry. **All machines and
   filters** shows no **AWS** filter. No row starts with `AWS ·`.

3. Click the **AWS** entry in the heading.

   Result: The entry is marked as the cloud's provider. One line under the title
   names the `aws` section and what to add to it. No AWS row shows. **Start
   cloud** stays disabled.

4. Put the `aws` section back, with the two credential files of section 5.

   Result: The file is valid JSON, and each credential file has mode `0600`.

### 6.4 AWS-04 — Choose an AWS row

1. Open the test repository in **New cloud**. Keep the profile `aws-build`.

   Result: The **AWS** entry is no longer dimmed. **All machines and filters**
   shows an **AWS** filter.

2. Click the **AWS** filter.

   Result: The list shows rows that start with `AWS ·`. Each row names one
   instance type, one region and a price in `$`. No row shows a GPU type.

3. Click one row. Read the summary.

   Result: The summary names the instance type, the region, the compute price in
   `$`/hr and the run estimate in `$`. It names the two storage fields:
   **Workspace volume** and **Root volume**. The note under **Root volume** says
   that the root volume goes with the instance. **Start cloud** is enabled.

4. Close the dialog and open it again. Click the **AWS** filter before the
   prices load.

   Result: The rows fill in when the prices arrive, with no second click.

### 6.5 AWS-05 — Examine a credential that Horizon cannot use

1. Close the dialog. Make the secret access key file readable by others.

   ```sh
   chmod 644 "$AWSS/secret-access-key"
   touch "$CLOUD/settings.json"
   ```

   Result: The mode of the file is `0644`.

2. Open the test repository in **New cloud**.

   Result: The provider check fails with the reason that the file is not
   private. No row starts with `AWS ·`. **Start cloud** stays disabled.

3. Close the dialog. Set the mode back, and keep a copy of the secret.

   ```sh
   chmod 600 "$AWSS/secret-access-key"
   cp "$AWSS/secret-access-key" "$AWSS/secret-access-key.save"
   ```

   Result: The mode of the file is `0600`, and the copy exists.

4. Write a secret that AWS rejects into the file.

   ```sh
   printf 'synthetic-rejected-secret\n' > "$AWSS/secret-access-key"
   touch "$CLOUD/settings.json"
   ```

   Result: The file holds a secret that the account does not have.

5. Open the test repository in **New cloud**. Wait for the price answer.

   Result: The check fails with the provider authentication reason. The dialog
   names the reason and no instance row shows. **Start cloud** stays disabled.
   Nothing is rented.

6. Close the dialog. Put the saved secret back.

   ```sh
   cp "$AWSS/secret-access-key.save" "$AWSS/secret-access-key"
   ```

   Result: The rows come back with no restart.

### 6.6 AWS-06 — Examine a profile that AWS cannot place

1. Change the profile to ask for a GPU.

   ```sh
   cp <repo>/.horizon/cloud.yml "$AWSS/cloud.yml.save"
   sed -i 's/gpu: false/gpu: true/' <repo>/.horizon/cloud.yml
   ```

   Result: The profile asks for a GPU.

2. Open the test repository in **New cloud**.

   Result: The **AWS** entry is absent for this profile, because AWS supports no
   GPU worker. **Start cloud** stays disabled. No instance is rented.

3. Change the memory of the profile above what an instance type of the account
   has.

   Result: **Start cloud** stays disabled, and the check names the reason.

4. Put the saved profile back.

   ```sh
   cp "$AWSS/cloud.yml.save" <repo>/.horizon/cloud.yml
   ```

   Result: `git -C <repo> status --short` shows no change of the profile.

### 6.7 AWS-07 — Examine a provider refusal for capacity

> **CAUTION:** THIS TASK CHANGES THE ON-DEMAND VCPU QUOTA OF THE TEST ACCOUNT.
> Set the quota of the test region to a value that refuses the request. Do not
> change a quota that another person uses, and put the value back at the end of
> the task.

1. Set the On-Demand vCPU quota of the test region to zero, or choose an
   instance type that the region has no stock for. Do this in the AWS console and
   keep the previous value.

   Result: The account cannot launch the type of the profile in that region.

> **CAUTION:** THIS STEP RENTS COMPUTE. A refused request usually makes no
> resource, but a partial resource that it leaves bills until you delete it.

2. Open the test repository in **New cloud**. Choose the AWS row and click
   **Start cloud**. Record the cloud ID of the card as `$ID_REFUSED`.

   Result: The card shows the provider refusal with the reason that AWS gave,
   such as a quota reason or a stock reason. The fence of the deployment record
   is not `terminated`. Record the reason and the stage.

3. Examine the records and prove that no instance runs.

   ```sh
   jq '{stage, operation, worker}' "$CLOUD/$ID_REFUSED/deployment.json"
   jq . "$CLOUD/$ID_REFUSED/aws.json"
   cdx reconcile "$CLOUD/settings.json" "$CLOUD/$ID_REFUSED"
   ```

   Result: The reconcile report names no running instance. `aws.json` names what
   Horizon recorded, and no tagged resource that it made stays.

> **CAUTION:** THIS STEP DELETES THE CLOUD OF THE REFUSED REQUEST. A partial
> resource that the refusal left bills until you delete it.

4. Delete the cloud of the refused request, as task AWS-L7 describes.

   Result: The card reads **Nothing billing now**, and the delete proof holds.

> **CAUTION:** THE PREVIOUS VALUE AGAIN ALLOWS INSTANCES OF THAT FAMILY IN THE
> REGION. The account can then bill for them.

5. Put the quota back to its previous value.

   Result: The console shows the previous value.

### 6.8 AWS-L1 — Rent one AWS cloud and reach it over SSH

> **CAUTION:** THIS TASK RENTS ONE EC2 INSTANCE, ONE GP3 WORKSPACE VOLUME AND
> ONE ELASTIC IP. They bill from the start until the delete completes. Do not
> end the run before task AWS-L7 deletes the cloud.

1. Write the real access key id and secret access key into the two credential
   files.

   Result: Each file has mode `0600`, and **Menu › Cloud settings** shows the
   AWS credential as ready.

2. Open **New cloud** for the test repository, keep the profile `aws-build`, and
   click the **AWS** filter. Read the checks under **BEFORE YOU START**.

   Result: The checks name the repository, the credential and the SSH identity.
   The list holds at least one row.

3. Click one row. Read the summary, and press **Start cloud**.

   Result: The card shows **Validate**, then **Build locally**, **Push image**,
   **Provision worker**, **Check readiness**, **Prepare worktrees**, **Start
   sessions** and **Ready**. Record the ready time, the instance id and the
   region. Record the cloud ID as `$ID`.

4. Read the SSH endpoint of the cloud.

   ```sh
   export CLOUD_DIR="$CLOUD/$ID"
   cdx endpoint "$CLOUD/settings.json" "$CLOUD_DIR" > "$AWSS/endpoint-1.json"
   jq . "$AWSS/endpoint-1.json"
   ```

   Result: The answer names `host`, `port`, `user`, `host_key_alias`,
   `known_hosts` and `identity_file`.

5. Reach the worker over SSH.

   ```sh
   ssh -i "$(jq -r .identity_file "$AWSS/endpoint-1.json")" \
     -o UserKnownHostsFile="$(jq -r .known_hosts "$AWSS/endpoint-1.json")" \
     -o StrictHostKeyChecking=yes \
     "$(jq -r .user "$AWSS/endpoint-1.json")@$(jq -r .host "$AWSS/endpoint-1.json")" \
     'hostname; grep PRETTY_NAME /etc/os-release; findmnt -no TARGET,SOURCE,FSTYPE /workspace; ls /etc/horizon-worker'
   ```

   Result: SSH answers with no prompt. The first line is the worker identity,
   `/workspace` is a mount of the workspace volume, and `/etc/horizon-worker`
   holds `agent-versions.json` and `capabilities.json`.

6. Write a marker on the workspace volume and record the identity of the
   resources.

   ```sh
   ssh -i "$(jq -r .identity_file "$AWSS/endpoint-1.json")" \
     -o UserKnownHostsFile="$(jq -r .known_hosts "$AWSS/endpoint-1.json")" \
     "$(jq -r .user "$AWSS/endpoint-1.json")@$(jq -r .host "$AWSS/endpoint-1.json")" \
     'mkdir -p /workspace/checkout && printf aws-marker > /workspace/checkout/aws-marker && sha256sum /workspace/checkout/aws-marker'
   jq '{worker: .worker.id, address: .worker.address}' "$CLOUD_DIR/deployment.json"
   jq . "$CLOUD_DIR/aws.json"
   ```

   Result: The first command prints one SHA-256. Record it as `$MARKER`. The
   record names the instance id and the address. `aws.json` names the instance,
   the workspace volume and the Elastic IP of the cloud, each with the tag
   `ManagedBy=horizon`. Record the volume id as `$VOLUME` and the address
   allocation as `$ADDRESS`.

7. Open a Shell panel on the cloud from the card.

   Result: The panel runs on the worker. `findmnt -no SOURCE /workspace` names
   the same volume as `$VOLUME`.

### 6.9 AWS-L2 — Stop, examine the reported state, and resume

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
   the instance `stopped`. Record the time of each report and the time that the
   card first reads **Stopped**.

   The card reads **Stopped** only after a report shows `Inactive`. A card that
   reads **Stopped** before the provider reports the stop is a finding.

4. Examine the resources that stay.

   ```sh
   jq . "$CLOUD_DIR/aws.json"
   jq '{stage, stop_requested: .stop_requested, worker: .worker}' "$CLOUD_DIR/deployment.json"
   ```

   Result: The instance is `stopped`. The workspace volume `$VOLUME` stays. The
   Elastic IP `$ADDRESS` stays, with the same public address. The record has
   `stop_requested` true and the stage `Stopped`.

> **CAUTION:** THE RESUME RENTS THE INSTANCE AGAIN. The instance bills from
> the start until the next stop or delete.

5. Press **Resume** on the card.

   Result: The card shows **Check readiness**, then **Start sessions**, then
   **Ready**.

6. Make sure that the same instance, volume and address run again.

   ```sh
   jq '{worker: .worker.id, address: .worker.address}' "$CLOUD_DIR/deployment.json"
   cdx endpoint "$CLOUD/settings.json" "$CLOUD_DIR" > "$AWSS/endpoint-2.json"
   ssh -i "$(jq -r .identity_file "$AWSS/endpoint-2.json")" \
     -o UserKnownHostsFile="$(jq -r .known_hosts "$AWSS/endpoint-2.json")" \
     -o StrictHostKeyChecking=yes \
     "$(jq -r .user "$AWSS/endpoint-2.json")@$(jq -r .host "$AWSS/endpoint-2.json")" \
     'sha256sum /workspace/checkout/aws-marker; findmnt -no SOURCE /workspace'
   ```

   Result: The instance id, the address and the volume id are the values of task
   AWS-L1 step 6. The SHA-256 is `$MARKER`. SSH accepts the same host key.

### 6.10 AWS-L3 — Rebuild on a new instance and keep the workspace volume

1. Record the state before the rebuild.

   ```sh
   jq '{worker: .worker.id, address: .worker.address}' "$CLOUD_DIR/deployment.json"
   jq -r .workspace "$CLOUD_DIR/aws.json"
   ```

   Result: Record the instance id as `$INSTANCE`, the address and `$VOLUME`.

2. Change the recipe of the image and commit the change.

   ```sh
   echo 'RUN echo aws-l3 > /etc/aws-l3' >> <repo>/<path of the Dockerfile>
   git -C <repo> commit -qam "test: aws rebuild"
   ```

   Result: `git -C <repo> log -1 --oneline` shows the commit.

> **CAUTION:** THE REBUILD DELETES THE INSTANCE AND RENTS A NEW ONE. The
> processes in the container end. The workspace volume stays.

3. Open the chevron next to **Stop…** on the card and select **Rebuild image &
   restart…**. Read the confirmation, then press its button.

   Result: The confirmation names the instance, the same workspace volume and
   the same address. The card shows **Replace image**, then **Provision
   worker**, **Check readiness** and **Ready**.

4. Make sure that the new instance holds the same volume and the same address.

   ```sh
   jq '{worker: .worker.id, address: .worker.address}' "$CLOUD_DIR/deployment.json"
   jq . "$CLOUD_DIR/aws.json"
   jq . "$CLOUD_DIR/aws-release.json"
   ```

   Result: The instance id is not `$INSTANCE`. The address is the value before
   the rebuild. The workspace volume is `$VOLUME`. `aws-release.json` holds
   `{"held": true}`.

5. Make sure that the workspace and the new image are in place.

   ```sh
   cdx endpoint "$CLOUD/settings.json" "$CLOUD_DIR" > "$AWSS/endpoint-3.json"
   ssh -i "$(jq -r .identity_file "$AWSS/endpoint-3.json")" \
     -o UserKnownHostsFile="$(jq -r .known_hosts "$AWSS/endpoint-3.json")" \
     -o StrictHostKeyChecking=yes \
     "$(jq -r .user "$AWSS/endpoint-3.json")@$(jq -r .host "$AWSS/endpoint-3.json")" \
     'sha256sum /workspace/checkout/aws-marker; cat /etc/aws-l3'
   ```

   Result: The SHA-256 is `$MARKER`. The last line is `aws-l3`.

> **CAUTION:** A VOLUME WITH THIS ATTRIBUTE IS DELETED WHEN THE INSTANCE IS
> DELETED. Put the attribute back directly after the examination in step 8.

6. Make the release unsafe: make the workspace volume go with the instance. Do
   this in the AWS console on the volume `$VOLUME`, and keep the previous value.

   Result: The volume shows that it is deleted with the instance.

7. Request a rebuild again, as in step 3.

   Result: Horizon refuses the rebuild before it deletes the instance. The
   message names the workspace volume. The instance, the address and the volume
   stay. The image is not built again. Record the text.

8. Put the attribute of the volume back.

   Result: The volume shows again that it stays when the instance is deleted.

### 6.11 AWS-L4 — Create, stop, resume and delete from the CLI plan runner

> **CAUTION:** THIS TASK RENTS A SECOND EC2 INSTANCE, A GP3 WORKSPACE VOLUME AND
> AN ELASTIC IP. It bills until step 6 deletes it.

1. Print the plan of the deployment.

   ```sh
   mkdir -p "$AWSS/cli"
   cdx plan-deploy "$CLOUD/settings.json" <repo> aws-build "$AWSS/cli" aws-l4-01
   ```

   Result: The command prints the profile, the region, the instance type, the
   price, the files of the working copy, and a plan token. Record the token.

2. Deploy with that token.

   ```sh
   cdx deploy "$CLOUD/settings.json" <repo> aws-build "$AWSS/cli" aws-l4-01 --plan-token <token>
   ```

   Result: The command prints the stage lines of task AWS-L1 step 3 and ends with
   `Stage: Ready` and `Ready: worker=<instance id>`. Record the instance id.

3. Reach the cloud over SSH and write a marker.

   ```sh
   cdx endpoint "$CLOUD/settings.json" "$AWSS/cli" > "$AWSS/cli-endpoint.json"
   ssh -i "$(jq -r .identity_file "$AWSS/cli-endpoint.json")" \
     -o UserKnownHostsFile="$(jq -r .known_hosts "$AWSS/cli-endpoint.json")" \
     -o StrictHostKeyChecking=yes \
     "$(jq -r .user "$AWSS/cli-endpoint.json")@$(jq -r .host "$AWSS/cli-endpoint.json")" \
     'printf aws-cli-marker > /workspace/checkout/cli-marker && sha256sum /workspace/checkout/cli-marker'
   ```

   Result: SSH answers and prints one SHA-256. Record it as `$CLI_MARKER`.

> **CAUTION:** THE STOP ENDS EVERY PROCESS IN THE CLOUD, AND THE RESUME RENTS
> THE INSTANCE AGAIN. The instance bills from the resume until the delete.

4. Stop the cloud, examine the reported state, then resume it.

   ```sh
   cdx stop "$CLOUD/settings.json" "$AWSS/cli"
   cdx reconcile "$CLOUD/settings.json" "$AWSS/cli"
   cdx resume "$CLOUD/settings.json" "$AWSS/cli"
   cdx reconnect "$CLOUD/settings.json" "$AWSS/cli"
   ```

   Result: `stop` ends with no error. `reconcile` reports `Inactive` and prints
   `Stopped. Resume starts the same worker again.` `resume` asks for the
   reconnect, and `reconnect` ends with `Stage: Ready`. The record names the same
   instance id, and the marker hash is `$CLI_MARKER`.

5. Record the endpoint before the delete.

   ```sh
   cdx endpoint "$CLOUD/settings.json" "$AWSS/cli"
   ```

   Result: The endpoint names the same address as before the stop.

> **CAUTION:** THIS DELETE REMOVES THE INSTANCE, THE WORKSPACE VOLUME AND THE
> ELASTIC IP OF CLOUD `aws-l4-01`. The files on its volume are lost.

6. Request the delete.

   ```sh
   cdx delete "$CLOUD/settings.json" "$AWSS/cli"
   ```

   Result: The command prints the deletion stages and ends with no error.

7. Prove that the resources are gone.

   ```sh
   jq '{stage, operation}' "$AWSS/cli/deployment.json"
   jq . "$AWSS/cli/aws.json"
   cdx delete "$CLOUD/settings.json" "$AWSS/cli"
   ```

   Result: The stage is `Deleted`. `aws.json` shows `deleting`, `swept`, and the
   instance, the volume and the address terminated. The second delete ends at
   once and sends no provider request.

### 6.12 AWS-L5 — Create and delete from the agent tools

> **CAUTION:** THIS TASK RENTS A THIRD EC2 INSTANCE, A GP3 WORKSPACE VOLUME AND
> AN ELASTIC IP. Step 5 deletes them. Do not end the run before it.

> **CAUTION:** THIS STEP WRITES AN AGENT POLICY THAT ALLOWS SPENDING. Copy the
> settings file first, and put the copy back in step 6. A wide policy allows a
> create that the person did not approve.

1. Add an `agent_lifecycle` section to the settings file that allows AWS, the
   test repository and the hourly rate of the profile, and copy the settings
   file first.

   Result: **Menu › Cloud settings** shows the agent policy of AWS as allowed.

2. From the agent panel, call `cloud_offers`.

   Result: The answer has an AWS section with one or more offers with a price in
   US dollars. Record the field names.

3. Call `cloud_create` with `operation submit`, a lowercase UUID
   `operation_id`, the absolute `repository` and `quick_start: true`. Then call
   it again with `operation status` and the same `operation_id`, until the phase
   is not pending.

   Result: The answer reaches `phase: ready`, and it names one cloud ID. An AWS
   cloud appears on the card of the agent's workspace.

4. Call `cloud_list`, then `cloud_exec` with that cloud ID.

   ```sh
   hostname; findmnt -no SOURCE /workspace
   ```

   Result: `cloud_list` names the cloud with its AWS placement and a ready
   state. `cloud_exec` prints the worker identity and the workspace volume.

> **CAUTION:** THIS DELETE REMOVES THE INSTANCE, THE WORKSPACE VOLUME AND THE
> ELASTIC IP OF THE CLOUD THAT THE AGENT MADE.

5. Call `cloud_delete` with the cloud ID and a new lowercase UUID
   `operation_id`.

   Result: The answer reports the delete. `cloud_list` no longer names the
   cloud, or it names it as deleted.

6. Put the copied settings file back.

   Result: The file has the sections that it had before step 1.

### 6.13 AWS-L6 — Examine what a restart preserves

> **CAUTION:** THIS STEP STOPS THE CLOUD. Every process in the cloud ends, and
> an agent session that runs on the worker does not continue.

1. Stop the cloud of task AWS-L1 from its card, and record its state. Do not
   delete it.

   Result: The card reads **Stopped**. Record the cloud ID, the instance id, the
   workspace volume `$VOLUME` and the address `$ADDRESS`.

2. Record the files of the cloud and the sections of the settings file.

   ```sh
   sha256sum "$CLOUD_DIR/aws.json" "$CLOUD_DIR/deployment.json"
   jq 'keys' "$CLOUD/settings.json"
   ls "$CLOUD"
   ```

   Result: Record the two hashes. The settings file holds the `aws` section.

3. Stop the candidate, and start the same frozen binary in the same fixture
   state.

   Result: The fixture prints a new manifest, and the desktop shows the same
   workspace.

4. Compare the workspace and the records with step 1 and step 2.

   ```sh
   sha256sum "$CLOUD_DIR/aws.json" "$CLOUD_DIR/deployment.json"
   ```

   Result: Each cloud card comes back with the same cloud ID. The card reads
   **Stopped**, with the same instance id, the same workspace volume and the same
   address. The two hashes are the values of step 2, so no provider request
   changed a record.

> **CAUTION:** THE RESUME RENTS THE INSTANCE AGAIN. The instance bills from
> the start until the next stop or delete.

5. Press **Resume** on the card.

   Result: The card reaches **Ready** with the same instance id and the same
   address. SSH answers with the same host key and the marker hash is `$MARKER`.

6. Examine the settings and the state of another provider.

   ```sh
   jq 'keys' "$CLOUD/settings.json"
   ls "$CLOUD"
   ```

   Result: The `aws` section is unchanged. The state directory of a cloud of
   another provider keeps its bytes.

### 6.14 AWS-L7 — Delete the cloud and prove the absence with no credential

> **CAUTION:** THIS DELETE REMOVES THE INSTANCE, THE WORKSPACE VOLUME `$VOLUME`
> AND THE ELASTIC IP `$ADDRESS`. The files on the volume, including `$MARKER`,
> are lost.

1. Delete the cloud of task AWS-L1 from its card.

   ```sh
   jq . "$CLOUD_DIR/aws.json"
   ```

   Result: The card reads **Worker deleted**, **Workspace storage cleaned up**
   and **Nothing billing now**.

2. Examine the records of the cloud.

   ```sh
   jq '{stage, operation}' "$CLOUD_DIR/deployment.json"
   jq . "$CLOUD_DIR/aws.json"
   ```

   Result: The stage is `Deleted`. The operation is `terminated` with the
   instance id. `aws.json` shows `deleting`, `swept`, and the instance, the
   workspace volume and the address terminated. The tag sweep is empty.

> **CAUTION:** THIS STEP REMOVES THE ACCESS OF THIS MACHINE TO AWS. Keep one
> copy of the settings file outside the repository, or Horizon cannot reach
> AWS again in this run.

3. Remove the credential from the machine, and prove that the absence proof still
   holds.

   ```sh
   mv "$CLOUD/settings.json" "$AWSS/settings-with-aws.json"
   jq 'del(.aws)' "$AWSS/settings-with-aws.json" > "$CLOUD/settings.json"
   jq 'has("aws")' "$CLOUD/settings.json"
   ```

   Result: The settings file has no `aws` section. It is valid JSON.

4. Open the cloud card of the deleted cloud. Select **Remove cloud**.

   Result: The card goes away. Horizon read the proof from the journals and sent
   no provider request, because the settings hold no `aws` section.

5. Run the headless proofs of the deletion and the removal.

   ```sh
   cargo test -p horizon-core --lib -- cloud_runtime::providers::deletion cloud_runtime::deployment::reconnect
   ```

   Result: All selected tests pass, including the test that reads the deletion
   proof of an `aws.json` with no credential.

6. Put the settings file with the `aws` section back.

   Result: The AWS entry in **New cloud** is ready again.

## 7. Pass criteria

- The baseline build shows no AWS entry and no AWS offer. The candidate shows
  both, and the offers name a price in US dollars.
- Without an `aws` section the dialog names the section, and **Start cloud**
  stays disabled.
- A credential file that others can read, and a secret that AWS rejects, each
  show their own reason, and nothing is rented.
- A GPU profile and a profile that no instance type of the account fits are
  each refused before a request.
- A provider refusal for capacity leaves no running instance and no tagged
  resource of the run.
- A person rents one AWS cloud from **New cloud** without an `aws` command on
  the computer, reaches it over SSH, and writes on the workspace volume.
- The card reads **Stopped** only after a reconcile report shows `Inactive`, and
  the resume starts the same instance on the same workspace volume and the same
  address.
- A rebuild keeps the workspace volume `$VOLUME` and the Elastic IP `$ADDRESS`,
  and the marker keeps its SHA-256.
- A release that would take the workspace is refused before the instance is
  deleted, and no image is built.
- The CLI plan runner and the agent tools create, stop, resume and delete an AWS
  cloud, and `cloud_exec` runs a command on the worker.
- The delete proof holds in `aws.json` when the settings file has no `aws`
  section, and it names the instance, the volume, the address and an empty tag
  sweep.
- A restart keeps each cloud ID, each reported state, each record and the `aws`
  section, and makes no new resource.

## 8. Cleanup

> **CAUTION:** THIS STEP DELETES THE CLOUDS OF THIS RUN, WITH THEIR WORKSPACE
> VOLUMES AND THEIR ELASTIC IPS. Read the cloud ID of each card before you
> delete it.

1. Delete every cloud that this run made and that still runs or bills.

   Result: Each card reads **Nothing billing now**.

2. Examine the instance, volume and address lists of the test account in the AWS
   console. Compare the tag of each resource with the cloud IDs of the run
   before you delete anything.

   Result: The console holds no instance, volume or address of any cloud ID of
   the run.

3. Stop the fixture with `Ctrl+C`.

   Result: The fixture stops its display, its window manager and Horizon.

4. Remove the state directory, the frozen binary, the markers and the credential
   copies of the run.

   ```sh
   rm -rf /tmp/horizon-aws-run "$smoke_bin" "$AWSS"
   ```

   Result: The directories do not exist.

5. Remove the test commits of task AWS-L3 from `<repo>`, if you do not keep
   them.

   ```sh
   git -C <repo> log --oneline -2
   git -C <repo> reset --hard HEAD~1
   ```

   Result: The log shows no commit `test: aws rebuild`.

## 9. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). If not, put the results in the
pull request. Keep private evidence out of the repository.
