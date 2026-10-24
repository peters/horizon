---
procedure: azure-described-provider
feature: Azure as a described provider that Horizon cannot create, manage or delete
platforms: [linux]
cost: none
destructive: no
secrets: synthetic fixtures only
owner: peters
---

# Azure described provider test procedure

## 1. Purpose

This procedure makes sure that Horizon describes Azure as a provider.
It also makes sure that each path that can create, stop, resume or delete an Azure cloud refuses.

## 2. Applicability

- Candidate: the first pull request of #1774, which adds Azure as a described provider.
- Platform: Linux with the repository Rust toolchain.
  The deployment record test and the CLI tasks use Unix paths and a POSIX shell.
- These tasks use synthetic inputs only. They need no network, no provider account and no Azure credential.
- No task makes an Azure resource, and no task needs a live Azure resource.
- This procedure does not test these items. Later pull requests of #1774 add them:
  - The provisioning of an Azure worker.
  - The stop, resume and delete of a live Azure worker.
  - Azure prices, offers and the VM size catalog.
  - Azure in the **New cloud** dialog.
- This procedure does not run the MCP tools live. Section 6.4 gives the shared code path that they use.

## 3. Safety

> **CAUTION:** REMOVE ONLY THE FIXTURE DIRECTORY THAT THIS RUN MADE. If the
> variable is empty or wrong, the cleanup command can remove other files.

## 4. Equipment and preconditions

- A checkout of the candidate commit.
- The repository's supported Rust toolchain, Git and Python 3.
- A POSIX shell.
- No Azure account, Azure credential or provider key.

## 5. Setup

1. Open a terminal in the candidate checkout.

   Result: `Cargo.toml` names the Horizon workspace.

2. Make a fixture directory outside the checkout.

   ```sh
   export AZURE_FIXTURE="$(mktemp -d)"
   ```

   Result: `$AZURE_FIXTURE` is an empty directory.

## 6. Tasks

Do AZ-01 through AZ-05 in order. Each task has a stable ID for the report.

### 6.1 AZ-01 — Check the described provider

1. Run the provider and profile tests.

   ```sh
   cargo test -p horizon-cloud -- provider:: profile::
   ```

   Result: All selected tests pass. These tests examine the items that follow:
   - `by_id("azure")` gives the Azure description. Its `creatable` value is `false`.
   - Azure places a worker by location. It gives prices by server type, in USD, and not net of VAT.
   - A stopped Azure worker is kept. Azure has no GPU and no hosted devices.
   - A profile with `provider: azure` gives the Azure description, not the RunPod description.
   - A CPU profile with `provider: azure` parses and validates.
   - An Azure profile with `gpu: true` or `browserstack` fails validation.

### 6.2 AZ-02 — Check the refusals at the provider boundary

1. Run the provider and deployment tests.

   ```sh
   cargo test -p horizon-core --lib -- cloud_runtime::providers cloud_runtime::deployment
   ```

   Result: All selected tests pass. These tests examine the items that follow:
   - `preflight` refuses an Azure cloud with `Horizon cannot create clouds on Azure yet`.
   - `compute` refuses with `Horizon has no Azure deployment client yet`.
   - `cpu_flavors` and `data_centers` refuse, also when RunPod settings are present.
   - `check`, `stop`, `resume`, `release_devices` and `delete` refuse with `Horizon cannot manage Azure clouds yet`.
   - These lifecycle refusals call no observer and send no event. The record does not change.
   - A reconnect refuses an Azure record and does not change `deployment.json`.

### 6.3 AZ-03 — Check that a prepared Azure cloud writes no record

1. Run the deployment record test.

   ```sh
   cargo test -p horizon-core --lib -- --exact cloud_runtime::deployment::tests::an_azure_cloud_is_refused_before_a_deployment_record_is_written
   ```

   Result: One test passes.
   `deployment::prepare`, `prepare_image_record` and `deploy` each give `Err` with `Horizon cannot create clouds on Azure yet`.
   The state directory contains no `deployment.json`.

### 6.4 AZ-04 — Check that no CLI or MCP entry point starts an Azure deployment

The CLI is the example `cloud_deploy`. It uses the same deployment coordinator as the UI.
The **New cloud** dialog, `cloud_deploy` and the MCP tool `cloud_companion` read a profile through `repository::launch`.
That function removes each profile whose provider is not creatable, so it removes each Azure profile.
A caller sees one of these refusals:

