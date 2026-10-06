---
procedure: <procedure file name>
candidate_commit: <full commit SHA>
candidate_sha256: <SHA-256 of the frozen candidate>
date: <YYYY-MM-DD>
lanes: [<lane>, <lane>]
issue: <issue URL>
---

# <Feature> test report, <YYYY-MM-DD>

Write this report with the STE descriptive rules. Use the simple past tense for
what happened. Delete this paragraph in a real report.

## 1. Summary

<Two to four sentences. Say what passed, what failed and what you did not test.>

## 2. Results

| Task ID | Result | Note | Defect |
|---|---|---|---|
| <ID> | pass, fail, blocked or not run | <Short note> | <Issue link or —> |

Use `not run` only in an interim report, for a test that the run did not do yet.
A final report has no `not run` row.

## 3. Defects

- <Issue link>: <one sentence that tells the symptom>.

## 4. Deviations from the procedure

- <Step that you changed or did not do, and the reason.>

## 5. Cleanup

- <Resource>: <deleted, kept, and why>.

## 6. Evidence

<Say where the private evidence is. Do not put secrets, private hosts or customer
data in this file.>
