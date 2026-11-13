---
procedure: ssh-upload-cancel
feature: SSH upload, cancel of one upload
platforms: [linux, darwin]
cost: none
destructive: no
secrets: none
owner: peters
---

# SSH upload cancel test procedure

## 1. Purpose

This procedure tests the cancel of an upload over SSH. It tests the button
`Cancel Upload`, the state `Upload cancelled`, the number of files that were
whole, the files that stay on the far host, and the link of the panel after the
cancel.

## 2. Applicability

- Use a current Horizon candidate on Linux or macOS.
- Use one SSH server on the loopback address of this computer, and one file that
  is large enough that the upload lasts a few seconds.
- This procedure does not test a completed upload, the destination picker of the
  upload, or Taildrop.

## 3. Safety

> **CAUTION: USE ONLY THE LOOPBACK SSH SERVER AND THE STATE FOLDER OF THIS RUN.**
> The server of this procedure listens on `127.0.0.1` and starts one container.
> Do not upload to an account or a computer that you need.

> **CAUTION: ADD ONLY THE KEY OF THE TEST HOST TO `~/.ssh/known_hosts`.** A wrong
> entry can make other hosts unreachable.

## 4. Equipment and preconditions

- A frozen candidate `horizon` binary and its SHA-256.
- Docker, `ssh-keygen` and Python 3.
- One free loopback port for the server, for example `2222`.
- One state folder of this run, for example `$HOME/horizon-ssh-upload-cancel`.
- About 2 GB of free disk space for the file of the test.
- No `known_hosts` entry for `[127.0.0.1]:2222`.

## 5. Setup

1. Make the folder of the keys and the host key.

   ```sh
   export STATE="$HOME/horizon-ssh-upload-cancel"
   mkdir -p "$STATE/home" "$STATE/files"
   chmod 700 "$STATE/home"
   ssh-keygen -q -t ed25519 -N '' -C upload-host -f "$STATE/home/host_key"
   ssh-keygen -q -t ed25519 -N '' -C upload-client -f "$STATE/home/client_key"
   ```

   Result: The folder holds four key files.

2. Start the server on the loopback address, with one user of the test.

   ```sh
   docker run -d --name ssh-upload --network host \
     -v "$STATE/home":/keys:ro alpine:3 sh -c 'apk add --no-cache openssh-server >/dev/null && \
       cp /keys/host_key /etc/ssh/ssh_host_ed25519_key && chmod 600 /etc/ssh/ssh_host_ed25519_key && \
       adduser -D -s /bin/sh lab && mkdir -p /home/lab/.ssh /home/lab/incoming && \
       cp /keys/client_key.pub /home/lab/.ssh/authorized_keys && \
       chmod 700 /home/lab/.ssh && chmod 600 /home/lab/.ssh/authorized_keys && \
       chown -R lab /home/lab/.ssh /home/lab/incoming && mkdir -p /run/sshd && \
       exec /usr/sbin/sshd -D -e -p 2222 -o ListenAddress=127.0.0.1'
   ```

   Result: `docker logs ssh-upload` holds `Server listening on 127.0.0.1 port 2222`.

3. Trust the key of the test host out of band.

   ```sh
   printf '[127.0.0.1]:2222 %s\n' "$(cut -d' ' -f1,2 "$STATE/home/host_key.pub")" >> ~/.ssh/known_hosts
   ```

   Result: `ssh-keygen -F '[127.0.0.1]:2222'` prints the entry.

4. Make the SSH host of this run.

   ```sshconfig
   Host upload-1
       HostName 127.0.0.1
       Port 2222
       User lab
       IdentityFile ~/horizon-ssh-upload-cancel/home/client_key
       IdentityAgent none
   ```

   Result: `ssh -T -o BatchMode=yes upload-1 'echo ok'` prints `ok`.

