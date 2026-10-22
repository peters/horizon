---
procedure: remote-ios-evaluation
feature: Evaluation on a browser driver that reports no script timeout
platforms: [linux, macos, windows]
cost: paid device
destructive: no
secrets: [remote provider credentials, managed by the host]
owner: peters
---

# Remote iOS evaluation test procedure

## 1. Purpose

This procedure proves that `browser_evaluate` works on a browser driver that
reports no script timeout. It also proves that a returned promise settles, that
a rejection and an unsettled promise give a typed failure, and that the caller
bound covers the whole evaluation.

## 2. Applicability

- Candidate: a `horizon-browser` build with the evaluation contract of
  [the remote browser document](../../architecture/remote-browser-sessions.md#evaluation-contract).
- Platforms: the host can run on any supported platform. The browser must run on
  the selected physical device.
- This procedure does not test: local browsers, the Horizon desktop panel, cloud
  natives, network capture or file transfer.

## 3. Safety

> **CAUTION:** OPEN ONLY ONE SESSION AT THE PAID PROVIDER AT A TIME. Each session
> uses paid device time. Close it before you open the next session.

> **CAUTION:** PUT NO CREDENTIAL VALUE INTO A COMMAND, A LOG OR A REPORT. Name
> only the private file that holds it.

## 4. Equipment and preconditions

- The frozen candidate `horizon-browser` binary, with its SHA-256 recorded.
- One remote provider account, with its username and access key in two private
  files. The mode of each file is 0600.
- One host file that names one provider, the Safari and Chrome targets of the
  physical iOS device, one physical Android target, and the two credential files.
  The mode of the host file is 0600.
- An empty runtime root, for example `HORIZON_BROWSER_ROOT=/var/tmp/rie/root`.
- A generic page on a public HTTPS host. The
  [Selenium web form](https://www.selenium.dev/selenium/web/web-form.html) is the
  reference page. Record the page that you use.

## 5. Setup

1. Run the page-script check.

   ```bash
   node --test scripts/browser-smoke/evaluation_handshake.test.cjs
   ```

   Result: The ten checks pass. The expression runs once, a rejection reason
   that cannot be printed is still reported, an await without the kept promise
   reports a lost document, and a page that keeps the promise global gives a
   typed failure.

2. Start the standalone remote host of the candidate.

   ```bash
   horizon-browser mcp --standalone --config <host file>
   ```

   Result: The host answers `initialize`. `browser_provider_usage` shows the
   provider with an allowed session count. No session is open.

## 6. Tasks

### 6.1 RIE-IOS: Physical iPhone

1. Call `browser_create` with the iOS Safari target and the reference page.
   Record the device and the operating system from `remote_device`.

   Result: The provider confirms a physical device.

2. Call `browser_evaluate` with `[document.title, location.href]`.

   Result: The result names the reference page. The evaluated document is the
   page you selected.

3. Call `browser_evaluate` with the constant `1+1`.

   Result: The result is `2`. No `invalid_result` and no `WebDriver returned no
   script timeout` occurs.

4. Call `browser_evaluate` with the object `({answer: 42})`.

   Result: The result is `{"answer": 42}`.

5. Call `browser_evaluate` with `Promise.resolve(42)`.

   Result: The result is `42`. An empty object is a failure.

6. Call `browser_evaluate` with `Promise.reject(new Error('synthetic rejection'))`.

   Result: The action fails with `javascript_error`, and the message contains
   `promise rejected: synthetic rejection`.

7. Call `browser_evaluate` with `Promise.reject(Object.create(null))`.

   Result: The action fails with `javascript_error` and not with
   `evaluation_timeout`. The rejection reason cannot be printed, so the message
   names an unprintable value.

8. Call `browser_evaluate` with
   `new Promise(r => setTimeout(() => r(7), 800))`.

   Result: The result is `7`.

9. Call `browser_evaluate` with `new Promise(() => {})` and
   `timeout_millis: 4000`.

   Result: The action fails with `evaluation_timeout` after about 4000 ms. The
   message names the 4000 ms bound.

10. Call `browser_evaluate` with
    `new Promise(r => setTimeout(() => r(7), 6000))` and `timeout_millis: 3000`.

   Result: The action fails with `evaluation_timeout` after about 3000 ms. It
   does not wait for the 6000 ms promise.

11. Call `browser_close`, then `browser_remote_allocations` with `operation: list`.

    Result: The allocation of this session is not `in_use`.

12. Do steps 1 to 11 again with the iOS Chrome target.

    Result: Every result is the same.

### 6.2 RIE-ANDROID: Physical Android

1. Do step 1 of RIE-IOS with the Android Chrome target.

   Result: The provider confirms a physical device.

2. Do steps 2 to 8 of RIE-IOS.

   Result: Every result is the same. The rejections in steps 6 and 7 come from
   the driver as `javascript_error`.

3. Call `browser_close`, then `browser_remote_allocations` with `operation: list`.

   Result: The allocation of this session is not `in_use`.

### 6.3 RIE-DESKTOP: Local desktop engine

1. Run the regression checks of the crate.

   ```bash
   cargo test -p horizon-browser --lib
   ```

   Result: The tests pass. `a_reported_script_bound_keeps_the_plain_sync_script`
   passes, so a driver that reports its bound gets the unchanged script.

2. Do steps 2 to 8 of RIE-IOS on one local panel with `browser_evaluate`.

   Result: Every result is the same.

## 7. Pass criteria

- Every result in section 6 is as written.
- No answer of a tool and no file except the two credential files holds a
  credential value.
- No session stays allocated after its close.

## 8. Cleanup

1. Close every owned session and close the standard input of the host.

   Result: The host exits, and every session is released.

2. Remove the host file, the two credential files and the runtime root.

   Result: No private file stays on the computer.

## 9. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). If not, put the results in the
pull request. Record the commit, the SHA-256 of the candidate, the device, the
operating system and the browser. Keep private evidence out of the repository.
