#!/bin/sh
# EGR-05: runs as root on the host of the check server.
A="docker exec horizon-worker setpriv --reuid=10001 --regid=10001 --clear-groups"
probe() { printf '%s: ' "$1"; $A curl -sS -m 8 -o /dev/null -w '%{http_code}\n' https://api.github.com/ 2>&1 | tail -1; }
probe "baseline"
echo "== stop the proxy"; systemctl stop horizon-worker-egress-proxy; probe "proxy stopped"
$A getent hosts github.com >/dev/null && echo "dns still answers"
echo "== start it again, then kill -9 it"; systemctl start horizon-worker-egress-proxy; sleep 1; probe "proxy started"
pid=$(systemctl show -p MainPID --value horizon-worker-egress-proxy); kill -9 $pid; probe "right after kill -9"; sleep 2
echo "new pid differs: $( [ "$(systemctl show -p MainPID --value horizon-worker-egress-proxy)" != "$pid" ] && echo yes)"; probe "after restart"
echo "== stop the resolver"; systemctl stop horizon-worker-egress-dns; printf 'resolve github.com: '; $A getent hosts github.com >/dev/null && echo answered || echo "no answer (exit $?)"
probe "resolver stopped"
systemctl start horizon-worker-egress-dns
echo "== remove every egress rule (as if they were lost)"
iptables -D DOCKER-USER -i horizon0 -j HORIZON-EGRESS; iptables -t nat -D PREROUTING -i horizon0 -j HORIZON-EGRESS; iptables -D INPUT -i horizon0 -j HORIZON-EGRESS-IN
printf 'direct to 140.82.121.3:443 without rules: '; $A curl -sS -m 8 --resolve api.github.com:443:140.82.121.3 -o /dev/null -w '%{http_code}\n' https://api.github.com/ 2>&1 | tail -1
printf 'DNS to 1.1.1.1 without rules: '; $A python3 -c "
import socket
q=b'\x12\x34\x01\x00\x00\x01\x00\x00\x00\x00\x00\x00\x07example\x03com\x00\x00\x01\x00\x01'
s=socket.socket(socket.AF_INET,socket.SOCK_DGRAM); s.settimeout(5); s.sendto(q,('1.1.1.1',53))
try: print('ANSWERED', len(s.recv(512)))
except Exception as e: print('no answer:', type(e).__name__)"
echo "== restart the worker: the guard installs the rules again"
systemctl restart horizon-worker; sleep 8; iptables -S DOCKER-USER | grep -c HORIZON-EGRESS; iptables -t nat -S PREROUTING | grep -c HORIZON-EGRESS
for i in $(seq 1 20); do docker exec horizon-worker true 2>/dev/null && break; sleep 3; done; probe "after worker restart"
