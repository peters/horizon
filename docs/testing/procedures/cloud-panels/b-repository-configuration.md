---
procedure: cloud-panels-b-repository-configuration
feature: Cloud panels smoke test, area B (repository configuration)
platforms: [linux]
cost: none
destructive: no
secrets: [registry pull credential for the worker image]
owner: peters
---

# Cloud panels test procedure, area B: repository configuration

## 1. Purpose

This area makes the synthetic repositories for the run. It also makes sure that
**New cloud…** reads `.horizon/cloud.yml`, refuses YAML that is not valid, offers
the setup agent and reads local image-only settings. It makes sure that
`check-markers.py` names the contract markers that an image does not report.

## 2. Applicability

- Candidate: a debug build of `origin/main`.
- Platforms: Linux with Xvfb.
- This area does not test: a deploy. No step in this area rents compute.

## 3. Safety

> **CAUTION:** DO NOT CLICK **Start cloud** IN THIS AREA. This button rents compute
> from the provider.

> **CAUTION:** PUT ONLY TEST CONTENT IN THE SYNTHETIC REPOSITORIES. The worker
> gets the committed source, and screenshots show the file names.

## 4. Equipment and preconditions

- The fixture of [area S](s-test-fixture.md), with a live view.
- The cloud settings of [area A](a-machine-setup.md), with a RunPod key and
  Hetzner turned on.
- These values from the operator. They are not secrets:

  | Name | Meaning |
  |---|---|
  | `<test-owner>` | The GitHub owner of the test account. |
  | `<worker-image>` | A CPU worker image, pinned by digest, that reports all contract markers. |
  | `<gpu-image>` | A GPU worker image, pinned by digest, that reports all contract markers. |
  | `<build-repository>` | A registry repository that the build profile can push to. |