| Entry point | Refusal |
|---|---|
| `cloud_deploy plan-deploy`, `deploy` or `prepare-image`, when all profiles are Azure profiles | `No profile in this repository's .horizon/cloud.yml can run on a provider Horizon creates clouds on.` |
| The same commands with an Azure profile beside a creatable profile | `Profile does not exist` |
| `cloud_companion` with `ensure_ready` for a new companion cloud on an Azure profile | `The checkout's committed .horizon/cloud.yml has no creatable profile with the companion's profile name` |
| `cloud_deploy stop`, `resume`, `reconnect` or `delete` on an Azure record | `Horizon cannot manage Azure clouds yet` |
| `cloud_companion` on a companion cloud with an Azure record | `Horizon cannot manage Azure clouds yet` |
| `cloud_deploy plan-deploy` or `deploy` with `--worker-choice` and an Azure offer | `Worker choice is not currently offered under this machine's policy and repository requirements` |

1. Build the CLI.

   ```sh
   cargo build -p horizon-core --example cloud_deploy
   ```

   Result: The build succeeds.

2. Record the path of the CLI.

   ```sh
   export CLOUD_DEPLOY="$(cargo metadata --format-version 1 --no-deps | python3 -c 'import json, sys; print(json.load(sys.stdin)["target_directory"])')/debug/examples/cloud_deploy"
   ```

   Result: `$CLOUD_DEPLOY` is the path of an executable file.

3. Write the synthetic machine settings.

   ```sh
   mkdir -p "$AZURE_FIXTURE/home" "$AZURE_FIXTURE/docker"
   touch "$AZURE_FIXTURE/runpod-key" "$AZURE_FIXTURE/identity"
   cat > "$AZURE_FIXTURE/settings.json" <<EOF
   {"runpod_key_file":"$AZURE_FIXTURE/runpod-key","ssh_identity_file":"$AZURE_FIXTURE/identity","docker_config":"$AZURE_FIXTURE/docker","registry_pull_auth_id":null,"cpu_flavors":[],"gpu_types":[]}
   EOF
   ```

   Result: The settings contain empty files and no secret.

4. Make a synthetic repository with one Azure profile.

   ```sh
   git init -q "$AZURE_FIXTURE/repository"
   mkdir "$AZURE_FIXTURE/repository/.horizon"
   cat > "$AZURE_FIXTURE/repository/.horizon/cloud.yml" <<'EOF'
   version: 1
   default: azure-cpu
   profiles:
     azure-cpu:
       provider: azure
       image: registry.example.com/horizon/dev:tested
       min_cpu: 4
       min_memory_gb: 8
       gpu: false
       capabilities:
         agents: [claude]
   EOF
   git -C "$AZURE_FIXTURE/repository" add .horizon/cloud.yml
   git -C "$AZURE_FIXTURE/repository" -c user.name=fixture -c user.email=fixture@example.com commit -qm azure
   ```

   Result: The synthetic repository has one commit.

5. Run each launch command on the Azure profile.

   ```sh
   for command in plan-deploy deploy prepare-image; do
     HOME="$AZURE_FIXTURE/home" "$CLOUD_DEPLOY" "$command" "$AZURE_FIXTURE/settings.json" \
       "$AZURE_FIXTURE/repository" azure-cpu "$AZURE_FIXTURE/state" azure-fixture
     echo "exit=$?"
   done
   ```

   Result: Each command prints `No profile in this repository's .horizon/cloud.yml can run on a provider Horizon creates clouds on.`
   Each command shows `exit=1`.

6. Add a creatable RunPod profile as the default.

   ```sh
   cat > "$AZURE_FIXTURE/repository/.horizon/cloud.yml" <<'EOF'
   version: 1
   default: runpod-cpu
   profiles:
     runpod-cpu:
       provider: runpod
       image: registry.example.com/horizon/dev:tested
       min_cpu: 4
       min_memory_gb: 8
       gpu: false
       capabilities:
         agents: [claude]
     azure-cpu:
       provider: azure
       image: registry.example.com/horizon/dev:tested
       min_cpu: 4
       min_memory_gb: 8
       gpu: false
       capabilities:
         agents: [claude]
   EOF
   git -C "$AZURE_FIXTURE/repository" -c user.name=fixture -c user.email=fixture@example.com commit -qam mixed
   ```

   Result: The synthetic repository has two commits.

