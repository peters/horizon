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
fresh without the host. It also makes sure that Git reaches GitHub through the
Git proxy of the service, and `gh` through its API broker, which add the access
token only for a granted repository. Agents never get the token chain, and Git
and `gh` never get a token. It
also makes sure that an agent can ask for more access, and that the worker obeys
the decision.

## 2. Applicability

- Candidate: each candidate that changes `horizon-worker-github`, its modules
  `horizon-worker-github-common`, `horizon-worker-github-agents`,
  `horizon-worker-github-git`, `horizon-worker-github-http`,
  `horizon-worker-github-api`, `horizon-worker-github-api-rest`,
  `horizon-worker-github-graphql` and `horizon-worker-github-graphql-policy`,
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
   import base64, http.server, json, os, subprocess, urllib.parse
   count = 0
   PRIVATE = ('example/project', 'example/extra', 'example/secret')
   class Handler(http.server.BaseHTTPRequestHandler):
       def reply(self, status, value):
           self.send_response(status)
           self.send_header('Content-Type', 'application/json')
           self.end_headers()
           self.wfile.write(json.dumps(value).encode())
       def git(self):
           path, _, query = self.path.partition('?')
           repository = '/'.join(path.split('/')[1:3]).removesuffix('.git')
           sent = self.headers.get('Authorization', '')
           secret = base64.b64decode(sent[6:]).decode().partition(':')[2] if sent.startswith('Basic ') else ''
           with open('/root/github/seen.log', 'a') as log:
               log.write('%s %s\n' % (repository, 'token' if secret else 'none'))
           if repository in PRIVATE and not secret.startswith(('ghu_synthetic-', 'ghp_synthetic-')):
               self.send_response(401)
               self.end_headers()
               return
           data = self.rfile.read(int(self.headers.get('Content-Length', 0)))
           env = dict(os.environ, GIT_PROJECT_ROOT='/root/github', GIT_HTTP_EXPORT_ALL='1',
                      REMOTE_USER='fake', PATH_INFO=path, QUERY_STRING=query,
                      REQUEST_METHOD=self.command, CONTENT_LENGTH=str(len(data)),
                      CONTENT_TYPE=self.headers.get('Content-Type', ''),
                      HTTP_CONTENT_ENCODING=self.headers.get('Content-Encoding', ''),
                      GIT_PROTOCOL=self.headers.get('Git-Protocol', ''))
           output = subprocess.run(['git', 'http-backend'], input=data, env=env,
                                   capture_output=True).stdout
           head, _, body = output.partition(b'\r\n\r\n')
           fields = [line.split(': ', 1) for line in head.decode().split('\r\n') if line]
           self.send_response(next((int(v.split()[0]) for n, v in fields if n == 'Status'), 200))
           for name, value in fields:
               if name != 'Status':
                   self.send_header(name, value)
           self.end_headers()
           self.wfile.write(body)
       def do_POST(self):
           global count
           if '.git/' in self.path:
               return self.git()
           form = urllib.parse.parse_qs(self.rfile.read(int(self.headers['Content-Length'])).decode())
           if form['refresh_token'][0] == 'ghr_synthetic-revoked':
               return self.reply(200, {'error': 'bad_refresh_token'})
           count += 1
           self.reply(200, {'access_token': 'ghu_synthetic-%d' % count, 'expires_in': 28800,
                            'refresh_token': 'ghr_synthetic-%d' % count,
                            'refresh_token_expires_in': 15724800, 'token_type': 'bearer'})
       def do_GET(self):
           if '.git/' in self.path:
               return self.git()
           with open('/root/github/seen.log', 'a') as log:
               log.write('api %s %s\n' % (self.path, 'token' if self.headers.get('Authorization') else 'none'))
           if self.path == '/repos/example/missing':
               return self.reply(404, {'message': 'Not Found'})
           self.reply(200, {'permissions': {'pull': True, 'push': True}})
   http.server.HTTPServer(('127.0.0.1', 18080), Handler).serve_forever()
   ```

   Result: The file exists. It answers the first refresh with `ghu_synthetic-1`.
   It answers that each repository except `example/missing` accepts a push. It
   serves the Git repositories in `/root/github` and records in
   `/root/github/seen.log` whether each Git request and each API `GET` had a
   token, but never the token. `example/project`, `example/extra` and `example/secret` are private:
   they need a `ghu_synthetic-` token, or the `ghp_synthetic-` token of a static
   binding.

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

2. Copy the fake GitHub into the container, make its repositories and start it:

   ```bash
   docker cp <evidence>/fake-github.py <c>:/root/fake-github.py
   docker exec <c> sh -c 'for name in project extra secret public; do
       git init -q --bare -b main /root/github/example/$name.git; done'
   docker exec -d <c> python3 /root/fake-github.py
   ```

   Result: The commands exit with status 0.

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
   docker exec <c> horizon-worker-tailnet agent /usr/bin/git config --global --includes --get-regexp '^(http|url)\.'
   ```

   Result: The first command shows `https://github.com/example/project.git`. The
   second command shows `http.https://github.com/.proxy http://127.0.0.1:47281`,
   `http.https://github.com/.sslcainfo /run/horizon-worker/github-ca.pem` and
   `url.https://github.com/.insteadof` lines for `git@github.com:`,
   `ssh://git@github.com/`, `http://github.com/`, `https://www.github.com/` and
   `http://www.github.com/`: Git goes to GitHub through the Git proxy.

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