- Docker on the host, for B05.
- The [worker image contract](../../../../examples/cloud-worker/README.md) and
  the [repository setup](../../../cloud-workspaces.md#repository-setup-and-deployment)
  notes.

## 5. Setup

If the host uses rootless Docker, the candidate must know the bound socket. The
bind of S02 alone does not change the socket that the candidate uses.

1. If the host uses rootless Docker, set `docker_host` in the cloud settings file.

   ```sh
   f=<data-home>/.horizon/cloud/settings.json; jq '.docker_host = "unix:///run/user/<uid>/docker.sock"' "$f" > "$f.new" && chmod 600 "$f.new" && mv "$f.new" "$f"
   ```

   Result: The file contains `docker_host` and keeps the mode `0600`.

2. After each later **Save settings**, examine the field again.

   ```sh
   jq -r '.docker_host' <data-home>/.horizon/cloud/settings.json
   ```

   Result: The output is `unix:///run/user/<uid>/docker.sock`.

3. On the host, make the directories of the synthetic repositories.

   ```sh
   mkdir -p <data-home>/smoke/app <data-home>/smoke/lib <data-home>/smoke/sib
   ```

   Result: The fixture shows the directories at `<home>/smoke`.

4. Make a Git repository in `<data-home>/smoke/app` with a README and a small Python test.

   ```sh
   cd <data-home>/smoke/app && git init -q && echo '# Smoke app' > README.md
   printf 'import unittest\nclass T(unittest.TestCase):\n    def test_one(self):\n        self.assertEqual(1, 1)\n' > test_app.py
   git add . && git commit -qm 'Add smoke app'
   ```

   Result: The primary synthetic repository has one commit and no `.horizon` directory.

5. Set the origin of the primary repository to a GitHub URL of the test account.

   ```sh
   git -C <data-home>/smoke/app remote add origin https://github.com/<test-owner>/app.git
   ```

   Result: The origin names the test account. The repository does not need to exist on GitHub.

6. Do steps 4 and 5 again for `lib` and `sib`. Use the names `lib.git` and `sib.git` in the origin.

   Result: Each companion repository has one commit and a GitHub origin.

## 6. Tasks

### 6.1 B01 — Offer the setup agent for a repository without cloud.yml

1. Click **Cloud** in the menu bar.

   Result: The Cloud menu opens.

2. Click **New cloud…**.

   Result: The New cloud dialog opens. Wait 2 seconds before the next click.

3. Type `<home>/smoke/app` as the repository.

   Result: The dialog reads the repository. It finds no `.horizon/cloud.yml`.

4. Expand **No cloud configuration yet?**.

   Result: The section shows **Codex**, **Claude** and **Open setup agent**.

5. Click **Claude**.

   Result: **Open setup agent** is available. Do not click it. The setup agent
   uses the local agent login of the fixture.

6. Click **Cancel**.

   Result: The dialog closes. No panel and no cloud start.

### 6.2 B02 — Read .horizon/cloud.yml with CPU and GPU profiles

1. Write `<data-home>/smoke/app/.horizon/cloud.yml` with this content.

   ```yaml
   version: 1
   default: hetzner-cpu
   profiles:
     hetzner-cpu:
       provider: hetzner
       image: <worker-image>
       min_cpu: 4
       min_memory_gb: 8
       storage:
         volume_gb: 20
       capabilities:
         agents: [codex, claude]
         browsers: [chromium]
         desktop: true
     hetzner-idle:
       provider: hetzner
       image: <worker-image>
       min_cpu: 2
       min_memory_gb: 4
       idle_stop_minutes: 10
       storage:
         volume_gb: 10
     runpod-small:
       provider: runpod
       image: <worker-image>
       min_cpu: 2
       min_memory_gb: 4
       storage:
         volume_gb: 10
     runpod-cpu:
       provider: runpod
       image: <worker-image>
       min_cpu: 4
       min_memory_gb: 8
       storage:
         volume_gb: 20
       capabilities:
         agents: [codex, claude]
     runpod-gpu:
       provider: runpod
       image: <gpu-image>
       gpu: true
       min_cpu: 4
       min_memory_gb: 16
       min_cuda_version: "12.8"
     runpod-build:
       provider: runpod
       image: <build-repository>
       build:
         context: .
         dockerfile: .horizon/Dockerfile
         platform: linux/amd64
       min_cpu: 4
       min_memory_gb: 8
       storage:
         volume_gb: 20
       capabilities:
         agents: [codex, claude]
   companions:
     lib:
       repository: <test-owner>/lib
       profile: hetzner-cpu
     sib:
       repository: <test-owner>/sib
       profile: runpod-build
       placement: same_worker
   ```

   Result: The file declares six profiles, one companion cloud and one
   companion with `placement: same_worker`.

2. Write `<data-home>/smoke/app/.horizon/Dockerfile` with two lines.

   ```dockerfile
   FROM <worker-image>
   RUN echo smoke-build > /etc/smoke-build
   ```

   Result: The build profile has a recipe.

3. Copy `cloud.yml` and `Dockerfile` to `.horizon` in `lib` and `sib`.

   Result: The companion repositories declare the profiles that the primary
   repository names.

4. In `lib` and `sib`, remove the `companions` section from the copy of `cloud.yml`.

   Result: The companion repositories do not declare companions of their own.

5. Commit the `.horizon` directory in `app`, `lib` and `sib`.

   ```sh
   for r in app lib sib; do git -C <data-home>/smoke/$r add .horizon && git -C <data-home>/smoke/$r commit -qm 'Add cloud configuration'; done
   ```

   Result: Each repository has a committed `.horizon` directory.

6. Open **Cloud › New cloud…** and type `<home>/smoke/app` as the repository.

   Result: The dialog reads the repository.

7. Click **Read .horizon/cloud.yml**.

   Result: **Profile** lists `hetzner-cpu`, `hetzner-idle`, `runpod-small`,
   `runpod-cpu`, `runpod-gpu` and `runpod-build`. The default is `hetzner-cpu`.

8. Select `runpod-gpu` in **Profile**.

   Result: The dialog shows GPU workers. Select `hetzner-cpu` again.

### 6.3 B03 — Show an error for YAML that is not valid

1. Add the line `profiles: [` to the end of the local file `app/.horizon/cloud.yml`.

   Result: The local file is not valid YAML. The committed file does not change.

2. In the New cloud dialog, expand **More options**.

   Result: The dialog shows **Committed base revision** and **Use local image-only settings**.

3. Select **Use local image-only settings**.

   Result: The dialog reads the local file, not the committed file.

4. Click **Read .horizon/cloud.yml**.

   Result: The dialog shows `Invalid .horizon/cloud.yml. Check its syntax, default
   profile and supported fields.` **Profile** shows no profile.

5. Restore the committed version of the local file.

   ```sh
   git -C <data-home>/smoke/app checkout -- .horizon/cloud.yml
   ```

   Result: `git status --short` shows no change.

### 6.4 B04 — Offer only image-only profiles in local mode

1. In the local file `app/.horizon/cloud.yml`, change `default: hetzner-cpu` to `default: runpod-build`.

   Result: The local default is a build profile.

2. With **Use local image-only settings** selected, click **Read .horizon/cloud.yml**.

   Result: The dialog refuses the build default with a clear message. It does not
   select another profile.

3. In the local file, change the default back to `hetzner-cpu`.

   Result: The local default is an image-only profile.

4. Click **Read .horizon/cloud.yml**.

   Result: **Profile** lists all profiles except `runpod-build`.

5. Restore the committed version of the local file.

   ```sh
   git -C <data-home>/smoke/app checkout -- .horizon/cloud.yml
   ```

   Result: `git status --short` shows no change.

6. Click **Cancel**.

   Result: The dialog closes. The next dialog uses the committed configuration.

### 6.5 B05 — Name the contract markers that an image does not report

1. In a worktree of the candidate commit, run the marker check for the CPU worker image.

   ```sh
   python3 examples/cloud-worker/check-markers.py <worker-image>
   ```

   Result: The output names no absent marker. If it names a marker, record it.

2. Run the marker check for the GPU worker image.

   ```sh
   python3 examples/cloud-worker/check-markers.py <gpu-image>
   ```

   Result: The output names no absent marker.

3. Run the marker check for an older worker image from the registry.

   ```sh
   python3 examples/cloud-worker/check-markers.py <older-worker-image>
   ```

   Result: The output names each marker that the older image does not report.
   The exit status is not zero.

4. Record the output of each check in the evidence.

   Result: The evidence shows `horizon-tailnet-contract=1` for the images that
   the tailnet tests use.

## 7. Pass criteria

- B01 shows **No cloud configuration yet?** with Codex and Claude.
- B02 lists the six profiles from the committed file.
- B03 shows the error and clears the profiles.
- B04 refuses a build default and offers only profiles without `build`.
- B05 names the absent markers of an older image and none for the test images.

## 8. Cleanup

Keep the synthetic repositories. The other areas use them.

1. Make sure that no synthetic repository has an uncommitted change.

   ```sh
   git -C <data-home>/smoke/app status --short
   ```

   Result: The output is empty for `app`, `lib` and `sib`.

## 9. Record of results

Write the results in the report of the run. Use the
[report template](../../reports/TEMPLATE.md). Keep private evidence out of the
repository.
