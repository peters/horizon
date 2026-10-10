---
procedure: native-app-automate.md
candidate_base: 5b3de3c5ce21612bf306eb05c99d589b4631d78c
candidate_sha256: ebb633f41ead6d479a3a04eed407ad45c58209a2c58762e6f16fe2e30205af17
date: 2026-10-10
lanes: [linux-unit, linux-native-vnc]
issue: https://github.com/peters/horizon/issues/1373
---

# Native host evidence test report, 2026-10-10

## 1. Summary

The archive boundary tests and the Linux native viewer test passed. The runner
kept the first host error and marked later steps and recipes as blocked. The
viewer showed PASS, FAIL and BLOCKED in its caption and Connection details.
The combined driver candidate also retained evidence through 29 minutes of native
steps on each of two iPhones. That run reached its configured lifetime before
all steps and media exports completed. It did not qualify the full paid run or
the iOS deep-link change.

## 2. Results

| Test | Result | Note |
|---|---|---|
| Archive diagnosis | pass | The original archive held 125,437,084 bytes of PNG files against a 125,829,120-byte limit. The 392,036-byte space could not hold the next image. |
| Long-run evidence budget | pass | A deterministic test reserved 290 images at 2 MiB each. It admitted evidence above the old limit and refused evidence above 1 GiB. The terminal report still fit its separate reserve. |
| Archive exhaustion | pass | A simulated test driver remained open. The runner recorded the archive cause, stopped actions and image captures, and blocked all later steps. |
| Closed driver session | pass | A simulated test driver returned session closed. The runner retained that cause after a cleanup error, blocked later recipes and tried bounded provider media exports while the archive remained usable. |
| Invalid screenshot | pass | A simulated test driver returned an invalid image. The runner blocked later actions and kept four provider diagnostic captures because the archive remained usable. The unavailable network capture had an explicit error. |
| Failed reset | pass | A simulated test driver closed the original session before replacement failed. Three distinct causes blocked all later actions and image captures, including the next recipe. The report kept the original cause. |
| Allocation failure | pass | A simulated test driver refused session creation. The healthy progress callback still received every blocked recipe, in order, and the terminal lane event. No action or screenshot was attempted. |
| Failed progress sink | pass | The terminal report kept every recipe and blocked step after the progress callback failed. |
| Native viewer | pass | The frozen Linux candidate displayed all three result states through a public native Device viewer. |
| Paid iPhone endurance | incomplete | The combined driver candidate kept 219 valid screenshots, 476,578,360 bytes in total. Each lane passed more than 29 minutes of steps. The final steps and media exports reached the configured lifetime. The full endurance requirement remains open. |

Required local validation passed on the final source. The full workspace test
reported 5,549 passed tests, no failed tests and 45 ignored tests. Four filtered
fixture subprocess checks also passed. The speech test,
blocking Clippy and strict Clippy passed. The advisory pedantic tier reported
existing findings in unchanged Wayland, Chromecast and cloud tests.

## 3. Defects and limits

The evidence limit increased from 120 MiB to 1 GiB. Each archive also has an
8 MiB terminal-report reserve. Eight retained archives can use approximately
8.1 GiB. The file-count, image-size and provider-media limits remained in force.

A closed session has a typed cause. Other host lifecycle paths can still return
a generic unavailable cause. These paths need a separate focused change. The
runner stops unavailable lanes. This test did not prove provider uptime.

## 4. Deviations from the procedure

The native viewer used a synthetic RFB target. It published native result
metadata and a changing framebuffer. It did not allocate a remote device or
connect to an application backend. The test checked local status presentation.

The first progress-sink test fixture rejected the build event before it reached
the tested lane. The fixture was corrected to admit build and session creation.
The original failed tool result was kept. The corrected workspace test passed.

The first paid test used the archive-only candidate. The first ten-second hold
failed with a driver transport error on both lanes. Successful wait and assertion
steps kept their screenshots. Hold steps did not. The test was stopped because
it could not qualify the evidence requirement. The report recorded cancellation
and explicit unavailable media outcomes. Both sessions confirmed cleanup. Exact
owner reconciliation found no pending operation, and provider usage returned zero.

The combined driver candidate then passed the short gesture test on the same
isolated project and persistent owner. Each lane passed all four steps and kept
four screenshots plus its provider video. This test did not qualify endurance.

The 100-hold endurance plan then ran on the combined candidate. Its binary
SHA-256 was `61b890f2d8efef49bf0bcd7d0fd774add8d62c0a8273c708d148770f8be81fb3`.
One lane passed 110 steps and kept 110 screenshots. The other passed 109 steps
and kept 109 screenshots. Their passed step durations were 1,741,591 ms and
1,747,110 ms. All 219 image files passed size and PNG checks.

The run reached the 30-minute lifetime. Its last steps reported a wait timeout
and an expired operation. One remaining step was blocked. Provider media exports
reported explicit expiry errors. The upload cleanup reported an expired artifact.
Both session cleanups were confirmed. Exact reconciliation returned no pending
operation. Shared provider usage returned zero. This result proves retention above
the old archive limit. It does not prove a complete endurance plan.

After a host interruption, the raw progress files had incomplete byte ranges.
The terminal JSON report parsed correctly and its referenced image files were
valid. The reported durations came from that report, not from the incomplete
progress receipt timeline.

A later combined candidate passed the focused declared deep-link recipe on both
real iPhones. Independent public snapshots confirmed its language effect. The
host then rebooted during the endurance recipe. No terminal report was produced,
so this interrupted run did not qualify endurance. The retained progress receipts
and live-viewer observations were kept.

Exact reconciliation released both recorded provider sessions. Shared provider
usage returned zero. Local guardian receipts remained incomplete after the reboot,
and exact recovery refused them. The upload remained retained behind these
receipts. The original owner and journal were kept. No replacement was allocated.
This local recovery limitation needs a separate focused change.

After the reboot, one retained app-host test executable failed before test
execution, including with the test-list argument. The failed executable was kept.
Only its inactive package cache was rebuilt. The rebuilt host tests passed. An
unchanged process cleanup timing test then missed its 5.5-second bound in the
workspace run. Its serial retry passed in 4.15 seconds. The failed logs were kept.
The required full workspace suite was then run again and passed. After the reset
fix, the final full workspace suite passed 5,549 tests and four filtered fixture
subprocess checks, with no failed tests and 45 ignored tests.

## 5. Cleanup

The task-owned public Device viewer was closed. The isolated candidate was
closed with its title-bar control. All fixture processes exited and the private
device target expired. No pre-existing viewer or desktop was changed.

## 6. Evidence

Private validation logs and screenshots were kept in the task evidence directory.
The refreshed candidate used binary SHA-256
`ebb633f41ead6d479a3a04eed407ad45c58209a2c58762e6f16fe2e30205af17`.
The executable of the running child matched the frozen file. The first public
Reveal showed a displayed image at sequence 17. Three later public inspections
showed displayed image sequences 52, 63 and 71. The received image sequences were
71, 87 and 104. Each inspection returned a displayed image. The test did not
change later user navigation.

The refreshed native recording finalized with 1,036 encoded frames, 456 dropped
frames, 625 repeated frames and no encoder error. Its duration was 149.256 seconds.
The final GIF contained 46 frames at 1,000 by 625 pixels. Its duration was
15.33 seconds and its size was 222,722 bytes. Representative frames were decoded and checked before publication.
The GIF used generic labels and synthetic content only.
