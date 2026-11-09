---
procedure: remote-browser-control-smoke
candidate_commit: 9752e96d9b52cbd371cfb385a1bcd3d4b8172abd
candidate_sha256: 15a93d349bebc7f31ff71cd5722c6d8da7c7a9a1bb8104ebce932a895fbdfb41
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

The run found two defects before it could pass: the bridge looked for the remote
host under the home of the remote user, and the host could record nothing and
stage no attachment. Both were fixed in the same change and the run then passed.

## 2. Results

| Task ID | Result | Note | Defect |
|---|---|---|---|
| S1 | pass | 21 of 21 steps held, 5.2 s; three refusals with the codes below | — |
| S2 | pass | the screenshot, the recording and the capture stayed on the other machine | — |
| S3 | pass | the second run closed the page of the first and then passed | — |

The refusals of the run were `unsupported_backend` for a handoff of a page on
another machine, the opt-in refusal for a second page, and `stale_reference` for
a reference that the page does not have.

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
