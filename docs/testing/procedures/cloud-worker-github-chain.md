# Test procedure: the worker's GitHub token chain kinds

This procedure shows that a cloud worker serves a Git and `gh` token of either kind, that it
never renews an installation token, and that it stops serving one when it ends.

It runs inside one cloud worker and uses a fake GitHub on the loopback address, so it needs no
real GitHub token and no GitHub account. The host-side mint of an installation token is a
different procedure.

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

   Result: the command exits 0. `status` then holds `"kind": "person"` and `"renewal": "worker"`.

4. Prove a person chain renews.

   Wait for the access token to come close to its expiry, or install a chain whose access token
   ends in 120 seconds, then read `status` again.

   Result: the fake GitHub printed `REQUEST /login/oauth/access_token`, and `status` holds a new
   `access_expires_at`.

5. Install an installation chain, version 2.

   ```bash
   printf '%s' '{"version":2,"client_id":"Iv23synthetic","author_name":"synthetic[bot]","author_email":"synthetic[bot]@users.noreply.github.com","grants":[{"repository":"synthetic/repo","target":"primary","access":"push"}],"chain":{"access_token":"synthetic-installation-1","access_expires_at":<now+3600>,"kind":"installation"}}' | horizon-worker-github install
   ```

   Result: the command exits 0, and `status` holds `"kind": "installation"`, `"renewal": "host"`
   and `"state": "ok"`.

6. Prove an installation chain is never renewed.

   Install the same payload with an access token that ends in 120 seconds, then wait 60 seconds
   and read `status`.

   Result: the fake GitHub printed no `REQUEST` after step 1, and `status` still holds the same
   `access_expires_at`.

7. Prove the ended state.

   Wait until that access token has ended, then read `status`, and ask the Git credential
   helper, which is what hands Git a token:

   ```bash
   horizon-worker-github status
   printf 'protocol=https\nhost=github.com\n\n' | horizon-worker-git-auth get
   ```

   Result: `status` holds `"state": "installation_token_expired"`, and the reason says that only
   the host can install a new one. The helper answers with no `password=` line, so Git gets no
   token and no request can reach GitHub.

8. Prove the refusals.

   Install an installation chain whose `access_expires_at` is in the past. Result: the command
   exits non-zero and says the host must install a fresh one.

   Install a chain that holds `"kind": "installation"` together with a `refresh_token`. Result:
   the command exits non-zero and says the chain is invalid.

   Install `{"version": 1}` with installation fields, and `{"version": 2}` with person fields.
   Result: both are refused because the kind does not match the version.

9. Clear the chain.

   ```bash
   horizon-worker-github clear
   horizon-worker-github status
   ```

   Result: `status` holds `"state": "absent"`. Stop the fake GitHub.

## Evidence to keep

- The `status` answers of steps 2, 3, 5, 6, 7 and 9, with the `kind`, `renewal` and `state`.
- The fake GitHub's printed requests: one refresh for the person chain, none for the
  installation chain.
- The refusal messages of step 8.

## Limits

- The tokens are synthetic. This procedure does not test a real GitHub answer, and it does not
  test the host that mints an installation token.
- A real installation token ends after one hour. A worker that the host did not touch for longer
  than that has no GitHub access until the host installs a new token.
