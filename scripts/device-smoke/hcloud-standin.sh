#!/bin/sh
# A stand-in for the `hcloud` program of the Set up procedure.
#
# The procedure of the Set up wizard walks the rent flow without money. This
# program answers the calls that the skill `horizon-machines` makes of the
# Hetzner CLI, in the words that the reference gives, and it makes nothing: it
# opens no socket, reaches no network and rents no machine. A procedure puts a
# copy of this file on a private `PATH` under the name `hcloud`, so the agent
# finds it as the provider CLI.
#
# The one machine that this program reports lives in one small file beside the
# copy, so `create`, `list`, `describe` and `delete` agree, and a second run of
# the procedure starts from no machine again when the file is removed.
#
# The address of the machine is `127.0.0.1`, so the SSH step of the flow fails at
# once: no machine holds the key that Horizon made. That failure is the point of
# the walk: the card of the flow shows the failure card.
set -eu

here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
record="$here/hcloud-standin.record"
address=127.0.0.1
created=2026-01-01T09:40:00+00:00

first=${1:-}
second=${2:-}

machine_name() {
    if [ -f "$record" ]; then
        cat "$record"
    else
        echo "hcloud-standin: no machine. Run create first." >&2
        exit 1
    fi
}

server_json() {
    name=$1
    printf '{"server": {"name": "%s", "status": "running", "location": {"name": "hel1"}, ' "$name"
    printf '"server_type": {"name": "cx22"}, "public_net": {"ipv4": {"ip": "%s"}}, ' "$address"
    printf '"created": "%s"}}\n' "$created"
}

server_row() {
    name=$1
    printf '%-16s %-10s %-16s %s\n' "ID" "NAME" "STATUS" "IPV4"
    printf '%-16s %-10s %-16s %s\n' "10000001" "$name" "running" "$address"
}

case "$first $second" in
    "version ")
        echo "hcloud 1.69.0"
        ;;
    "context active")
        echo "stand-in"
        ;;
    "server list")
        # `--quiet` still prints its header when no machine exists.
        if [ -f "$record" ]; then
            server_row "$(cat "$record")"
        else
            printf '%-16s %-10s %-16s %s\n' "ID" "NAME" "STATUS" "IPV4"
        fi
        ;;
    "server describe")
        name=$(machine_name)
        server_json "$name"
        ;;
    "server create")
        name=""
        for argument in "$@"; do
            if [ "$name" = "next" ]; then
                name=$argument
                break
            fi
            if [ "$argument" = "--name" ]; then
                name=next
            fi
        done
        if [ -z "$name" ] || [ "$name" = "next" ]; then
            echo "hcloud-standin: create needs --name" >&2
            exit 1
        fi
        printf '%s\n' "$name" > "$record"
        # A create writes its progress to the standard error, as the real one does.
        echo "Waiting for server to be running..." >&2
        server_json "$name"
        ;;
    "server delete")
        name=$(machine_name)
        rm -f "$record"
        echo "Server deleted"
        ;;
    "server shutdown")
        name=$(machine_name)
        echo "Server $name shut down"
        ;;
    "server poweron")
        name=$(machine_name)
        echo "Server $name started"
        ;;
    "location list")
        printf 'hel1    Helsinki    FI    eu-central\n'
        printf 'fsn1    Falkenstein DE    eu-central\n'
        printf 'ash     Ashburn     US    us-east\n'
        printf 'sin     Singapore   SG    ap-southeast\n'
        ;;
    "server-type list")
        printf 'cx22    2    4.0    40    x86    shared    shared    v1    hel1,fsn1,ash,sin\n'
        printf 'cx32    4    8.0    80    x86    shared    shared    v1    hel1,fsn1,ash,sin\n'
        printf 'cx42    8    16.0   160   x86    shared    shared    v1    hel1,fsn1,ash,sin\n'
        ;;
    "image list")
        printf 'ubuntu-24.04    ubuntu    24.04    x86\n'
        printf 'ubuntu-22.04    ubuntu    22.04    x86\n'
        ;;
    "ssh-key create")
        echo '{"ssh_key": {"id": 20000001, "name": "build-1-key"}}'
        ;;
    "firewall create")
        echo '{"firewall": {"id": 30000001, "name": "build-1-fw", "rules": []}}'
        ;;
    "firewall add-rule")
        echo "Firewall rule added"
        ;;
    *)
        echo "hcloud-standin: this call is not part of the stand-in: $*" >&2
        exit 1
        ;;
esac
