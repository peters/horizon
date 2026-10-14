---
procedure: worker-images-multiarch
feature: Worker images for linux/amd64 and linux/arm64
platforms: [linux, macos]
cost: none
destructive: no
secrets: none
owner: peters
---

# Worker images for two architectures test procedure

## 1. Purpose

This procedure proves that the Worker images workflow builds, checks and publishes
the helper artifact and the CPU base worker image for `linux/amd64` and
`linux/arm64`. It also proves that a Docker engine on each architecture pulls the
image of its own architecture.

## 2. Applicability

- Candidate: a pull request or a main commit that changes
  `examples/cloud-worker/` or `.github/workflows/worker-images.yml`.
- Platforms: GitHub-hosted runners `ubuntu-latest` and `ubuntu-24.04-arm`. For task
  W03, a Linux amd64 computer and an arm64 computer with Docker, for example Docker
  Desktop on Apple silicon.
- This procedure does not test: a cloud deployment on an ARM host. The quick-start
  pin and the Docker host provider have their own procedures
  ([cloud quick start](cloud-quick-start.md), [Docker host probe](docker-host-probe.md)).

## 3. Safety

> **CAUTION:** DO NOT PUSH A WORKER IMAGE FROM A LOCAL COMPUTER.
> Only the Worker images workflow on main publishes images. A local push can
> replace a public tag with an image that the workflow did not check.

## 4. Equipment and preconditions

- A GitHub login that can read the workflow runs of `peters/horizon`.
- Docker 25 or later with `docker buildx`.
- An empty Docker configuration directory for anonymous pulls, for example
  `export DOCKER_CONFIG=$(mktemp -d)`.

## 5. Setup

1. Find the pull request or the main commit under test.

   Result: You have its head commit.

## 6. Tasks

### 6.1 W01 — Pull request builds both architectures

1. Open the **Worker images** run of the pull request head.

   Result: The jobs **Build and check (amd64)** and **Build and check (arm64)** ran.
   Each job ran on a runner of its architecture.

2. In the log of each job, find `The image reports all` and
   `No image layer contains SSH host keys.`

   Result: Both lines are in each log. Each job passed.

3. In each job, examine the step **Render a page in Chromium**.

   Result: The step passed. Chromium of that architecture rendered the test page.

4. Examine the job **Compare worker check reports**.

   Result: The job passed. The two architectures report the same markers.

### 6.2 W02 — Main publishes one index for each image

1. Open the newest successful **Worker images** run on main.

   Result: The jobs for each architecture passed, then
   **Publish worker helpers index** and **Publish CPU base worker index** passed.

2. Copy the image reference with `@sha256:` from the summary of
   **Publish CPU base worker index**.
3. Do this command with that reference:

   ```
   docker buildx imagetools inspect <reference>
   ```

   Result: The media type is an image index. The index has `linux/amd64` and
   `linux/arm64`.

4. Do the same for the helper reference in the summary of
   **Publish worker helpers index**.

   Result: The index has `linux/amd64` and `linux/arm64`.

### 6.3 W03 — Each engine pulls its own architecture

1. On the amd64 computer, do these commands with the base image reference:

   ```
   docker pull <reference>
   docker image inspect --format '{{.Architecture}}' <reference>
   ```

   Result: The output is `amd64`.

2. On the arm64 computer, do the same commands.

   Result: The output is `arm64`. Docker shows no warning about a platform that
   does not agree with the host.

3. On the arm64 computer, do this command:

   ```
   docker run --rm --network=none --entrypoint /usr/local/bin/horizon-worker-check \
     --env 'HORIZON_WORKER_CAPABILITIES={"agents":["claude","codex"],"browsers":["chromium"],"desktop":true}' \
     <reference> --git-auth
   ```

   Result: The command exits with status 0. It prints the same markers as the
   summary of **Publish CPU base worker index**.

## 7. Pass criteria

- W01 and W02 pass.
- W03 passes on both architectures.
- No image tag moved in a run where a check failed.

## 8. Cleanup

1. Remove the pulled images:

   ```
   docker image rm <reference>
   ```

   Result: Docker removes the image.

2. Remove the temporary Docker configuration directory.

   Result: No login or temporary file stays.

## 9. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). If not, put the results in the
pull request. Keep private evidence out of the repository.
