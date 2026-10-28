# Test procedure: the person chain of an agent's cloud lasts

This procedure shows two properties of the GitHub access that an agent's cloud holds:

- The worker renews the chain itself, before its access token ends.
- Git and `gh` work in the worker while Horizon does not run on the person's computer.

Both properties need a real cloud, and one property needs a wait of at least one access
token's lifetime, which is eight hours. The procedure therefore takes hours, and its steps
can run in the evening and the next morning.

## Prerequisites

- Horizon on this computer with a GitHub App in Cloud settings, as
  [Connect GitHub](connect-github.md) leaves it.
- An `agent_lifecycle` policy in Cloud settings that lists one repository whose origin is a
  GitHub repository, and `allow_github_sign_in_skip: false`.
- The private state directory of a Horizon instance for this test, such as the one that
  [the isolated desktop](../procedures/device-smoke-extra-panels.md) prepares. Use its own
  state, because this procedure stops that instance.
- The browser of this computer is signed in to GitHub as the account of the app, for the one
  sign-in that the create makes.
- A shell on this computer that can read the state directory.

> **CAUTION:** THIS PROCEDURE RENTS A CLOUD AND LEAVES IT RUNNING OVERNIGHT. IT COSTS ABOUT
> 10 CENTS FOR 12 HOURS ON A HETZNER `cx23`. DELETE THE CLOUD BEFORE YOU STOP THE HORIZON
> INSTANCE, OR THE DEPLOYMENT KEY AND THE CLOUD RECORD ARE LOST WITH ITS STATE DIRECTORY.

## Steps

1. Note the frozen binary and its hash.

   Result: the test names one executable in its evidence, for example
   `/var/tmp/horizon-run/bin/horizon-lane4` with the SHA-256 of that file.

2. Start the instance with the state directory, and create one cloud with an agent's
   `cloud_create`. Poll its `status`.

   Result: `lines` gains a line that starts `GitHub sign-in needed:` and holds either a
   `https://github.com/login/oauth/authorize` address or a device code. The create waits.

3. Approve the sign-in in the browser of this computer: open that address, or enter the code
   at `https://github.com/login/device`.

   Result: the create continues without another request, and the cloud reaches `ready`.

4. Copy the deployment key and the cloud record out of the state directory, so that step 7
   can reach the worker after the instance stops.

   ```bash
   STATE=<state directory>
   cp -a "$STATE/data/creds/ed25519" /var/tmp/lane/ed25519
   cp -a "$STATE/data/home/.horizon/cloud" /var/tmp/lane/cloud-record
   ```

   Result: both copies exist. The key has mode 0600.

5. Read the chain status in the worker, as root, and note the three times.

   ```bash
   ssh -i /var/tmp/lane/ed25519 -o IdentitiesOnly=yes root@<worker address> \
     'horizon-worker-github status'
   ```

   Result: `"kind": "person"`, `"renewal": "worker"`, `"serving": true`, and
   `access_expires_at` is eight hours after the sign-in. `last_refresh_at` is `null`.

6. Wait until `access_expires_at` is less than 30 minutes away, then read the status again
   every 15 minutes.

   The margin is the worker's own: it renews when the access token is within 30 minutes of
   its end.

   Result: one reading has `last_refresh_at` set, and `access_expires_at` has moved eight
   hours later. `last_error` stays `null`.

7. Push from the worker while Horizon does not run.

   a. Stop the Horizon instance of step 2 with the signal that its own launcher names.
   b. Confirm that no process of that binary runs.
   c. In the worker, as root, run one push and one `gh` command as the agent user:

   ```bash
   ssh -i /var/tmp/lane/ed25519 -o IdentitiesOnly=yes root@<worker address> \
     'setpriv --reuid=10001 --regid=10001 --init-groups env HOME=/workspace/home \
        PATH=/usr/local/bin:/usr/bin:/bin sh -c \
        "cd /workspace/checkout && git checkout -B no-horizon && \
         printf \"# No Horizon\n\" > NO-HORIZON.md && git add NO-HORIZON.md && \
         git commit -m \"docs: push with no Horizon running\" && \
         git push -u origin no-horizon && \
         gh pr create --head no-horizon --title \"docs: push with no Horizon running\" \
           --body \"Pushed while Horizon was stopped.\""'
   ```

   The agent's Git configuration lives in `/workspace/home`, and `HOME` must name it. A push
   without it asks for a user name, because the credential helper is not found.

   Result: the push succeeds, the commit and the pull request name the account of the chain,
   and no Horizon process exists on this computer.

8. Delete the cloud at its provider, and prove that nothing remains.

   ```bash
   TOKEN=$(cat "$HOME/.horizon/cloud/credentials/hetzner-kpTX7W")
   curl -s -H "Authorization: Bearer $TOKEN" \
     "https://api.hetzner.cloud/v1/servers/<server id>"
   curl -s -X DELETE -H "Authorization: Bearer $TOKEN" \
     "https://api.hetzner.cloud/v1/servers/<server id>"
   ```

   Result: the first call answers `200` and the delete answers `200`. A further call answers
   `404`, and the list of servers of the account holds no server of this cloud.

   A run that keeps its instance running uses the card of the cloud, which deletes it through
   `cloud_delete` instead. Delete the cloud **before** step 7a in that case.

9. Remove the copies of step 4.

   Result: `/var/tmp/lane` holds no private key and no cloud record.
