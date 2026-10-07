#!/usr/bin/env bash
# Builds the public CPU base worker image from the generic recipe and the published
# helper artifact, then runs the worker contract checks. It pushes nothing.
#   build-base-image.sh HELPERS_IMAGE TAG
# HELPERS_IMAGE is a helper artifact reference, such as
# ghcr.io/peters/horizon-worker-helpers@sha256:<digest> or a local tag.
set -euo pipefail

helpers=${1:?helper artifact image}
tag=${2:?image tag}
here=$(cd "$(dirname "$0")" && pwd)
# The capabilities of the built-in quick-start profile in
# crates/horizon-core/src/cloud_runtime/repository/launch/quick_start.rs; a test
# there keeps the two in step.
agents=claude,codex
browsers=chromium
desktop=true

work=$(mktemp -d)
container=
cleanup() {
    if [ -n "$container" ]; then
        docker rm -f "$container" > /dev/null 2>&1 || true
    fi
    rm -rf "$work"
}
trap cleanup EXIT

# The artifact is FROM scratch, so the container is only created to copy files out.
container=$(docker create --platform linux/amd64 "$helpers" /usr/local/bin/horizon-cloud-worker)
mkdir "$work/bin"
for helper in horizon-cloud-worker horizon-browser horizon-device; do
    docker cp "$container:/usr/local/bin/$helper" "$work/bin/$helper"
done
revision=$(docker image inspect --format '{{ index .Config.Labels "org.opencontainers.image.revision" }}' "$helpers")
python3 -B "$here/prepare-context.py" --bin-dir "$work/bin" --output "$work/context" > /dev/null

docker build --pull --platform linux/amd64 \
    --build-arg HORIZON_AGENTS="$agents" \
    --build-arg HORIZON_BROWSERS="$browsers" \
    --build-arg HORIZON_DESKTOP="$desktop" \
    --label org.opencontainers.image.source=https://github.com/peters/horizon \
    --label org.opencontainers.image.revision="$revision" \
    --label org.opencontainers.image.description="Horizon public CPU base worker for repositories without their own image" \
    --tag "$tag" "$work/context"

# The full selection the quick-start profile requests. Horizon keeps this check's report
# beside the pin (quick_start::CONTRACT) instead of a local check before allocation, so
# the report is printed with the image it belongs to.
selection=$(python3 -c 'import json, sys; print(json.dumps({"agents": sys.argv[1].split(","), "browsers": sys.argv[2].split(","), "desktop": sys.argv[3] == "true"}, separators=(",", ":")))' "$agents" "$browsers" "$desktop")
echo "Worker check report for the quick-start capabilities:"
docker run --rm --network=none --entrypoint /usr/local/bin/horizon-worker-check \
    --env HORIZON_WORKER_CAPABILITIES="$selection" "$tag" --git-auth
echo "End of the worker check report."
python3 -B "$here/check-markers.py" "$tag"
# A public image must not carry SSH host keys in any layer; each worker makes its own.
python3 -B "$here/check-host-keys.py" "$tag"