1. Clone the granted repository as the agent user. Then push a commit:

   ```bash
   docker exec <c> horizon-worker-tailnet agent sh -c 'cd /workspace/home \
       && git clone -q https://github.com/example/project.git project \
       && git -C project -c user.name=Smoke -c user.email=smoke@example.invalid \
          commit -q --allow-empty -m smoke \
       && git -C project push -q origin HEAD:main'
   docker exec <c> git --git-dir=/root/github/example/project.git log -1 --format=%s main
   ```

   Result: The first command exits with status 0. It can show that the cloned
   repository is empty. The second command shows `smoke`.

2. Read the granted repository through the proxy:

   ```bash
   docker exec <c> horizon-worker-tailnet agent git ls-remote https://github.com/example/project.git
   ```

   Result: The command shows `refs/heads/main`.

3. In the clone of step 1, ask the `gh` wrapper for its token without `GH_REPO`:

   ```bash
   docker exec <c> horizon-worker-tailnet agent sh -c 'cd /workspace/home/project \
       && git remote get-url origin && env -u GH_REPO gh auth token'
   ```

   Result: The command shows `https://github.com/example/project.git`, then
   `horizon-api-broker`. The remote URL stays a GitHub URL, so the wrapper finds
   the repository of the checkout. `gh` holds only the placeholder.

4. Clone a private repository that has no grant:

   ```bash
   docker exec <c> horizon-worker-tailnet agent git clone -q https://github.com/example/secret.git /workspace/home/secret
   ```

   Result: The command fails. It shows a line that starts with `remote: Horizon:
   example/secret has no GitHub grant on this worker`. It does not ask for a user name.

5. Clone a public repository that has no grant. Then try to push to it:

   ```bash
   docker exec <c> horizon-worker-tailnet agent sh -c 'cd /workspace/home \
       && git clone -q https://github.com/example/public.git public \
       && git -C public -c user.name=Smoke -c user.email=smoke@example.invalid \
          commit -q --allow-empty -m smoke \
       && git -C public push -q origin HEAD:main'
   ```

   Result: The clone works. The push fails and shows a line that starts with
   `remote: Horizon: example/public has no GitHub grant on this worker`.

6. Examine what the fake GitHub received:

   ```bash
   docker exec <c> sort -u /root/github/seen.log
   ```

   Result: `example/project` shows `token`. `example/secret` and
   `example/public` show only `none`. No line starts with `api` yet.

7. Ask the socket for a Git credential as the agent user:

   ```bash
   printf 'protocol=https\nhost=github.com\npath=example/project.git\n\n' \
       | docker exec -i <c> horizon-worker-tailnet agent horizon-worker-git-auth get
   ```

   Result: The command shows nothing. Git gets no token.

