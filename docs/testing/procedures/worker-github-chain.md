---
procedure: worker-github-chain
feature: Worker GitHub access with a token chain that refreshes on the worker
platforms: [linux]
cost: none
destructive: yes
secrets: [client ID of a test GitHub App, sign-in of a test GitHub account, real access and refresh token chain in <evidence>/reply.json and <evidence>/real-chain.json, SSH private key in <evidence>/key]
owner: peters
---

# Worker GitHub token chain test procedure

## 1. Purpose

This procedure makes sure that the chain service on a worker keeps a token chain
fresh without the host. It also makes sure that agents get only the access token
of a granted repository, and never the token chain. It also makes sure that an
agent can ask for more access, and that the worker obeys the decision.

## 2. Applicability

- Candidate: each candidate that changes `horizon-worker-github`, its modules
  `horizon-worker-github-common` and `horizon-worker-github-agents`,
  `horizon-worker-git-auth`, `horizon-worker-supervise` or the token chain part
  of `horizon-worker-check`.
- Platforms: Linux with Docker.
- Lanes:
  - Lane U: the unit tests.
  - Lane C: a local worker container and a fake GitHub.
  - Lane R: access requests in the lane C container.
  - Lane G: a local worker container and a real GitHub App.
- This procedure does not test: the Horizon host side that gets the token chain,
  a worker at a provider, or a volume that does not keep POSIX modes. The unit
  tests simulate that volume.

## 3. Safety

> **CAUTION:** DO NOT SHOW A TOKEN IN EVIDENCE. A token chain gives access to
> the repositories of the test account. Lane C uses synthetic tokens only.

> **CAUTION:** USE ONLY A TEST GITHUB APP AND A TEST ACCOUNT IN LANE G. Lane G
> sends a real token chain to the container.

> **CAUTION:** REMOVE ONLY THE CONTAINERS, VOLUMES AND IMAGES THAT THIS RUN MADE.
> Other containers can hold the work of other people.

## 4. Equipment and preconditions

- A Linux computer with Docker and Python 3.12 or newer.
- A checkout of the candidate commit. In this procedure, `<repo>` is its path.
- A private evidence directory, `<evidence>`, outside the checkout.
- The pinned base image from
  `crates/horizon-core/src/cloud_runtime/repository/launch/quick_start.rs`. In
  this procedure, `<base>` is its reference with the digest.
- For lane G only:
  - A test GitHub App with the device flow on and expiring user tokens on.
    `<client-id>` is its client ID.
  - A test GitHub account that can push to a synthetic repository,
    `<owner>/<name>`. The app must be installed on that repository.

In this procedure, `<c>` is the container name `chain-smoke-<nonce>`, and `<v>`
is the volume name `chain-smoke-<nonce>`. `<nonce>` is a random value of this run.

## 5. Setup

1. Make a new, empty directory `<ctx>` outside the checkout.

   Result: The directory exists and is empty.

2. Copy the worker scripts of the candidate into `<ctx>`:

   ```bash
   cp <repo>/examples/cloud-worker/horizon-worker-* <ctx>/
   ```

   Result: `<ctx>` contains `horizon-worker-github` and the other scripts.

3. Write this `<ctx>/Dockerfile`:

   ```dockerfile
   FROM <base>
   COPY horizon-worker-* /usr/local/bin/
   RUN chmod 755 /usr/local/bin/horizon-worker-*
   ```

   Result: The file has three lines.

4. Build the image:

   ```bash
   docker build -t chain-smoke:<nonce> <ctx>
   ```

   Result: Docker shows the image ID. Write it in the evidence.

5. Make a new SSH key pair in `<evidence>`:

   ```bash
   ssh-keygen -q -t ed25519 -N '' -f <evidence>/key
   ```

   Result: The files `key` and `key.pub` exist.

