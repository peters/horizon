#!/usr/bin/env bash
# Run the browser image of packaging/browser-vm on this computer, so that Horizon
# reaches its profile host over SSH without a rented machine. README.md section 9
# gives the commands and the limits.
#
# Usage: browser-vm-local.sh <up|down|status|alias|key> [options]
#
#   up        Build the image when it is absent, then start the container.
#   down      Stop and remove the container. The state volume stays.
#   status    Print the container, the ports, the state volume and the SSH target.
#   alias     Print the SSH config block of the container.
#   key       Print the public key that the container accepts.
#
# Options:
#   --name <name>        Container and volume name (default horizon-browser-vm-local).
#   --image <reference>  Image reference (default horizon-browser-vm-local:latest).
#   --vnc-port <port>    Loopback port of the VNC server (default 5900).
#   --ssh-port <port>    Loopback port of the SSH server (default 2222).
#   --state <directory>  Directory of the key, the alias block and the records
#                        (default ${XDG_STATE_HOME:-~/.local/state}/horizon/browser-vm-local).
#   --no-build           Do not build the image when it is absent.
#   --write-alias        Append the alias block to ~/.ssh/config, once.
#   --docker <command>   Docker command (default docker).
#
# Every port is published on the loopback of this computer only. The SSH server
# accepts one key, and the script writes that key on the first `up`.
set -euo pipefail

readonly DEFAULT_STATE="${XDG_STATE_HOME:-${HOME}/.local/state}/horizon/browser-vm-local"
# The markers carry the container name: a second name adds a second block, and a
# rewrite removes only its own block.
alias_begin() { printf '# >>> horizon-browser-vm-local:%s >>>' "$name"; }
alias_end() { printf '# <<< horizon-browser-vm-local:%s <<<' "$name"; }
readonly SSHD_CONTAINER_PORT=2222
readonly VNC_CONTAINER_PORT=5900

command_name=""
name="horizon-browser-vm-local"
image="horizon-browser-vm-local:latest"
vnc_port=5900
ssh_port=2222
state_dir="$DEFAULT_STATE"
build_image=1
write_alias=0
docker_command="docker"

usage() {
    awk 'NR > 1 && /^#/ { sub(/^# ?/, ""); print; next } NR > 1 { exit }' \
        "${BASH_SOURCE[0]}" >&2
    exit 2
}

fail() {
    printf 'browser-vm-local: %s\n' "$*" >&2
    exit 1
}

log() {
    printf 'browser-vm-local: %s\n' "$*" >&2
}

while [[ "$#" -gt 0 ]]; do
    case "$1" in
        up|down|status|alias|key)
            [[ -z "$command_name" ]] || usage
            command_name="$1"
            ;;
        --name) name="${2:?--name needs a value}"; shift ;;
        --image) image="${2:?--image needs a value}"; shift ;;
        --vnc-port) vnc_port="${2:?--vnc-port needs a value}"; shift ;;
        --ssh-port) ssh_port="${2:?--ssh-port needs a value}"; shift ;;
        --state) state_dir="${2:?--state needs a value}"; shift ;;
        --docker) docker_command="${2:?--docker needs a value}"; shift ;;
        --no-build) build_image=0 ;;
        --write-alias) write_alias=1 ;;
        -h|--help) usage ;;
        *) printf 'browser-vm-local: unknown argument %s\n' "$1" >&2; usage ;;
    esac
    shift
done

[[ -n "$command_name" ]] || usage
for port in "$vnc_port" "$ssh_port"; do
    [[ "$port" =~ ^[0-9]+$ ]] && ((port >= 1 && port <= 65535)) \
        || fail "port $port is not a number between 1 and 65535"
done
[[ "$vnc_port" != "$ssh_port" ]] || fail "the VNC port and the SSH port must differ"

readonly volume="${name}-state"
readonly ssh_dir="${state_dir}/ssh"
readonly key_file="${ssh_dir}/id_ed25519"
# Each name has its own host file: a new volume brings a new host key, and ssh
# refuses a changed key for a known host.
readonly known_hosts="${ssh_dir}/known_hosts-${name}"

# The repository root holds the recipe. The script is called from any directory.
repo_root() {
    local here
    here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
    cd "${here}/.." && pwd
}

