      qualify_worker_runtime() (
        uid=$1
        probe_token=$(cat /proc/sys/kernel/random/uuid)
        probe_name="horizon-runtime-probe-$probe_token"
        cleanup_probe() {
          ids=$(timeout --kill-after=5s 30 docker container ls --all --quiet --filter "name=^/$probe_name$" --filter "label=io.horizon.runtime-probe=$probe_token") || return 1
          if [ -n "$ids" ]; then
            timeout --kill-after=5s 30 docker rm --force "$ids" >/dev/null || return 1
          fi
          ids=$(timeout --kill-after=5s 30 docker container ls --all --quiet --filter "name=^/$probe_name$" --filter "label=io.horizon.runtime-probe=$probe_token") || return 1
          [ -z "$ids" ]
        }
        finish_probe() {
          status=$?
          trap - EXIT
          cleanup_probe || status=1
          exit "$status"
        }
        trap finish_probe EXIT
        timeout --kill-after=5s 60 docker create --name "$probe_name" --label "io.horizon.runtime-probe=$probe_token" --network none --user "$uid:$uid"__RUNTIME_FLAGS__ --entrypoint horizon-agent-sandbox-smoke '__WORKER_IMAGE__' >/dev/null
        timeout --kill-after=5s 60 docker start --attach "$probe_name" | python3 -c 'import json,sys; p=json.load(sys.stdin); assert p["passed"] is True'
        cleanup_probe
        trap - EXIT
      )
      for uid in 0 1000; do qualify_worker_runtime "$uid"; done