8. Ask the `gh` wrapper for its token:

   ```bash
   docker exec <c> horizon-worker-tailnet agent /usr/bin/env GH_REPO=example/project gh auth token
   ```

   Result: The command shows `horizon-api-broker`.

9. Do step 8 again with `GH_REPO=example/other`.

   Result: The command shows `horizon-api-broker` too. `gh` holds only the
   placeholder, and the API broker refuses its requests for `example/other` with
   its reason (step 12).

10. Examine the route of `gh`:

    ```bash
    docker exec <c> horizon-worker-tailnet agent /usr/bin/gh config get http_unix_socket
    docker exec <c> stat -c '%a %U' /run/horizon-worker/github-api.sock
    ```

    Result: The first command shows `/run/horizon-worker/github-api.sock`. The
    second shows `666 root`.

11. Read the granted repository with `gh` through the API broker:

    ```bash
    docker exec <c> horizon-worker-tailnet agent sh -c 'cd /workspace/home/project && gh api repos/example/project'
    ```

    Result: The command shows `{"permissions": {"pull": true, "push": true}}`.

12. Read a repository that has no grant, and a path outside a repository:

    ```bash
    docker exec <c> horizon-worker-tailnet agent sh -c 'cd /workspace/home/project && gh api repos/example/secret'
    docker exec <c> horizon-worker-tailnet agent sh -c 'cd /workspace/home/project && gh api user/repos'
    ```

    Result: Both commands fail. The first shows `gh: Horizon: example/secret has
    no GitHub grant on this worker. Ask for access with the github_access tool.
    (HTTP 403)`. The second shows a line that starts with `gh: Horizon: this
    worker does not let an agent reach /user/repos`.

13. Send the placeholder past the broker:

    ```bash
    docker exec <c> horizon-worker-tailnet agent curl -s -o /dev/null -w '%{http_code}\n' \
        --unix-socket /run/horizon-worker/github-api.sock http://api.github.com/repos/example/project
    docker exec <c> grep '^api' /root/github/seen.log
    ```

    Result: The first command shows `200`: the broker adds the token, and the
    caller holds none. The log shows `api /repos/example/project token` for
    steps 11 and 13 only. No line names `example/secret` or `/user/repos`.

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
   docker exec <c> grep '"request":"git-' /run/horizon-github/requests.log | tail -1
   docker exec <c> grep -c 'ghu_\|ghr_' /run/horizon-github/requests.log
   ```

   Result: Each line shows `uid` 10001. The last lines are of the socket
   (`gh-token`, with a `pid`) and of the API broker (`"kind":"api"`). The line of
   the Git proxy shows its kind in `request`, such as `git-push` or `git-read`, an
   `outcome` such as `relayed`, and `"pid":null`: the proxy names no process in
   its log. The count is `0`.

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

5. Do step 2 of task C2 again, because the new container has no fake GitHub.
   Then read the granted repository:

   ```bash
   docker exec <c> horizon-worker-tailnet agent git ls-remote https://github.com/example/project.git; echo "exit=$?"
   ```

   Result: The command shows `exit=0` and no `remote: Horizon:` line. The new
   repository is empty, so the command shows no reference.

### 6.8 C7: Revoked chain

1. Make sure that the fake GitHub runs:

   ```bash
   docker exec <c> pgrep -f fake-github.py
   ```

   Result: The command shows one process ID. If it shows nothing, do step 2 of
   task C2 again.

2. Do step 3 of task C2 again with the refresh token `ghr_synthetic-revoked`.

   Result: The file contains one JSON object.

3. Install the chain. Do step 4 of task C2.

   Result: The JSON shows `"state":"ok"`.

4. Wait 70 seconds. Then examine the status:

   ```bash
   docker exec <c> horizon-worker-github status
   ```

   Result: The JSON shows `"state":"revoked"` and `"last_error":"bad_refresh_token"`.

5. Do step 2 of task C4 again.

   Result: The command fails and shows `remote: Horizon: GitHub access on this
   worker was revoked`.

6. Install a static binding for `example/secret`, as Horizon does for a
   `git_credentials` binding that the app does not reach:

   ```bash
   printf '%s' '{"version":2,"grants":[{"repository":"example/secret","target":"primary",
   "token":"ghp_synthetic-static","author_name":"Test Author",
   "author_email":"author@example.invalid"}]}' \
       | docker exec -i <c> horizon-worker-git-auth install
   ```

   Result: The command exits with status 0.

7. Read `example/secret` and examine the Git configuration:

   ```bash
   docker exec <c> horizon-worker-tailnet agent git ls-remote https://github.com/example/secret.git; echo "exit=$?"
   docker exec <c> horizon-worker-tailnet agent /usr/bin/git config --global --includes --get http.https://github.com/.proxy
   ```

   Result: The first command shows `exit=0`. The second command shows
   `http://127.0.0.1:47281`: the binding goes through the Git proxy too.