6. Write this fake GitHub to `<evidence>/fake-github.py`:

   ```python
   import http.server, json, urllib.parse
   count = 0
   class Handler(http.server.BaseHTTPRequestHandler):
       def reply(self, status, value):
           self.send_response(status)
           self.send_header('Content-Type', 'application/json')
           self.end_headers()
           self.wfile.write(json.dumps(value).encode())
       def do_POST(self):
           global count
           form = urllib.parse.parse_qs(self.rfile.read(int(self.headers['Content-Length'])).decode())
           if form['refresh_token'][0] == 'ghr_synthetic-revoked':
               return self.reply(200, {'error': 'bad_refresh_token'})
           count += 1
           self.reply(200, {'access_token': 'ghu_synthetic-%d' % count, 'expires_in': 28800,
                            'refresh_token': 'ghr_synthetic-%d' % count,
                            'refresh_token_expires_in': 15724800, 'token_type': 'bearer'})
       def do_GET(self):
           if self.path == '/repos/example/missing':
               return self.reply(404, {'message': 'Not Found'})
           self.reply(200, {'permissions': {'pull': True, 'push': True}})
   http.server.HTTPServer(('127.0.0.1', 18080), Handler).serve_forever()
   ```

   Result: The file exists. It answers the first refresh with `ghu_synthetic-1`.
   It answers that each repository except `example/missing` accepts a push.

## 6. Tasks

### 6.1 U1: Unit tests

1. Run the worker tests:

   ```bash
   python3 -B -m unittest discover -s <repo>/examples/cloud-worker -p 'test_*.py'
   ```

   Result: The last line shows `OK`. The `test_horizon_worker_github` tests pass.

### 6.2 C1: Contract marker and service start

1. Start the container with the fake GitHub address. Select no agents, because
   this lane does not need them:

   ```bash
   docker volume create <v>
   docker run -d --name <c> -v <v>:/workspace \
       -e PUBLIC_KEY="$(cat <evidence>/key.pub)" \
       -e HORIZON_WORKER_GITHUB_TEST_URL=http://127.0.0.1:18080 \
       -e 'HORIZON_WORKER_CAPABILITIES={"agents":[],"browsers":[],"desktop":false}' \
       chain-smoke:<nonce>
   ```

   Result: Docker shows the container ID.

2. Wait until the worker is ready:

   ```bash
   docker exec <c> horizon-worker-check --ready
   ```

   Result: The command exits with status 0. If it fails, wait 5 seconds and do
   this step again.

3. Examine the contract marker:

   ```bash
   docker exec <c> horizon-worker-check --git-auth > <evidence>/check.txt \
     && grep -x horizon-github-chain-contract=1 <evidence>/check.txt
   ```

   Result: The command exits with status 0 and shows
   `horizon-github-chain-contract=1`. A failed check stops the command before the
   marker is examined.

4. Examine the services:

   ```bash
   docker exec <c> cat /run/horizon-worker/services.json
   docker exec <c> ls -l /run/horizon-worker/github.sock
   ```

   Result: `services.json` contains `github`. The socket has the mode `srw-rw-rw-`.

5. Examine the status:

   ```bash
   docker exec <c> horizon-worker-github status
   ```

   Result: The JSON shows `"state":"absent"`, `"serving":true` and an empty
   `repositories` list.

### 6.3 C2: Install

1. Make the primary bare repository as the agent user:

   ```bash
   docker exec <c> horizon-worker-tailnet agent /usr/bin/git init -q --bare /workspace/repository.git
   ```

   Result: The command exits with status 0.

2. Copy the fake GitHub into the container and start it:

   ```bash
   docker cp <evidence>/fake-github.py <c>:/root/fake-github.py
   docker exec -d <c> python3 /root/fake-github.py
   ```

   Result: Both commands exit with status 0.

3. Write a synthetic token chain whose access token expires in 10 minutes:

   ```bash
   now=$(date +%s)
   printf '%s' '{"version":1,"client_id":"Iv23synthetic","author_name":"Test Author",
   "author_email":"author@example.invalid",
   "grants":[{"repository":"example/project","target":"primary","access":"push"}],
   "chain":{"access_token":"ghu_synthetic-0","access_expires_at":'$((now + 600))',
   "refresh_token":"ghr_synthetic-0","refresh_expires_at":'$((now + 86400))'}}' > <evidence>/chain.json
   ```

   Result: The file contains one JSON object.

4. Install the chain:

   ```bash
   docker exec -i <c> horizon-worker-github install < <evidence>/chain.json
   ```

   Result: The JSON shows `"state":"ok"` and `"persistent":true`. It shows no token.

5. Examine the repository configuration:

   ```bash
   docker exec <c> horizon-worker-tailnet agent /usr/bin/git --git-dir=/workspace/repository.git config remote.origin.url
   ```

   Result: The command shows `https://github.com/example/project.git`.