7. Run each launch command on the Azure profile again.

   ```sh
   for command in plan-deploy deploy prepare-image; do
     HOME="$AZURE_FIXTURE/home" "$CLOUD_DEPLOY" "$command" "$AZURE_FIXTURE/settings.json" \
       "$AZURE_FIXTURE/repository" azure-cpu "$AZURE_FIXTURE/state" azure-fixture
     echo "exit=$?"
   done
   ```

   Result: Each command prints `Profile does not exist` and shows `exit=1`.

8. Make sure that no launch command wrote a state directory.

   ```sh
   test ! -e "$AZURE_FIXTURE/state" && echo "no state"
   ```

   Result: The command prints `no state`.

9. Write a synthetic Azure deployment record with a requested worker.

   ```sh
   mkdir -m 700 "$AZURE_FIXTURE/record"
   python3 - "$AZURE_FIXTURE/record/deployment.json" <<'EOF'
   import json, sys
   profile = {"provider": "azure", "image": "registry.example/worker", "cpu": 4, "memory_gb": 8}
   spec = {"operation_id": "azure-fixture", "image_digest": "registry.example/worker@sha256:" + "a" * 64,
           "profile": profile, "public_key": "unused", "registry_auth_id": None, "gpu_types": [],
           "cpu_flavors": [], "data_centers": []}
   record = {"version": 1, "cloud_id": "azure-fixture", "repository": "/synthetic/not-mounted",
             "revision": "a" * 40, "profile": profile, "stage": "Ready",
             "operation": {"state": "bound", "worker_id": "worker1"}, "spec": spec, "worker": None,
             "sessions": [], "ready_history": "Observed"}
   json.dump(record, open(sys.argv[1], "w"))
   EOF
   cp "$AZURE_FIXTURE/record/deployment.json" "$AZURE_FIXTURE/saved.json"
   ```

   Result: `$AZURE_FIXTURE/record/deployment.json` and a copy in `$AZURE_FIXTURE/saved.json` exist.

10. Run each lifecycle command on the Azure record.

    ```sh
    for command in stop resume reconnect delete; do
      HOME="$AZURE_FIXTURE/home" "$CLOUD_DEPLOY" "$command" "$AZURE_FIXTURE/settings.json" "$AZURE_FIXTURE/record"
      echo "exit=$?"
    done
    ```

    Result: Each command prints `Horizon cannot manage Azure clouds yet` and shows `exit=1`.

11. Compare the record with its copy.

    ```sh
    cmp "$AZURE_FIXTURE/record/deployment.json" "$AZURE_FIXTURE/saved.json" && echo "unchanged"
    ```

    Result: The command prints `unchanged`. The `delete` command did not mark the record as deleted.

12. Run the worker choice test.

    ```sh
    cargo test -p horizon-core --lib -- cloud_panel::selection
    ```

    Result: All selected tests pass.
    `an_azure_worker_cannot_be_chosen` gives `This worker cannot run the repository profile` for an Azure worker choice.
    No command reaches that guard yet: the worker-choice commands stop before it, because no offer catalog lists Azure.

### 6.5 AZ-05 — Check the deletion rule

1. Run the deletion proof and retained resource tests.

   ```sh
   cargo test -p horizon-core --lib -- cloud_runtime::tailnet::cleanup::tests::deletion_proof cloud_runtime::providers::tests::an_azure_record_holds_no_provider_resources
   ```

   Result: All selected tests pass. These tests examine the items that follow:
   - A deleted Azure record without a provider journal is not confirmed deleted.
   - A settled journal of another provider does not confirm the deletion of an Azure record.
   - Azure is in the loop of foreign provider records that do not confirm absence.
   - `retained` gives `false` for an Azure record, so the record does not block **Remove**.

## 7. Pass criteria

- All tasks pass on the candidate commit.
- No path writes `deployment.json` for a new Azure cloud.
- Each launch and lifecycle command refuses with the text in the table of AZ-04.
- No Azure record is confirmed deleted.
- No task uses the network, a provider account or a credential.

## 8. Cleanup

1. Remove the fixture directory.

   ```sh
   rm -rf -- "${AZURE_FIXTURE:?}"
   ```

   Result: The synthetic repository, settings and record are removed.

## 9. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). If not, put the results in the
pull request. Keep private evidence out of the repository.
