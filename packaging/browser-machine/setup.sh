#!/bin/sh
# Prepare a browser machine once: docker, the image, the wrapper, and one
# container that the browser panels of Horizon then reach over the person's SSH.
#
# Run this on the destination, as the user that the person's own SSH login uses
# (on a rented machine usually root, with the machine key of Horizon):
#
#   scp Dockerfile entrypoint.sh horizon-browser-wrapper.sh setup.sh <target>:
#   ssh <target> 'sh setup.sh --profile mail'
#
# It is idempotent: a second run installs nothing twice, builds nothing again
# unless --rebuild is given, and starts the container only when it is stopped.
#
# Usage:
#   sh setup.sh [--profile NAME] [--image IMAGE] [--engine ENGINE] [--horizon PATH]
#               [--rebuild] [--force]
#
# The last line of the output is, for Chromium:
#   browser-machine ready profile=<name> port=<n>
# and for Firefox:
#   browser-machine ready profile=<name> engine=firefox command=<what to write in the settings>

set -eu

IMAGE=horizon-browser-machine:local
PROFILE=default
ENGINE=chromium
HORIZON=
REBUILD=0
FORCE=0
WRAPPER_DIR=/usr/local/bin
WRAPPER_NAME=google-chrome

HERE=$(cd "$(dirname "$0")" && pwd)

log() {
    printf 'setup: %s\n' "$*" >&2
}

die() {
    log "$*"
    exit 1
}

usage() {
    cat >&2 <<'EOF'
usage: sh setup.sh [--profile NAME] [--image IMAGE] [--engine ENGINE] [--horizon PATH]
                   [--rebuild] [--force]

  --profile NAME  profile of the browser machine to prepare (a-z, 0-9, '-')
  --image IMAGE   image tag to build and run (default horizon-browser-machine:local)
  --engine ENGINE chromium (default) or firefox; firefox needs --horizon
  --horizon PATH  the horizon binary of this system and CPU, copied into the image
  --rebuild       build the image again, also when it exists
  --force         continue on a machine that already has a Chromium program
EOF
}

while [ "$#" -gt 0 ]; do
    case $1 in
        --profile)
            [ "$#" -ge 2 ] || die "--profile needs a name"
            PROFILE=$2
            shift 2
            ;;
        --image)
            [ "$#" -ge 2 ] || die "--image needs a tag"
            IMAGE=$2
            shift 2
            ;;
        --engine)
            [ "$#" -ge 2 ] || die "--engine needs an engine"
            ENGINE=$2
            shift 2
            ;;
        --horizon)
            [ "$#" -ge 2 ] || die "--horizon needs a path"
            HORIZON=$2
            shift 2
            ;;
        --rebuild)
            REBUILD=1
            shift
            ;;
        --force)
            FORCE=1
            shift
            ;;
        -h | --help)
            usage
            exit 0
            ;;
        *)
            usage
            die "unknown argument: $1"
            ;;
    esac
done

case $PROFILE in
    '' | .* | *[!a-z0-9-]*) die "the profile name '$PROFILE' is not one name of a-z, 0-9 and '-'" ;;
esac
if [ "${#PROFILE}" -gt 40 ]; then
    die "the profile name '$PROFILE' is longer than 40 characters"
fi

case $ENGINE in
    chromium | firefox) : ;;
    *) die "the engine '$ENGINE' is not one engine of this image: chromium or firefox" ;;
esac
if [ "$ENGINE" = firefox ] && [ -z "$HORIZON" ]; then
    die "the firefox engine runs a driver of Horizon inside this container, so name its binary with --horizon <path>"
fi

# 1. This recipe is Linux only: the image is a Linux image, docker.io is the
#    distribution package of Debian and Ubuntu, and macOS uses its own Chrome.
OS=$(uname -s)
[ "$OS" = Linux ] || die "the browser machine image needs a Linux destination, not $OS"
ARCH=$(uname -m)
case $ARCH in
    x86_64 | aarch64 | arm64) : ;;
    *) log "warning: $ARCH is not a tested architecture of this image" ;;
esac
log "destination $OS $ARCH, docker user $(id -un), home $HOME"