### 6.4 C3: Refresh

1. Wait 70 seconds.

   Result: The chain service polls one time each minute.

2. Examine the status:

   ```bash
   docker exec <c> horizon-worker-github status
   ```

   Result: `last_refresh_at` has a value. `access_expires_at` is about 8 hours
   after `last_refresh_at`. `last_error` is `null`.

### 6.5 C4: Agent access

1. Ask for the credential of the granted repository as the agent user:

   ```bash
   printf 'protocol=https\nhost=github.com\npath=example/project.git\n\n' \
       | docker exec -i <c> horizon-worker-tailnet agent horizon-worker-git-auth get
   ```

   Result: The command shows `password=ghu_synthetic-1`. It does not show a
   `ghr_` value.

2. Do step 1 again with the path `example/other.git`.

   Result: The command shows nothing.

3. Ask the `gh` wrapper for its token:

   ```bash
   docker exec <c> horizon-worker-tailnet agent /usr/bin/env GH_REPO=example/project gh auth token
   ```

   Result: The command shows `ghu_synthetic-1`.

4. Do step 3 again with `GH_REPO=example/other`.

   Result: The command shows `horizon: no Git grant for this repository`. It shows
   no token.

### 6.6 C5: Private storage

1. Try to read the chain as the agent user:

   ```bash
   docker exec <c> horizon-worker-tailnet agent cat /workspace/.horizon-root/github/state.json
   ```

   Result: The command shows `Permission denied`.

2. Examine the owner and mode of the storage:

   ```bash
   docker exec <c> stat -c '%U %a %n' /workspace/.horizon-root /workspace/.horizon-root/github \
       /workspace/.horizon-root/github/state.json
   ```

   Result: The owner is `root`. The modes are `700`, `700` and `600`.

3. Examine the request log:

   ```bash
   docker exec <c> tail -3 /run/horizon-github/requests.log
   docker exec <c> grep -c 'ghu_\|ghr_' /run/horizon-github/requests.log
   ```

   Result: Each line shows `uid` 10001 and a `pid`. The count is `0`.

### 6.7 C6: Container recreation

1. Record the status:

   ```bash
   docker exec <c> horizon-worker-github status > <evidence>/before.json
   ```

   Result: The file contains the status.

2. Remove the container, but keep the volume:

   ```bash
   docker rm -f <c>
   ```

   Result: Docker shows the container name.

3. Do the steps 1 and 2 of task C1 again with the same volume.

   Result: The worker is ready.

4. Examine the status:

   ```bash
   docker exec <c> horizon-worker-github status
   ```

   Result: The status is the same as `<evidence>/before.json`.

5. Do step 1 of task C4 again.

   Result: The command shows `password=ghu_synthetic-1`.

### 6.8 C7: Revoked chain

1. Start the fake GitHub again. Do step 2 of task C2.

   Result: Both commands exit with status 0.

2. Do step 3 of task C2 again with the refresh token `ghr_synthetic-revoked`.

   Result: The file contains one JSON object.

3. Install the chain. Do step 4 of task C2.

   Result: The JSON shows `"state":"ok"`.

4. Wait 70 seconds. Then examine the status:

   ```bash
   docker exec <c> horizon-worker-github status
   ```

   Result: The JSON shows `"state":"revoked"` and `"last_error":"bad_refresh_token"`.

5. Do step 1 of task C4 again.

   Result: The command shows nothing.

### 6.9 R1: Request outside an agent session

1. Install a new chain. Do the steps 2, 3 and 4 of task C2 again.

   Result: The JSON shows `"state":"ok"` and `"pending_requests":0`.

2. Call the tool from a process that is not in an agent session:

   ```bash
   printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"github_access","arguments":{"repository":"example/extra","access":"push","reason":"Push the fix"}}}' \
       | docker exec -i <c> horizon-worker-tailnet agent horizon-worker-github mcp
   ```

   Result: The reply shows `Only an agent session on this worker can ask for
   GitHub access` and `"isError": true`.

### 6.10 R2: Allow for this cloud

1. Mark a synthetic agent session:

   ```bash
   docker exec <c> mkdir -p /workspace/sessions/agent-smoke
   docker exec <c> sh -c 'echo claude > /workspace/sessions/agent-smoke/agent'
   ```

   Result: Both commands exit with status 0.

