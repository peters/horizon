#!/bin/bash
# The Upkeep worker exchange in a disposable worker container, as root, with links that the
# agent planted. Usage: worker.sh IMAGE COMMAND-FILE
# COMMAND-FILE holds the exact remote command that Horizon builds (see the procedure).
set -u
if [ "${1:-}" != --inside ]; then
    image="${1:?worker image required}"; command_file="$(realpath "${2:?command file required}")"
    exec docker run --rm --network none -v "$command_file:/check/command.sh:ro" \
        -v "$(realpath "$0"):/check/worker.sh:ro" --entrypoint /bin/bash "$image" /check/worker.sh --inside
fi
fail=0
check() { if eval "$2"; then echo "PASS $1"; else echo "FAIL $1"; fail=1; fi; }
mkdir -p /workspace/home /workspace/sessions /workspace/session-data /run/horizon-tailnet
chown 10001:10001 /workspace/home /workspace/sessions /workspace/session-data
chmod 700 /workspace/home /workspace/sessions /workspace/session-data
cp /etc/hostname /run/hostname-before
setpriv --reuid=10001 --regid=10001 --clear-groups -- sh -c '
    mkdir -p /workspace/home/upkeep /workspace/sessions/upkeep-abc-123
    ln -s /etc/hostname /workspace/home/upkeep/config.json
    ln -s /etc/hostname /workspace/sessions/upkeep-abc-123/revision
    ln -s /etc/shadow /workspace/home/upkeep/status.json'

refusal="$(sh /check/command.sh 2>&1)"
check "root without isolation is refused" '[ "$refusal" = "Upkeep refused this worker: /workspace/home belongs to another account." ]'
touch /run/horizon-tailnet/agent-isolation
answer="$(sh /check/command.sh)"
check "the exchange answers" 'printf %s "$answer" | python3 -c "import json,sys; a=json.load(sys.stdin); sys.exit(not (a[\"loop\"] and a[\"status\"] is None))"'
check "no file outside the folders changed" 'cmp -s /etc/hostname /run/hostname-before'
check "the links are replaced by files" '[ -f /workspace/home/upkeep/config.json ] && [ ! -L /workspace/home/upkeep/config.json ] && [ ! -L /workspace/sessions/upkeep-abc-123/revision ]'
check "the agent account owns every written path" '[ -z "$(find /workspace/home /workspace/sessions /workspace/session-data ! -user horizon-agent)" ]'
sleep 2
check "the loop runs as the agent account" '[ -n "$(pgrep -u 10001 -f upkeep/.active/run.py)" ] && [ -z "$(pgrep -u 0 -f upkeep/.active/run.py)" ]'
exit "$fail"