if [ "$(id -u)" -ne 0 ]; then
    log "warning: this script does not run as root; docker and apt-get may need root"
fi

# 2. Docker, from the distribution. Debian's docker.io is the engine only: this
#    recipe needs no Docker Desktop, no compose and no plugin.
if ! command -v docker >/dev/null 2>&1; then
    command -v apt-get >/dev/null 2>&1 ||
        die "this machine has no docker program and no apt-get: install docker, then run this script again"
    [ "$(id -u)" -eq 0 ] ||
        die "this machine has no docker program: run this script as root, or install docker.io with sudo"
    log "installing docker.io from the distribution"
    apt-get update
    DEBIAN_FRONTEND=noninteractive apt-get install --yes --no-install-recommends docker.io
fi

if ! docker info >/dev/null 2>&1; then
    log "the docker engine does not answer yet: starting it"
    if command -v systemctl >/dev/null 2>&1; then
        systemctl enable --now docker >/dev/null 2>&1 || true
        systemctl start docker >/dev/null 2>&1 || true
    else
        service docker start >/dev/null 2>&1 || true
    fi
    sleep 2
fi
docker info >/dev/null 2>&1 ||
    die "the docker engine does not answer: start it with 'systemctl start docker', then run this script again"

# 3. Stop on a machine that already starts a real browser: the wrapper would
#    shadow that program for Horizon, and a real browser is the better browser
#    host. This recipe is for a machine where no browser exists.
shadowed=
for candidate in google-chrome google-chrome-stable chromium chromium-browser brave-browser microsoft-edge; do
    found=$(command -v "$candidate" 2>/dev/null || true)
    if [ -z "$found" ] || [ "$found" = "$WRAPPER_DIR/$WRAPPER_NAME" ]; then
        continue
    fi
    case " $shadowed " in
        *" $found "*) : ;;
        *) shadowed="$shadowed $found" ;;
    esac
done
if [ -n "$shadowed" ]; then
    log "this machine already runs a browser:$shadowed"
    log "the wrapper in $WRAPPER_DIR would shadow it for Horizon, and that browser is the better browser host"
    log "use the remote-browser-tunnel procedure instead, or pass --force to install the wrapper anyway"
    [ "$FORCE" -eq 1 ] || die "this machine needs no browser machine"
fi

# 4. The image, built from the files that sit beside this script and, for the
#    firefox engine, from the horizon binary as the fifth input.
for required in Dockerfile entrypoint.sh horizon-browser-wrapper.sh; do
    [ -f "$HERE/$required" ] ||
        die "$HERE/$required is missing: copy Dockerfile, entrypoint.sh, horizon-browser-wrapper.sh and setup.sh to one folder"
done

# The driver of a Firefox host runs inside this container, so the image carries
# the binary. It matches the system and CPU of the destination, which is where it
# runs; an empty placeholder keeps a Chromium image buildable without one.
if [ -n "$HORIZON" ]; then
    [ -f "$HORIZON" ] || die "no file at $HORIZON"
    [ -x "$HORIZON" ] || die "$HORIZON is not executable"
    install -m 0755 "$HORIZON" "$HERE/horizon"
    log "copied $HORIZON beside the Dockerfile as the driver of this container"
elif [ ! -f "$HERE/horizon" ]; then
    : >"$HERE/horizon"
    chmod 0755 "$HERE/horizon"
fi

existing=$(docker inspect --format '{{.State.Running}}' "hz-bvm-$PROFILE" 2>/dev/null || true)
if docker image inspect "$IMAGE" >/dev/null 2>&1 && [ "$REBUILD" -eq 0 ]; then
    log "the image $IMAGE exists: keep it (use --rebuild to build it again)"
else
    log "building the image $IMAGE from $HERE"
    docker build --tag "$IMAGE" "$HERE"
    if [ -n "$existing" ]; then
        log "the container hz-bvm-$PROFILE runs the earlier image: remove it to use this one"
        log "  docker rm -f hz-bvm-$PROFILE"
    fi
fi
docker image inspect "$IMAGE" >/dev/null 2>&1 || die "the image $IMAGE is not built"