# The key and the alias carry the loopback ports, so a person keeps one key per
# name and port pair and the known-host entries stay valid.
prepare_state() {
    mkdir -p "$ssh_dir"
    chmod 0700 "$state_dir" "$ssh_dir"
    if [[ ! -f "$key_file" ]]; then
        ssh-keygen -q -t ed25519 -N '' -C "horizon-browser-vm-local:${name}" -f "$key_file"
        log "wrote the key to $key_file"
    fi
    chmod 0600 "$key_file"
}

alias_block() {
    cat <<EOF
$(alias_begin)
Host ${name}
  HostName 127.0.0.1
  Port ${ssh_port}
  User root
  IdentityFile ${key_file}
  IdentitiesOnly yes
  IdentityAgent none
  UserKnownHostsFile ${known_hosts}
  StrictHostKeyChecking yes
$(alias_end)
EOF
}

container_exists() {
    "$docker_command" container inspect "$name" >/dev/null 2>&1
}

container_running() {
    [[ "$("$docker_command" container inspect --format '{{.State.Running}}' "$name" 2>/dev/null)" == "true" ]]
}

image_exists() {
    "$docker_command" image inspect "$image" >/dev/null 2>&1
}

wait_for_port() {
    local port="$1"
    for _ in {1..150}; do
        if ! container_running; then
            return 1
        fi
        if (exec 3<>"/dev/tcp/127.0.0.1/${port}") 2>/dev/null; then
            exec 3>&- 3<&-
            return 0
        fi
        sleep 0.2
    done
    return 1
}

# The host key comes from the container that this run started, read through
# Docker and not through the network. A recreated state volume brings a new host
# key, and the pin replaces the earlier entry of the same loopback address, so a
# start never fails on a key that belongs to a container that is gone.
pin_host_key() {
    local host_key=""
    # Docker's proxy makes the port answer before the entrypoint wrote the host
    # key, so the read waits for the file and not only for the port.
    for _ in {1..60}; do
        host_key="$("$docker_command" exec "$name" \
            cat /state/ssh/ssh_host_ed25519_key.pub 2>/dev/null \
            | awk 'NF >= 2 { print $1" "$2 }')" || true
        [[ -n "$host_key" ]] && break
        container_running || break
        sleep 0.5
    done
    if [[ -z "$host_key" ]]; then
        fail "the container $name has no SSH host key; read docker logs $name"
    fi
    local entry="[127.0.0.1]:${ssh_port} ${host_key}"
    local staged
    staged="$(mktemp "${TMPDIR:-/tmp}/horizon-known-hosts.XXXXXX")"
    if [[ -f "$known_hosts" ]]; then
        grep -v -F "[127.0.0.1]:${ssh_port} " "$known_hosts" >"$staged" || true
    fi
    printf '%s\n' "$entry" >>"$staged"
    cat "$staged" >"$known_hosts"
    rm -f "$staged"
    chmod 0600 "$known_hosts"
}

# Docker's proxy answers a published port whether or not the container listens
# behind it, so a connect alone proves nothing. The SSH server must accept the
# key, and the VNC server must greet with its protocol.
prove_ssh() {
    local attempts="${1:-30}"
    for _ in $(seq 1 "$attempts"); do
        # A running agent offers its keys first, and the server refuses the login
        # after six of them, so the check names its own key and no agent.
        if ssh -T -o BatchMode=yes -o ConnectTimeout=5 \
            -o IdentitiesOnly=yes -o IdentityAgent=none \
            -o PreferredAuthentications=publickey -o PasswordAuthentication=no \
            -o StrictHostKeyChecking=yes -o "UserKnownHostsFile=${known_hosts}" \
            -i "$key_file" -p "$ssh_port" root@127.0.0.1 true 2>/dev/null; then
            return 0
        fi
        container_running || return 1
        sleep 1
    done
    return 1
}

prove_vnc() {
    local banner=""
    for _ in {1..30}; do
        banner="$(timeout 5 bash -c "exec 3<>/dev/tcp/127.0.0.1/${vnc_port}; head -c 4 <&3" 2>/dev/null || true)"
        if [[ "$banner" == "RFB " ]]; then
            return 0
        fi
        container_running || return 1
        sleep 1
    done
    return 1
}

