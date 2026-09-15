      /usr/local/sbin/horizon-worker-runtime-preflight
      [ "$(docker version --format '{{.Server.Version}}')" = '29.1.3' ]
      docker info --format '{{json .SecurityOptions}}' | python3 -c 'import json,sys; p=json.load(sys.stdin); assert "name=apparmor" in p and "name=seccomp,profile=builtin" in p'
