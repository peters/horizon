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
# The capabilities the quick-start profile requests; keep them in step with
# the quick-start profile in
# crates/horizon-core/src/cloud_runtime/repository/launch/quick_start.rs.
agents=${HORIZON_AGENTS-claude,codex}
browsers=${HORIZON_BROWSERS-chromium}
desktop=${HORIZON_DESKTOP-true}

work=$(mktemp -d)
container=
cleanup() {
    [ -n "$container" ] && docker rm -f "$container" > /dev/null 2>&1
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

selection=$(python3 -c 'import json, sys; print(json.dumps({"agents": [a for a in sys.argv[1].split(",") if a], "browsers": [b for b in sys.argv[2].split(",") if b], "desktop": sys.argv[3] == "true"}, separators=(",", ":")))' "$agents" "$browsers" "$desktop")
# The full selection the quick-start profile requests, as Horizon checks it before allocation.
docker run --rm --network=none --entrypoint /usr/local/bin/horizon-worker-check \
    --env HORIZON_WORKER_CAPABILITIES="$selection" "$tag" --git-auth > /dev/null
python3 -B "$here/check-markers.py" "$tag"
# A public image must not carry SSH host keys; each worker creates its own.
if docker run --rm --network=none --entrypoint /bin/sh "$tag" -c 'ls /etc/ssh/ssh_host_* 2> /dev/null' | grep -q .; then
    echo 'The image contains SSH host keys under /etc/ssh' >&2
    exit 1
fi
