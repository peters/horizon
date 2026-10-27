# Test procedure: the worker's person GitHub token chain

This procedure shows that a cloud worker accepts a person's GitHub token chain, serves its
access token to Git, and renews the chain itself from its refresh token.

It runs inside one cloud worker and uses a fake GitHub on the loopback address, so it needs no
real GitHub token and no GitHub account. The sign-in that makes a real chain is a different
procedure.

## Prerequisites

- One cloud worker from the image under test, at `Ready`.
- A shell in that worker as root, for example through the host's worker access.
- No chain installed: run step 2 to check, and run `horizon-worker-github clear` if one is.

## Steps

1. Start the fake GitHub.

   In the worker, as root:

   ```bash
   python3 - <<'PY' &
   import http.server, json
   class Fake(http.server.BaseHTTPRequestHandler):
       def do_POST(self):
           body = json.dumps({"access_token": "synthetic-access-2", "expires_in": 28800,
                              "refresh_token": "synthetic-refresh-2", "refresh_token_expires_in": 15897600})
           self.send_response(200); self.send_header('Content-Type', 'application/json'); self.end_headers()
           self.wfile.write(body.encode())
       def log_message(self, *args):
           print('REQUEST', self.path, flush=True)
   http.server.HTTPServer(('127.0.0.1', 8099), Fake).serve_forever()
   PY
   ```

   Then set `HORIZON_WORKER_GITHUB_TEST_URL=http://127.0.0.1:8099` in the shell that runs the
   service commands below.

2. Check the empty state.

   ```bash
   horizon-worker-github status
   ```

   Result: the answer holds `"state": "absent"`.

3. Install a person chain, version 1.

   ```bash
   printf '%s' '{"version":1,"client_id":"Iv23synthetic","login":"synthetic","author_name":"Synthetic","author_email":"synthetic@example.invalid","grants":[{"repository":"synthetic/repo","target":"primary","access":"push"}],"chain":{"access_token":"synthetic-access-1","access_expires_at":<now+3600>,"refresh_token":"synthetic-refresh-1","refresh_expires_at":<now+86400>}}' | horizon-worker-github install
   ```

   Result: the command exits 0. `status` then holds `"state": "ok"`, `"login": "synthetic"`,
   and `synthetic/repo` in `repositories`.

4. Prove the worker serves the token to Git.

   ```bash
   printf 'protocol=https\nhost=github.com\npath=synthetic/repo\n\n' | horizon-worker-git-auth get
   ```

   Result: the answer holds `username=x-access-token` and one `password=` line, which is the
   access token of the chain.

5. Prove the chain renews.

   ```bash
   horizon-worker-github status
   ```

   Wait for the access token to come close to its expiry, or install a chain whose access token
   ends in 120 seconds, then read `status` again.

   Result: the fake GitHub printed `REQUEST /login/oauth/access_token`, and `status` holds a new
   `access_expires_at` and a new `refresh_expires_at`.

6. Prove the refusals.

   Install a chain whose `refresh_expires_at` is in the past. Result: the command exits non-zero
   and says that the token chain expired.

   Install the same payload with `"version": 2`. Result: the command exits non-zero and says
   that the version is not supported.

   Install the same payload with `"kind": "person"` in the chain. Result: the command exits
   non-zero and says that the token chain is invalid.

   Install the same payload with the login `synthetic login`. Result: the command exits non-zero
   and says that the GitHub login is invalid.

   Result: no refusal prints the access token or the refresh token.

7. Clear the chain.

   ```bash
   horizon-worker-github clear
   horizon-worker-github status
   ```

   Result: `status` holds `"state": "absent"`. Stop the fake GitHub.

## Evidence to keep

- The `status` answers of steps 2, 3, 5 and 7, with the `state` and the repositories.
- The fake GitHub's printed requests, with the one refresh of step 5.
- The refusal messages of step 6.

## Limits

- The tokens are synthetic. This procedure does not test a real GitHub answer.
- A real chain needs a person's sign-in. This procedure does not show which repositories
  GitHub grants that person, and it does not test the app's installation on an account.
- The worker renews the chain while its service runs. A worker whose volume does not keep
  private permissions falls back to memory, and a restart of that worker has no chain.
