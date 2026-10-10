---
procedure: native-ios-deep-links.md
candidate_base: 12b73dd5899e578b71208a0c73c1110824817bdb
candidate_sha256: 7e491597ade7a8f786ceec324c42eed3217d2410ea06c1457f03a3674e767069
date: 2026-10-10
lanes: [linux-unit, real-ios]
issue: https://github.com/peters/horizon/issues/1373
---

# Native iOS deep link test report, 2026-10-10

## 1. Summary

The declared deep-link recipe passed on two real iPhones through App Automate.
Both native hosts stayed available for more than 22 minutes of recipe steps.
Each lane passed all 106 steps and kept a PNG file for every step. Exact owner
reconciliation found no pending operation. Provider usage returned zero running
sessions and zero queued sessions, with a quota of two.

The three acceptance conditions in issue 1373 passed with the merged host change
and this driver candidate. Complete provider media export did not pass: one lane
returned a typed video-unavailable result. The wider requirement to retain every
host lifecycle cause still needs the separate lifecycle change.

## 2. Candidate and setup

The source started from the host change merged as
`12b73dd5899e578b71208a0c73c1110824817bdb`. The reviewed driver patch had SHA-256
`f938fede5eda9560eb11861fe3b0ec6724289184846e3319766b8171a94614f9`.
The default Linux executable had SHA-256
`7e491597ade7a8f786ceec324c42eed3217d2410ea06c1457f03a3674e767069`.
The running native controller matched that frozen executable. Its source files
did not change during the run.

The test used one declared immutable Debug IPA with SHA-256
`119fca377c086d4f2166f300c7c1599f24f4ee3d4fdb6bd73e464051fd3b35dc`.
Each lane used its own isolated synthetic backend. The original owner, private
client file and shared journal were kept. No app source was changed.

The seven-step deep-link recipe opened a declared German-language URL, checked
the home button, captured the screen, opened the declared language-reset URL,
checked the home button again and captured the screen. The retained images from
both lanes showed the German button label before reset and the English label
after reset. Thus the evidence showed the app effect as well as command success.

The 99-step endurance recipe used 88 bounded ten-second holds on an inert home
element, with periodic home assertions. The host executed the plan automatically.
The test did not use intentional wait failures or remove existing app assertions.

## 3. Results

| Check | iPhone 15, iOS 27 | iPhone 12 Pro, iOS 18 |
|---|---|---|
| Deep-link recipe | 7 of 7 steps passed | 7 of 7 steps passed |
| Endurance recipe | 99 of 99 steps passed | 99 of 99 steps passed |
| Sum of measured step durations | 1,347,690 ms | 1,333,275 ms |
| Host continuity | One session, no reset or host error | One session, no reset or host error |
| Step screenshots | 106 valid PNG files | 106 valid PNG files |
| Screenshot bytes | 237,157,775 | 221,432,804 |
| Blocked steps | None | None |
| Session cleanup | Confirmed | Confirmed |
| Provider video | Retained and decoded | Explicitly unavailable |

The measured step durations are a lower bound on each session's duration. They
exclude allocation and final media export. Both bounds exceed 1,200,000 ms.
The 212 retained PNG files passed size, PNG-signature and image-decoder checks.
Their combined size was 458,590,579 bytes, above the former 120 MiB archive limit.
Every file has a private copy and a SHA-256 receipt.

The retained provider video was 18,747,262 bytes and 1,375.828 seconds long.
It decoded as H.264 at 380 by 824 pixels. Frames at 25 and 50 seconds showed the
German-to-English change. A frame at 900 seconds showed the app during endurance.
The second lane returned `app_media_unavailable`: provider evidence was disabled,
not finalized or unavailable. The report kept that error separately from the
successful steps and confirmed cleanup.

The [matrix procedure](../procedures/native-app-automate.md) requires each retained
video to be decoded. Its interactive video task permits a retained video or a
typed unavailable result. The [native runbook](../../architecture/remote-device-testing.md)
limits video-finalization polling to 30 seconds and keeps a pending result as an
explicit failure. No video was claimed for the second lane. The controller had
closed, so its in-memory provider reference could not support a later public
download. No session was renewed or allocated to obtain media.

## 4. Local validation

All required local validation passed in the final worktree. The full workspace
test passed 5,649 tests and four filtered fixture subprocess checks, with no
failed tests and 47 ignored tests. The host package passed 100 tests. The native
driver package passed 77 tests. The default build, format, maintainability, skill
coverage, speech, blocking Clippy and strict Clippy checks passed.

The advisory pedantic tier reported existing findings in unchanged dependency
tests. A separate native-package check without dependency linting reported three
existing findings in unchanged test code. Comparison with the base confirmed
that this change did not introduce or worsen those findings.

The merged host regression `forced_host_loss_blocks_later_recipes_and_retains_the_initial_cause`
also passed in this candidate. It used a simulated test driver. The first cause
remained in the lane report after a cleanup error. Later actions stopped and the
next recipe was blocked. The archive-failure regression also checked that the
step, lane and single `lane_blocked` progress event had the same first cause,
with no later action or capture. These deterministic failure tests are separate
from the real-device endurance proof.

The driver tests checked bounded, redacted errors, unsupported commands,
transport failures without URL replay, recognized Open confirmation, unrelated
alerts, slow negative alert replies and a positive reply after the poll window.
The original session deadline still bounded all confirmation requests.

## 5. Live viewer evidence

Each lane had an owned public native Device viewer. The first lane's initial
Reveal produced a displayed image. The second lane was already displayed and
needed no Reveal. Three inspections at least two seconds apart showed displayed
frame sequences 27, 30 and 32 on the first lane, and 22, 24 and 27 on the second.
The native recipe steps advanced during these observations.

Later inspections showed connected streams and advancing received frames while
the viewers were clipped after canvas navigation. Those later observations were
not counted as displayed images. The test preserved that navigation.

After terminal evidence was copied, the first owned viewer closed through the
public tool. The second viewer was already absent from the workspace. The final
public viewer list was empty. Closing a viewer did not terminate a native target.

## 6. Cleanup and limits

Both session cleanups were confirmed. Upload cleanup had no error. Exact
reconciliation returned an empty pending list. The read-only provider catalog
reported zero running sessions, zero queued sessions and a quota of two.

An initial cleanup catalog command included an unsupported client argument and
returned a generic unavailable error. The failed receipts were kept. The
documented catalog command without that argument passed. It did not allocate.

Earlier functional app recipes had label and reference assertion failures. They
were kept as separate follow-up evidence. This focused recipe used declared
identifiers and retained images to test the actual URL effect. This report does
not claim that every original functional app recipe passed.

The host recognizes the expected English Open confirmation only. Other system
text remains a typed refusal. The provider version selection and deep-link
runtime requirements are documented in the [procedure](../procedures/native-ios-deep-links.md).
Generic host lifecycle causes and recovery after a machine restart are separate
changes. This report does not close those requirements.

## 7. Evidence

The private evidence includes the source and executable manifests, immutable IPA
hash, public MCP request and response receipts, progress timestamps, per-step
PNG hashes, retained media hashes, decoded frames, viewer inspections and exact
cleanup receipts. App pixels, raw logs, private paths, owner identifiers and
provider references remain private.
