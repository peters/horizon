#!/bin/sh
# The browser of a browser machine, in the form that Horizon starts.
#
# Horizon looks for a Chromium on the destination, in this order: google-chrome,
# google-chrome-stable, chromium, chromium-browser, brave-browser,
# microsoft-edge. This script is installed as /usr/local/bin/google-chrome, so
# the first lookup finds it. Horizon then starts it with a fixed argument list:
#
#   --remote-debugging-port=0 --remote-debugging-address=127.0.0.1
#   --user-data-dir=<profile folder> --no-startup-window --no-first-run
#   --no-default-browser-check --disable-features=TranslateUI about:blank
#
# This script ignores those arguments except the profile folder. It starts or
# reuses the container of that profile, waits for its published DevTools port to
# answer on the loopback of this machine, and writes that port as the first line
# of <profile folder>/DevToolsActivePort, which is the file that Horizon reads.
#
# It is safe to run again while the container runs: Horizon runs the start
# snippet at every connect that found no answering browser.
#
# Exit codes, as Horizon reads them:
#   0  the endpoint file names a port that answers on the loopback
#   3  this machine cannot start a browser machine (no docker, no image);
#      Horizon reports attached_host_no_browser with the line below
#   5  the container runs but its endpoint did not answer in 20 seconds;
#      Horizon reports attached_host_not_started with the line below
#
# Only POSIX sh: the destination may have dash as /bin/sh and no bash.

set -eu

IMAGE=${HZ_BVM_IMAGE:-horizon-browser-machine:local}
BRIDGE_PORT=9222
READY_STEPS=20
READY_INTERVAL=1
MAX_PROFILE_LEN=40

umask 077

die() {
    exit_code=$1
    shift
    printf 'browser-machine: %s\n' "$*" >&2
    exit "$exit_code"
}

# Read the profile folder out of the arguments of Horizon. Both the `=` form and
# the two-words form are accepted, because the argument list of Horizon is not
# the only caller of this script.
profile_dir=
take_next=0
for argument in "$@"; do
    case $argument in
        --user-data-dir=*)
            profile_dir=${argument#--user-data-dir=}
            take_next=0
            ;;
        --user-data-dir)
            take_next=1
            ;;
        *)
            if [ "$take_next" -eq 1 ]; then
                profile_dir=$argument
                take_next=0
            fi
            ;;
    esac
done

if [ -z "$profile_dir" ]; then
    die 3 "the start names no --user-data-dir, so no profile folder of this machine names the browser machine to use"
fi

profile=${profile_dir##*/}
case $profile in
    '' | .* | *[!a-z0-9-]*)
        die 3 "the profile folder name '$profile' is not one name of a-z, 0-9 and '-'"
        ;;
esac
if [ "${#profile}" -gt "$MAX_PROFILE_LEN" ]; then
    die 3 "the profile folder name '$profile' is longer than $MAX_PROFILE_LEN characters"
fi

# The container name and the container hostname are the same, and the hostname
# stays the same across a container that is replaced on the same profile. A
# changed hostname makes Chromium refuse the profile folder as "in use by
# another computer".
name="hz-bvm-$profile"

command -v docker >/dev/null 2>&1 ||
    die 3 "this machine has no docker program: run setup.sh on it, then use Retry"

docker info >/dev/null 2>&1 ||
    die 3 "docker does not answer for the user $(id -un): run setup.sh on this machine as that user, then use Retry"

# Never pull and never build here: Horizon allows 20 seconds for the whole start.
docker image inspect "$IMAGE" >/dev/null 2>&1 ||
    die 3 "the image $IMAGE is not built on this machine: run setup.sh on it, then use Retry"

mkdir -p "$profile_dir"

# `docker inspect` answers a state for an existing container and fails for a name
# that does not exist yet.
state=$(docker inspect --format '{{.State.Running}}' "$name" 2>/dev/null || true)

start_existing() {
    if ! docker start "$name" >/dev/null 2>&1; then
        die 3 "the container $name did not start: read 'docker logs $name', then use Retry"
    fi
}

if [ "$state" = true ]; then
    :
elif [ "$state" = false ]; then
    start_existing
else
    # A bind mount of the profile folder, so a sign-in survives a restart of the
    # container. The published port stays on the loopback of this machine: the
    # SSH forward of Horizon connects to it, and no other host of the network
    # reaches it.
    if ! docker run --detach \
        --name "$name" \
        --hostname "$name" \
        --volume "$profile_dir:/profile" \
        --shm-size 1g \
        --publish "127.0.0.1::${BRIDGE_PORT}" \
        "$IMAGE" >/dev/null 2>&1; then
        # Two connects of two panels can start at the same time. The loser of
        # the name is the container of the winner, so start that one.
        state=$(docker inspect --format '{{.State.Running}}' "$name" 2>/dev/null || true)
        if [ "$state" = true ]; then
            :
        elif [ "$state" = false ]; then
            start_existing
        else
            die 3 "the container $name did not start: read 'docker logs $name' or run setup.sh, then use Retry"
        fi
    fi
fi

# The published port is the port that Horizon must forward to. It is read from
# Docker and not computed, so a restart of the container and a second profile
# need no table of ports here.
endpoint_answers() {
    url="http://127.0.0.1:$1/json/version"
    if command -v curl >/dev/null 2>&1; then
        curl --fail --silent --show-error --max-time 2 --output /dev/null "$url" >/dev/null 2>&1
    elif command -v wget >/dev/null 2>&1; then
        wget --quiet --timeout=2 --output-document=/dev/null "$url" >/dev/null 2>&1
    else
        # No HTTP client on this machine: ask from inside the container, whose
        # bridge port answers only when Xvfb and Chromium run.
        docker exec "$name" curl --fail --silent --show-error --max-time 2 --output /dev/null \
            "http://127.0.0.1:${BRIDGE_PORT}/json/version" >/dev/null 2>&1
    fi
}

published=
steps=0
while [ "$steps" -lt "$READY_STEPS" ]; do
    mapped=$(docker port "$name" "${BRIDGE_PORT}/tcp" 2>/dev/null | head -n 1 || true)
    if [ -n "$mapped" ]; then
        # "127.0.0.1:49153" for an IPv4 publish. Keep the last colon, so an IPv6
        # publish form still gives the port.
        candidate=${mapped##*:}
        case $candidate in
            '' | *[!0-9]*) candidate= ;;
        esac
        if [ -n "$candidate" ] && endpoint_answers "$candidate"; then
            published=$candidate
            break
        fi
    fi
    steps=$((steps + 1))
    sleep "$READY_INTERVAL"
done

if [ -z "$published" ]; then
    die 5 "the container $name did not answer on its DevTools port within 20 seconds: read 'docker logs $name', then use Retry"
fi

# Horizon reads the first line of this file as the port to forward to, and then
# asks that port for /json/version itself. Chromium of the container writes its
# own port (9223 in the container) into this same file; this line replaces it,
# because the port of this machine is the one that the forward reaches.
active="$profile_dir/DevToolsActivePort"
temporary="$profile_dir/.DevToolsActivePort.new"
printf '%s\n' "$published" >"$temporary"
mv -f "$temporary" "$active"

printf 'browser-machine: profile=%s container=%s port=%s\n' "$profile" "$name" "$published"
exit 0
