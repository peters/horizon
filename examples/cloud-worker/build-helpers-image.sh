#!/usr/bin/env bash
# Builds the worker helper artifact of one public Horizon commit, then checks that a
# minimal contract base with the artifact copied in reports every current marker.
# It pushes nothing.
#   build-helpers-image.sh REVISION TAG
# REVISION is a full 40-character commit of github.com/peters/horizon. The build
# fetches it itself, so it needs no context. The artifact is for the architecture of
# the Docker engine. Set PLATFORM, such as linux/arm64, to build for another
# architecture through emulation.
set -euo pipefail

revision=${1:?full commit}
tag=${2:?image tag}
here=$(cd "$(dirname "$0")" && pwd)
platform=${PLATFORM:-$(docker version --format '{{.Server.Os}}/{{.Server.Arch}}')}

docker build --pull --platform "$platform" --build-arg HORIZON_REVISION="$revision" --target helpers \
    --tag "$tag" - < "$here/Dockerfile.helpers"
# The probe copies the artifact over a contract base exactly as a recipe does, then
# runs its checker; a missing current marker fails the build.
docker build --platform "$platform" --build-arg HORIZON_REVISION="$revision" --target probe \
    --tag horizon-worker-helpers-probe - < "$here/Dockerfile.helpers"
python3 -B "$here/check-markers.py" horizon-worker-helpers-probe
