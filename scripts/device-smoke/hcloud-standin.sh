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
# The one machine that this program reports lives in small files beside the copy,
# one for the machine, one for its ssh key and one for its firewall, so `create`,
# `list`, `describe`, `delete` and the proof of the cleanup agree, and a second run
# of the procedure starts from nothing again when the files are removed.
#
# The address of the machine is `127.0.0.1`, so the SSH step of the flow fails at
# once: no machine holds the key that Horizon made. That failure is the point of
# the walk: the card of the flow shows the failure card.
set -eu

here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
record="$here/hcloud-standin.record"
key_record="$here/hcloud-standin.key"
firewall_record="$here/hcloud-standin.firewall"
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

# The name that one create call gives, from its `--name` argument.
created_name() {
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
    printf '%s\n' "$name"
}

# The name that one call of a firewall or of a key names: the last argument of the
# call, as `firewall delete $NAME-fw` and `firewall add-rule ... $NAME-fw` give it.
last_name() {
    for argument in "$@"; do
        name=$argument
    done
    if [ -z "${name:-}" ]; then
        echo "hcloud-standin: this call names no resource" >&2
        exit 1
    fi
    printf '%s\n' "$name"
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
        name=$(created_name "$@")
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
        name=$(created_name "$@")
        printf '%s\n' "$name" > "$key_record"
        printf '{"ssh_key": {"id": 20000001, "name": "%s"}}\n' "$name"
        ;;
    "ssh-key list")
        printf '%-10s %s\n' "ID" "NAME"
        if [ -f "$key_record" ]; then
            printf '%-10s %s\n' "20000001" "$(cat "$key_record")"
        fi
        ;;
    "ssh-key delete")
        name=$(last_name "$@")
        if [ -f "$key_record" ] && [ "$(cat "$key_record")" = "$name" ]; then
            rm -f "$key_record"
            echo "SSH key $name deleted"
        else
            echo "hcloud-standin: no ssh key $name. It is not part of the stand-in." >&2
            exit 1
        fi
        ;;
    "firewall create")
        name=$(created_name "$@")
        printf '%s\n' "$name" > "$firewall_record"
        printf '{"firewall": {"id": 30000001, "name": "%s", "rules": []}}\n' "$name"
        ;;
    "firewall add-rule")
        name=$(last_name "$@")
        if [ -f "$firewall_record" ] && [ "$(cat "$firewall_record")" = "$name" ]; then
            echo "Firewall rule added"
        else
            echo "hcloud-standin: no firewall $name. It is not part of the stand-in." >&2
            exit 1
        fi
        ;;
    "firewall apply-to-resource")
        name=$(last_name "$@")
        if [ -f "$firewall_record" ] && [ "$(cat "$firewall_record")" = "$name" ]; then
            echo "Firewall $name applied"
        else
            echo "hcloud-standin: no firewall $name. It is not part of the stand-in." >&2
            exit 1
        fi
        ;;
    "firewall delete")
        name=$(last_name "$@")
        if [ -f "$firewall_record" ] && [ "$(cat "$firewall_record")" = "$name" ]; then
            rm -f "$firewall_record"
            echo "Firewall $name deleted"
        else
            echo "hcloud-standin: no firewall $name. It is not part of the stand-in." >&2
            exit 1
        fi
        ;;
    "all list")
        # The proof of the cleanup reads one document with one array for each kind
        # of resource that the labels cover. The stand-in holds three of them.
        servers=
        if [ -f "$record" ]; then
            servers=$(server_json "$(cat "$record")")
        fi
        firewalls=
        if [ -f "$firewall_record" ]; then
            firewalls=$(printf '{"id": 30000001, "name": "%s"}' "$(cat "$firewall_record")")
        fi
        keys=
        if [ -f "$key_record" ]; then
            keys=$(printf '{"id": 20000001, "name": "%s"}' "$(cat "$key_record")")
        fi
        printf '{"servers": [%s], "firewalls": [%s], "ssh_keys": [%s]}\n' "$servers" "$firewalls" "$keys"
        ;;
    *)
        echo "hcloud-standin: this call is not part of the stand-in: $*" >&2
        exit 1
        ;;
esac