8. Do step 7 of task C4 again with the path `example/secret.git`.

   Result: The command shows nothing. Git gets no token from the binding either.

9. Remove the binding:

   ```bash
   docker exec <c> horizon-worker-git-auth clear
   ```

   Result: The command exits with status 0.

### 6.9 R1: Request outside an agent session

1. Install a new chain. Do the steps 3 and 4 of task C2 again. The fake GitHub
   still runs.

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

3. Start the session. It calls the tool, then reads the repository with Git:

   ```bash
   docker exec <c> horizon-worker-tailnet agent tmux -L horizon-cloud new-session -d -s agent-smoke \
       'horizon-worker-github mcp < /workspace/home/mcp-in.jsonl > /workspace/home/mcp-out.jsonl; git ls-remote https://github.com/example/extra.git > /workspace/home/git.txt 2>&1; echo "exit=$?" >> /workspace/home/git.txt; sleep 600'
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

6. Wait 10 seconds. Then examine the tool output and the Git output:

   ```bash
   docker exec <c> cat /workspace/home/mcp-out.jsonl /workspace/home/git.txt
   ```

   Result: The tool output shows `Allowed for this cloud`. The Git output shows
   `exit=0` and no `remote: Horizon:` line.

7. Read the repository from a process outside the session:

   ```bash
   docker exec <c> horizon-worker-tailnet agent git ls-remote https://github.com/example/extra.git; echo "exit=$?"
   ```

   Result: The command shows `exit=0`. Access is per cloud, so a process outside
   the asking session gets through too. The status shows `example/extra` with
   `"target":null`.

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

   Result: `clear` shows nothing. The status JSON shows `"state":"absent"`.

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

7. Read the repository as the agent user. Then read it, and another
   repository, with `gh`:

   ```bash
   docker exec <c> horizon-worker-tailnet agent /usr/bin/git ls-remote https://github.com/<owner>/<name>.git
   docker exec <c> horizon-worker-tailnet agent sh -c 'cd /workspace && gh pr list -R <owner>/<name> \
       && gh api graphql -f query="{ repository(owner: \"cli\", name: \"cli\") { name } }"'
   ```

   Result: The first command shows the references of the synthetic repository.
   The second shows the pull requests of the synthetic repository (it can be an
   empty list), then fails with `GraphQL: Horizon: cli/cli has no GitHub grant
   on this worker`.

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
- Git fetches and pushes a granted repository through the Git proxy. The proxy
  adds the token only for a granted repository. A private repository without a
  grant and a push without a grant fail with a `remote: Horizon:` message.
- The socket gives Git no token. The `gh` wrapper gets only the placeholder
  `horizon-api-broker`, and finds the repository of a checkout without `GH_REPO`.
- `gh` reaches a granted repository through the API broker, which adds the
  token. A repository without a grant and a path outside a repository get a
  `Horizon:` refusal, and those requests do not reach GitHub. In lane G, the
  GraphQL policy refuses a repository without a grant.
- A static binding also goes through the Git proxy. Git gets no token from it.
- The agent user cannot read the token chain. No reply and no log line contains
  a token.
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
