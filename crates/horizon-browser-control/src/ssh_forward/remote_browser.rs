//! The two `sh` snippets that this computer runs on the browser machine.
//!
//! One starts the Chromium of a profile folder, detached so that a signed-in
//! page outlives this computer, and prints the endpoint that Chromium writes
//! into that folder. The other reads the endpoint of a browser that already
//! runs, so that a connect can reuse that browser instead of starting a second
//! one.
//!
//! The far side needs `sh`, `cat`, `mkdir`, `nohup` and a Chromium program. No
//! Horizon program, record file or interactive prompt takes part.

use super::transport::quote;

/// The folder of the browser profiles of the remote machine, under its home.
const REMOTE_PROFILE_ROOT: &str = "\"$HOME/.horizon/browser-profiles/\"";
/// The file that Chromium writes its `DevTools` port and browser path into.
const DEVTOOLS_ACTIVE_PORT: &str = "DevToolsActivePort";
/// The browser start polls its endpoint file this often, this many times.
const BROWSER_READY_ATTEMPTS: u32 = 20;
const BROWSER_READY_INTERVAL_SECONDS: u32 = 1;
/// The exit code of the start snippet when that machine has no Chromium.
const NO_BROWSER_EXIT: i32 = 3;
/// The exit code of the start snippet when it could not make the profile folder.
const NO_PROFILE_FOLDER_EXIT: i32 = 4;
/// The exit code of the start snippet when the browser wrote no endpoint.
const NO_ENDPOINT_EXIT: i32 = 5;
/// The programs that serve a Chromium `DevTools` port, in the order of use.
pub(super) const REMOTE_BROWSER_NAMES: [&str; 6] = [
    "google-chrome",
    "google-chrome-stable",
    "chromium",
    "chromium-browser",
    "brave-browser",
    "microsoft-edge",
];
/// The bundle paths of that same list on macOS, where a browser is not on the
/// `PATH` of a non-interactive login.
pub(super) const REMOTE_BROWSER_INSTALLATIONS: [&str; 4] = [
    "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
    "/Applications/Chromium.app/Contents/MacOS/Chromium",
    "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
    "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser",
];

/// Start the Chromium of `profile` on the remote machine, detached, and print
/// the `DevTools` endpoint that it writes into the profile folder.
///
/// The endpoint file is removed first, so an endpoint of a browser that is gone
/// never answers for the browser that this snippet starts. The caller reads the
/// endpoint that a running browser wrote with [`super::remote_port_command`]
/// before it runs this snippet; that is what makes a browser outlive a panel.
///
/// The exit codes name what a person must change ([`NO_BROWSER_EXIT`],
/// [`NO_PROFILE_FOLDER_EXIT`], [`NO_ENDPOINT_EXIT`]).
#[must_use]
pub fn remote_browser_command(profile: &str, headless: bool) -> String {
    let window_switch = if headless {
        "--headless=new"
    } else {
        "--no-startup-window"
    };
    let names = REMOTE_BROWSER_NAMES.map(quote).join(" ");
    let installations = REMOTE_BROWSER_INSTALLATIONS.map(quote).join(" ");
    format!(
        r#"set -u
profile_dir={root}{profile}
active="$profile_dir/{active_port}"
rm -f "$active"
umask 077
mkdir -p "$profile_dir" || exit {no_folder}
binary=
for name in {names}; do
  found=$(command -v "$name" 2>/dev/null) || found=
  if [ -n "$found" ]; then
    binary=$found
    break
  fi
done
if [ -z "$binary" ]; then
  for candidate in {installations}; do
    if [ -x "$candidate" ]; then
      binary=$candidate
      break
    fi
  done
fi
if [ -z "$binary" ]; then
  printf '%s\n' 'this machine has no chromium program' >&2
  exit {no_browser}
fi
cd "$profile_dir" || exit {no_folder}
nohup "$binary" --remote-debugging-port=0 --remote-debugging-address=127.0.0.1 --user-data-dir="$profile_dir" {window_switch} --no-first-run --no-default-browser-check --disable-features=TranslateUI about:blank >>"$profile_dir/chromium.log" 2>&1 </dev/null &
tries=0
while [ ! -s "$active" ]; do
  tries=$((tries + 1))
  if [ "$tries" -ge {attempts} ]; then
    printf '%s\n' 'the browser wrote no {active_port}' >&2
    exit {no_endpoint}
  fi
  sleep {interval}
done
cat "$active"
"#,
        root = REMOTE_PROFILE_ROOT,
        profile = quote(profile),
        active_port = DEVTOOLS_ACTIVE_PORT,
        no_folder = NO_PROFILE_FOLDER_EXIT,
        no_browser = NO_BROWSER_EXIT,
        no_endpoint = NO_ENDPOINT_EXIT,
        names = names,
        installations = installations,
        window_switch = window_switch,
        attempts = BROWSER_READY_ATTEMPTS,
        interval = BROWSER_READY_INTERVAL_SECONDS,
    )
}

/// Print the `DevTools` endpoint that the browser of `profile` on the remote
/// machine wrote, without starting one.
///
/// A profile folder without an endpoint prints nothing, because that is not a
/// failure of the transport: it tells the caller to start the browser.
#[must_use]
pub fn remote_port_command(profile: &str) -> String {
    format!(
        "if [ -s {root}{profile}/{active} ]; then cat {root}{profile}/{active}; fi",
        root = REMOTE_PROFILE_ROOT,
        profile = quote(profile),
        active = DEVTOOLS_ACTIVE_PORT,
    )
}