start() {
    prepare_state
    if container_exists; then
        fail "the container $name exists; run 'down' first"
    fi
    if ! image_exists; then
        if [[ "$build_image" == 0 ]]; then
            fail "the image $image is absent; build it or drop --no-build"
        fi
        log "build the image $image from $(repo_root)"
        (cd "$(repo_root)" && "$docker_command" build \
            --file packaging/browser-vm/Dockerfile \
            --target browser-vm-local \
            --tag "$image" .)
    fi
    local public_key
    public_key="$(cat "${key_file}.pub")"
    "$docker_command" run --detach --name "$name" \
        --volume "${volume}:/state" \
        --publish "127.0.0.1:${vnc_port}:${VNC_CONTAINER_PORT}" \
        --publish "127.0.0.1:${ssh_port}:${SSHD_CONTAINER_PORT}" \
        --shm-size 1g \
        --env HORIZON_BROWSER_VM_SSH=1 \
        --env "HORIZON_BROWSER_VM_SSH_PUBKEY=${public_key}" \
        "$image" >/dev/null
    log "the container $name runs"
    wait_for_port "$ssh_port" || fail "the SSH server of $name did not answer on 127.0.0.1:${ssh_port}"
    pin_host_key
    prove_ssh || fail "the SSH server of $name refused the key; read '$0 status' and docker logs $name"
    log "the SSH server accepts the key on 127.0.0.1:${ssh_port}"
    prove_vnc || fail "the VNC server of $name gave no protocol greeting on 127.0.0.1:${vnc_port}"
    log "the VNC server greets on 127.0.0.1:${vnc_port}"
    printf '\n%s\n' "$(alias_block)"
    printf 'Add this block to ~/.ssh/config, or run this command again with --write-alias.\n'
    printf 'The SSH target is %s, and Horizon takes it as the entry browser_host.ssh_target.\n' "$name"
    printf 'The state volume %s keeps the sign-ins.\n' "$volume"
}

stop() {
    if ! container_exists; then
        log "the container $name does not run"
    else
        "$docker_command" stop "$name" >/dev/null 2>&1 || true
        "$docker_command" rm "$name" >/dev/null 2>&1 || true
        log "the container $name stopped and removed"
    fi
    log "the state volume $volume stays, and so does the key in $ssh_dir"
}

# The alias block is written once: every earlier block of this script leaves
# first, so two runs cannot leave two blocks that name the same host.
write_ssh_alias() {
    local config="${HOME}/.ssh/config"
    mkdir -p "${HOME}/.ssh"
    chmod 0700 "${HOME}/.ssh"
    touch "$config"
    chmod 0600 "$config"
    local staged
    staged="$(mktemp "${TMPDIR:-/tmp}/horizon-ssh-config.XXXXXX")"
    awk -v begin="$(alias_begin)" -v end="$(alias_end)" '
        $0 == begin { skip = 1; next }
        $0 == end { skip = 0; next }
        !skip { print }
    ' "$config" >"$staged"
    # A file that ends without a newline must not join the block to its last line.
    if [[ -s "$staged" && -n "$(tail -c 1 "$staged")" ]]; then
        printf '\n' >>"$staged"
    fi
    alias_block >>"$staged"
    cat "$staged" >"$config"
    rm -f "$staged"
    log "wrote the alias block of $name to $config"
}

show_status() {
    local running="no"
    if container_running; then
        running="yes"
    fi
    printf 'container: %s\n' "$name"
    printf 'running: %s\n' "$running"
    printf 'image: %s\n' "$image"
    printf 'state volume: %s\n' "$volume"
    printf 'vnc: 127.0.0.1:%s\n' "$vnc_port"
    printf 'ssh: 127.0.0.1:%s (target %s)\n' "$ssh_port" "$name"
    printf 'key: %s\n' "$key_file"
    if [[ "$running" == "yes" ]]; then
        printf 'pages: %s\n' "$("$docker_command" exec "$name" horizon-browser profiles 2>/dev/null || printf 'unknown')"
    fi
}

case "$command_name" in
    up)
        start
        if [[ "$write_alias" == 1 ]]; then
            write_ssh_alias
        fi
        ;;
    down) stop ;;
    status) show_status ;;
    alias)
        alias_block
        if [[ "$write_alias" == 1 ]]; then
            write_ssh_alias
        fi
        ;;
    key) prepare_state && cat "${key_file}.pub" ;;
esac