# 5. The wrapper, under the first browser name that Horizon looks for. The
#    firefox engine needs none: its browser runs behind the driver of the
#    container, and the person names that command in the settings.
if [ "$ENGINE" = firefox ]; then
    log "the firefox engine starts no wrapper: the settings of Horizon name the driver command below"
else
mkdir -p "$WRAPPER_DIR"
install -m 0755 "$HERE/horizon-browser-wrapper.sh" "$WRAPPER_DIR/$WRAPPER_NAME"
log "installed $WRAPPER_DIR/$WRAPPER_NAME"

# Horizon runs `command -v google-chrome` in a non-interactive SSH session, so
# the folder must be in that PATH and before any real browser folder.
resolved=$(command -v "$WRAPPER_NAME" 2>/dev/null || true)
if [ "$resolved" != "$WRAPPER_DIR/$WRAPPER_NAME" ]; then
    log "warning: the shell resolves $WRAPPER_NAME to '${resolved:-nothing}', not $WRAPPER_DIR/$WRAPPER_NAME"
    log "warning: a non-interactive SSH session needs $WRAPPER_DIR before every other folder of its PATH"
    log "warning: this shell has PATH=$PATH"
    [ "$FORCE" -eq 1 ] || die "the wrapper is not the first $WRAPPER_NAME on the PATH"
fi
log "the non-interactive PATH must start with $WRAPPER_DIR; this shell has PATH=$PATH"
fi

if [ "$ENGINE" = firefox ]; then
    # 6a. The container of the firefox engine: no display, no Chromium, one
    #     profile mount. The driver runs inside it on demand, with the home of
    #     the person set to the mount, so a profile, its downloads and its page
    #     outlive both the link and the container.
    profile_dir="$HOME/.horizon/browser-profiles/$PROFILE"
    mkdir -p "$profile_dir"
    chmod 0700 "$profile_dir" 2>/dev/null || true
    docker rm -f "hz-bvm-$PROFILE" >/dev/null 2>&1 || true
    log "starting the browser machine of the profile $PROFILE (folder $profile_dir)"
    docker run --detach --name "hz-bvm-$PROFILE" \
        --env "BM_ENGINE=firefox" \
        --env "BM_PROFILE_DIR=/profile" \
        --mount "type=bind,source=$profile_dir,target=/profile" \
        "$IMAGE" >/dev/null
    command_line="docker exec -i -e HOME=/profile hz-bvm-$PROFILE /opt/horizon/horizon --browser-host-driver"
    log "the driver of that container is: $command_line --profile $PROFILE --engine firefox"
    log "write 'browser_host.command' as: $command_line"
    printf 'browser-machine ready profile=%s engine=firefox command=%s\n' "$PROFILE" "$command_line"
    exit 0
fi

# 6. Start the container through the same wrapper that Horizon runs, so the
#    setup proves the start path and not a second copy of it.
profile_dir="$HOME/.horizon/browser-profiles/$PROFILE"
mkdir -p "$profile_dir"
chmod 0700 "$profile_dir" 2>/dev/null || true
log "starting the browser machine of the profile $PROFILE (folder $profile_dir)"
if ! "$WRAPPER_DIR/$WRAPPER_NAME" \
    --remote-debugging-port=0 \
    --remote-debugging-address=127.0.0.1 \
    --user-data-dir="$profile_dir" \
    --no-startup-window \
    about:blank >>"$profile_dir/setup.log" 2>&1; then
    log "read $profile_dir/setup.log and 'docker logs hz-bvm-$PROFILE'"
    die "the browser machine did not start"
fi

# 7. Read back what Horizon reads.
active="$profile_dir/DevToolsActivePort"
[ -s "$active" ] || die "the wrapper wrote no $active"
port=$(sed -n '1p' "$active")
case $port in
    '' | *[!0-9]*) die "the port in $active is not a number: $port" ;;
esac

log "the DevTools endpoint answers on the loopback: curl -s http://127.0.0.1:$port/json/version"
log "the person's SSH login must be $(id -un), or the profile folder is another one"
log "the container is hz-bvm-$PROFILE, the image is $IMAGE, the profile is $PROFILE"

printf 'browser-machine ready profile=%s port=%s\n' "$PROFILE" "$port"
