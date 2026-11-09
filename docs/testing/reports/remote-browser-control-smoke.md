---
procedure: remote-browser-control-smoke
candidate_commit: d051d06d03d049d6e2d00ccda06901f7a31a3612
candidate_sha256: 30c3824f7af99a3d897b14bea454391d5e105996ecc67ad70799bb3fd9bc577a
date: 2026-11-09
lanes: [linux-local-container]
issue: https://github.com/peters/horizon/issues/1971
---

# Remote browser control smoke test report, 2026-11-09

## 1. Summary

One ordered run of the remote browser lane passed on Linux against the browser
machine on the same computer. The run covered 21 steps: the twelve calls of the
lane, the three refusals that the lane defines, and the checks that the page and
its files stay on the other machine.

The run found four defects before it could pass: the bridge looked for the remote
host under the home of the remote user, the host could record nothing, the host
could stage no attachment, and a start gave the display only one moment to greet.
All four were fixed in the same change and the run then passed.

## 2. Results

| Task ID | Result | Note | Defect |
|---|---|---|---|
| S1 | pass | 21 of 21 steps held, 5.2 s; three refusals with the codes below | — |
| S2 | pass | the screenshot, the recording and the capture stayed on the other machine | — |
| S3 | pass | the second run closed the page of the first and then passed | — |

The refusals of the run were `unsupported_backend` for a handoff of a page on
another machine, the refusal of a second page without the opt-in, which carries
no typed code, and `stale_reference` for a reference that the page does not have.

## 3. Defects

- Fixed in this change: the bridge read the runtime root of the remote host from
  `$HOME/.horizon`, so no container host was reachable. It now reads
  `$HORIZON_BROWSER_ROOT` first, which is the variable that the host itself uses.
- Fixed in this change: a host panel was refused `capture_retention` for its own
  recording and network capture. The host now writes them into a `captures`
  folder inside the page folder, which is the shape that retention accepts.
- Fixed in this change: an attachment over the bridge was refused
  `attachment_policy`, because the host allowed only the home of root. The image
  now names its staging folder in `HORIZON_BROWSER_ATTACHMENT_ROOTS`.
- Fixed in this change: a start reported `no VNC greeting` when the display of the
  container was still coming up. A start now retries the greeting for thirty
  seconds, as it already waited that long for the SSH server.

## 4. Deviations from the procedure

- Step 6.1 ran with `--profile probe`, which is the profile of the image, and not
  with a profile that the procedure writes.
- Task S3 used a one second timeout to force a run that stops early.

## 5. Cleanup

- The container `horizon-browser-vm-local`: stopped and removed.
- The state volume `horizon-browser-vm-local-state`: deleted.
- The image `horizon-browser-vm-local:latest`: deleted.
- The host block of `~/.ssh/config`: removed.
- The report folder `/tmp/remote-smoke`: removed after this report was written.

## 6. Evidence

- The report of the run named the commit and the SHA-256 of the binary above.
- The transcript held one line for each of the 22 calls of the run.
- The screenshot and the recording were files of the other machine, under
  `/state/browser-root/profile-host-captures/<page>/`.