2. Write the tool input for the session:

   ```bash
   printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"github_access","arguments":{"repository":"example/extra","access":"push","reason":"Push the fix"}}}' \
       | docker exec -i <c> tee /workspace/home/mcp-in.jsonl
   docker exec <c> chown 10001:10001 /workspace/home/mcp-in.jsonl
   ```

   Result: The file contains one line.

3. Start the session. It calls the tool, then asks Git for a credential:

   ```bash
   docker exec <c> horizon-worker-tailnet agent tmux -L horizon-cloud new-session -d -s agent-smoke \
       'horizon-worker-github mcp < /workspace/home/mcp-in.jsonl > /workspace/home/mcp-out.jsonl; printf "protocol=https\nhost=github.com\npath=example/extra.git\n\n" | horizon-worker-git-auth get > /workspace/home/cred.txt; sleep 600'
   ```

   Result: The command exits with status 0.

4. List the requests:

   ```bash
   docker exec <c> horizon-worker-github requests
   ```

   Result: The JSON shows one request for `example/extra` with `"access":"push"`
   and `"session":"agent-smoke"`, and no agent name: the session's agent marker
   is not a verified identity. Write its `id` as `<id>`.

5. Allow the request for the cloud:

   ```bash
   docker exec <c> horizon-worker-github decide <id> allow-cloud
   ```

   Result: The JSON shows `"ok":true` and `"status":"allowed"`.

6. Wait 10 seconds. Then examine the tool output and the credential:

   ```bash
   docker exec <c> cat /workspace/home/mcp-out.jsonl /workspace/home/cred.txt
   ```

   Result: The tool output shows `Allowed for this cloud`. The credential shows a
   `password=ghu_synthetic-` line.

7. Do step 1 of task C4 again with the path `example/extra.git`.

   Result: The command shows a `password=ghu_synthetic-` line. Access is per cloud,
   so a process outside the asking session gets the token too. The status shows
   `example/extra` with `"target":null`.

### 6.11 R3: Repository that GitHub does not show

1. Do the steps 2 and 3 of task R2 again with the repository `example/missing`,
   the access `read` and the window command `tmux ... new-window -t agent-smoke`.

   Result: `horizon-worker-github requests` shows a request for `example/missing`.

2. Allow the request for the cloud:

   ```bash
   docker exec <c> horizon-worker-github decide <id> allow-cloud
   ```

   Result: The JSON shows `"ok":false` and `"error":"not_installed"`.

3. Deny the request:

   ```bash
   docker exec <c> horizon-worker-github decide <id> deny
   ```

   Result: The JSON shows `"status":"denied"`. The status shows
   `"pending_requests":0`.

### 6.12 C8: Clear

1. Remove the chain:

   ```bash
   docker exec <c> horizon-worker-github clear
   docker exec <c> horizon-worker-github status
   ```

   Result: The JSON shows `"state":"absent"`.

2. Examine the services:

   ```bash
   docker exec <c> cat /run/horizon-worker/services.json
   ```

   Result: `services.json` still contains `github`. The worker did not stop.

### 6.13 G1: Real GitHub refresh

Do this lane in a new container without `HORIZON_WORKER_GITHUB_TEST_URL`. Do
the steps 1 and 2 of task C1 without that variable, and step 1 of task C2.

1. Ask GitHub for a device code:

   ```bash
   curl -s -X POST https://github.com/login/device/code \
       -H 'Accept: application/json' -d client_id=<client-id>
   ```

   Result: The JSON shows `device_code`, `user_code` and `verification_uri`.

2. Open `verification_uri` and type `user_code` as the test account.

   Result: GitHub shows that the device is connected.

> **CAUTION:** KEEP THE REPLY OF THE NEXT STEP PRIVATE. It contains a real token chain.

3. Get the token chain:

   ```bash
   curl -s -X POST https://github.com/login/oauth/access_token -H 'Accept: application/json' \
       -d client_id=<client-id> -d device_code=<device-code> \
       -d grant_type=urn:ietf:params:oauth:grant-type:device_code > <evidence>/reply.json
   ```

   Result: The file contains `access_token`, `expires_in`, `refresh_token` and
   `refresh_token_expires_in`.