5. Make the file of the test: 2 GB of zeros, so that one upload of a loopback
   link lasts a few seconds and a person has time to cancel it.

   ```sh
   dd if=/dev/zero of="$STATE/files/big.bin" bs=1M count=2048
   ```

   Result: `ls -l "$STATE/files/big.bin"` holds about 2 GB.

6. Measure one upload of that file over the loopback link, so the cancel has time
   to happen:

   ```sh
   time ssh -T upload-1 'cat > /home/lab/incoming/probe.bin' < "$STATE/files/big.bin"
   ```

   Result: The command prints a time. When that time is less than two seconds,
   make the file larger with step 5 and measure again. Remove the probe:

   ```sh
   ssh -T upload-1 'rm -f /home/lab/incoming/probe.bin'
   ```

## 6. Tasks

### 6.1 SSH-UP-CANCEL-01 — An upload that runs

1. Start the candidate with a private home of this run, open **Menu**, then
   **Remote Hosts**, and open a panel of `upload-1`. On Linux, start the
   candidate through
   [the device smoke fixture](../../../scripts/device-smoke/README.md#horizon-inside-a-native-vnc-device-panel)
   and watch the window in a Device panel.

   Result: The panel reaches a shell prompt.

2. Drop the file `$STATE/files/big.bin` on the body of that panel.

   Result: The upload window opens on the panel.

3. Set **Remote destination** to `/home/lab/incoming`, and press **Start Upload**.

   Result: The state `Uploading...` shows a progress line, and the number of bytes
   grows.

4. Read the bottom of the window.

   Result: It holds the button `Cancel Upload`.

### 6.2 SSH-UP-CANCEL-02 — Cancel ends the upload

1. Press `Cancel Upload` while the progress line is still moving.

   Result: Within about one second the state becomes `Upload cancelled`, and the
   line under it says `Cancelled after 0 of 1 files`.

2. Read the folder of the destination on the far host.

   ```sh
   docker exec ssh-upload ls -a /home/lab/incoming
   ```

   Result: The folder holds neither `big.bin` nor a file whose name starts with
   `.horizon-upload-`. The far side removes its temporary file when the link
   ends.

3. Read the panel under the window.

   Result: The panel keeps its shell: the upload runs `ssh` children of its own,
   and the cancel ended only them. No strip appears at the bottom of the grid.

4. Type `echo after-cancel` in the panel and press Enter.

   Result: The panel prints `after-cancel`. The link of the panel did not end
   with the upload.

### 6.3 SSH-UP-CANCEL-03 — The window returns to a ready state

1. Press `Done` in the upload window, and drop `$STATE/files/big.bin` on the
   panel again.

   Result: The upload window opens again, and it offers the destination
   `/home/lab/incoming` of the previous run.

2. Press `Start Upload`, and wait until the state says `Upload complete`.

   Result: The line says `Uploaded 1 file(s)`, and
   `docker exec ssh-upload ls -l /home/lab/incoming/big.bin` shows the file of
   about 2 GB, with the size of `$STATE/files/big.bin`. A cancel leaves no state
   behind that blocks the next upload.

## 7. Pass criteria

- `Cancel Upload` ends an upload in about one second.
- The window shows `Upload cancelled` and the number of files that were whole.
- The far host holds no file of the cancelled upload, and no temporary file.
- The link of the panel of the upload stays, and the next command of that shell
  runs.
- The next upload of the same file completes.

## 8. Cleanup

1. Remove the container of this run.

   ```sh
   docker rm -f ssh-upload
   ```

   Result: The container is gone.

2. Remove the entry of the test host from `known_hosts`, and the two hosts of the
   SSH configuration.

   ```sh
   ssh-keygen -R '[127.0.0.1]:2222'
   ```

   Result: The command removes the entry and writes a backup file beside it.

3. Remove the folder of this run and the private Horizon home of the candidate.

   Result: No folder of this run is left.

## 9. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). If not, put the results in the
pull request. Keep private evidence out of the repository.