4. Make the install file. Set the access expiry 10 minutes from now, so that the
   chain service refreshes at its next poll:

   ```bash
   python3 -c 'import json, sys, time; r = json.load(open(sys.argv[1])); now = int(time.time())
   print(json.dumps({"version": 1, "client_id": sys.argv[2], "author_name": "Test Author",
       "author_email": "author@example.invalid",
       "grants": [{"repository": sys.argv[3], "target": "primary", "access": "push"}],
       "chain": {"access_token": r["access_token"], "access_expires_at": now + 600,
                 "refresh_token": r["refresh_token"],
                 "refresh_expires_at": now + r["refresh_token_expires_in"]}}))' \
       <evidence>/reply.json <client-id> <owner>/<name> > <evidence>/real-chain.json
   ```

   Result: The file contains one JSON object.

> **CAUTION:** THE NEXT STEP SENDS A REAL TOKEN CHAIN TO THE CONTAINER. Use only
> the test container of this run.

5. Install the chain. Do step 4 of task C2 with `<evidence>/real-chain.json`.

   Result: The JSON shows `"state":"ok"`.

6. Wait 70 seconds. Then examine the status.

   Result: `last_refresh_at` has a value and `last_error` is `null`.

7. Read the repository as the agent user:

   ```bash
   docker exec <c> horizon-worker-tailnet agent /usr/bin/git ls-remote https://github.com/<owner>/<name>.git
   ```

   Result: The command shows the references of the synthetic repository.

> **CAUTION:** THE NEXT STEP SENDS THE OLD REFRESH TOKEN TO GITHUB. The token goes
> over standard input, never in a command line, so it stays out of the shell
> history and the process list.

8. Try to refresh with the old refresh token from `<evidence>/reply.json`:

   ```bash
   python3 -c 'import json, sys, urllib.parse; r = json.load(open(sys.argv[1]))
   sys.stdout.write(urllib.parse.urlencode({"client_id": sys.argv[2], "grant_type": "refresh_token",
                                            "refresh_token": r["refresh_token"]}))' \
       <evidence>/reply.json <client-id> \
     | curl -s -X POST https://github.com/login/oauth/access_token -H 'Accept: application/json' --data @-
   ```

   Result: GitHub answers `"error":"bad_refresh_token"`. The chain service
   rotated the chain, and GitHub cancelled the old one.

## 7. Pass criteria

- Lane U shows `OK`.
- The worker reports `horizon-github-chain-contract=1` and starts the chain service.
- The chain service refreshes before the access token expires and stores the new
  token chain on the volume.
- The agent user gets the access token only for a granted repository.
- The agent user cannot read the token chain. No reply and no log line contains
  a refresh token.
- The token chain survives a recreated container.
- `bad_refresh_token` makes the state `revoked`, and agents then get no token.
- `clear` makes the state `absent`, and the worker continues to run.
- Only an agent session can ask for access. An allowed repository reaches every
  session of the cloud.
- A decision fails, and the request stays pending, when GitHub does not show the
  repository.
- In lane G, the real refresh works and GitHub refuses the old refresh token.

## 8. Cleanup

1. Remove the token chain. Do step 1 of task C8.

   Result: The JSON shows `"state":"absent"`.

> **CAUTION:** THE NEXT STEP DELETES A CONTAINER, A VOLUME AND AN IMAGE. Use only
> the names that this run made.

2. Remove the container, the volume and the image of this run:

   ```bash
   docker rm -f <c>
   docker volume rm <v>
   docker rmi chain-smoke:<nonce>
   ```

   Result: Docker shows each name.

> **CAUTION:** REVOKE ONLY THE AUTHORIZATION OF THE TEST GITHUB APP.

3. For lane G, open **Settings**, **Applications**, **Authorized GitHub Apps** as
   the test account. Revoke the test GitHub App.

   Result: The test GitHub App is not in the list.

> **CAUTION:** THE NEXT STEP DELETES FILES THAT HOLD SECRETS. Delete only the files
> of this run.

4. Delete `<evidence>/reply.json`, `<evidence>/real-chain.json`, `<evidence>/key`
   and `<evidence>/key.pub`.

   Result: No real token and no private key of this run stays on the computer.

## 9. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). If not, put the results in the
pull request. Keep private evidence out of the repository.
