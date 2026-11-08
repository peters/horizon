# Browser panel downloads

A browser panel saves every download of its page into one folder that Horizon
owns for that panel, and reports each download it observed. The behavior was
introduced for [#1994](https://github.com/peters/horizon/issues/1994), where a
page's second download was dropped without a prompt or an error, and where a
person's download from the built-in PDF viewer left no file.

The public contract is the `browser_downloads` tool in
`crates/horizon-browser-mcp/README.md`. The trust boundary is
[`docs/review/trust-boundaries/browser-downloads.md`](../review/trust-boundaries/browser-downloads.md).
The repeatable check is
[`docs/testing/procedures/browser-downloads.md`](../testing/procedures/browser-downloads.md).

## One folder for each panel

The folder is `<panel profile root>/<panel id>/downloads`. It is a private
directory (`0700`) beside the backend profiles of the same panel, so a panel
that switches engine keeps one folder, and a Snap-confined browser can write
into it. `browser.profile_root` moves it together with the profile.

A panel that shares a browser profile from `browser_duplicate` shares the
download folder of that profile. The browser-wide setting that selects a
download folder cannot be per panel inside one browser process, so the shared
profile is the smallest unit that keeps the behavior honest.

## Chromium

At session start, before any caller document exists, the driver sends
`Browser.setDownloadBehavior` with `behavior: allow`, the panel folder and
`eventsEnabled: true`. Two effects matter:

- The page's downloads are saved into the panel folder, whatever the home
  directory or desktop of the process is.
- Chromium's limit of one page-initiated download for each document no longer
  applies. With the default behavior Chromium drops every later download and
  emits no event at all, which is the silent failure in #1994.

`Browser.downloadWillBegin` names the file the page suggested.
`Browser.downloadProgress` carries the bytes, the state and, once the browser
finished the file, its final path. The driver keeps the newest 64 reports and
counts the rest as `dropped`.

If Chromium refuses the call, the panel still serves the page. It then states
`downloads: false` in its negotiated capabilities and `reporting: false` in a
download report, so an empty list cannot be read as "nothing was downloaded".

## Firefox

Firefox saves a download only where its own preferences say, and it asks a
person when a content type is new to it. A headless or unattended panel cannot
answer that question, so the session sets these preferences at creation:

| Preference | Value |
| --- | --- |
| `browser.download.folderList` | `2` (the folder below) |
| `browser.download.dir` | the panel folder |
| `browser.download.useDownloadDir` | `true` |
| `browser.download.start_downloads_in_tmp_dir` | `false` |
| `browser.download.always_ask_before_handling_new_types` | `false` |
| `browser.helperApps.neverAsk.saveToDisk` | document, archive, image and text types |

The driver then subscribes to `browsingContext.downloadWillBegin` and
`browsingContext.downloadEnd`. Firefox 144 and 145 introduced those events. An
older Firefox still saves into the panel folder; the session reports
`reporting: false` because it cannot name what it saved.

The subscription carries no context filter. A page can start a download in a
child frame, and the event then names that frame's context; a filter on the
panel's top-level context would drop a file the panel did save. The rows
therefore describe the downloads of this browser, which is what Chromium's
browser-level events already report.

Firefox reports the final path when the download ends, and no byte counts. The
engine measures the file on disk instead, so a completed Firefox row reports the
same size in `received_bytes` and `total_bytes` as a Chromium row. It reads only a
path inside this panel's folder; a path outside it is named in the row and never
opened.

The same rule covers a transfer that is still running. Firefox reports no
progress at all, so an in-progress Firefox row would read zero bytes until the
file finished. Both engines write the bytes they have into a file beside the one
they will save: Chromium adds `.crdownload` to the whole name, and Firefox writes
`<stem>.<random>.<extension>.part` and leaves an empty file under the name it will
keep. A read measures the incomplete file that carries the stem and the extension
of the row when the row has no count of its own, and never reports more than the
total the browser stated. Two transfers can share a name, and a browser states a
different name for the second one (`export(1).csv`), or leaves a stale file of an
earlier run beside it. A name that matches more than one row without a count of
its own, or one file that more than one running row could claim, describes no
single transfer, so the row states no size instead of another transfer's bytes.
A row that counts its own bytes still holds its own file, so it is counted as a
claimant too. The folder outlives a
session, so a part file that an earlier session left is skipped as well: a row
measures only a file that was written while the row was already observed, and a
new download of the same name writes a file of its own. A browser that reports its own progress keeps
its number, and a finished or canceled row is never measured.

## Safari and remote sessions

Safari's classic `WebDriver` exposes no download events and no download folder,
and a remote device writes files on the provider's machine. Both answer
`unsupported_backend`, and neither advertises the `downloads` capability.

## A repeated file name

A page can download two files with the same name. The two engines answer
differently, and the report shows what each did: Chromium replaces the earlier
file at the same path, and Firefox saves the second file beside the first with a
suffix (`report-1.pdf`). A row always names the path the browser wrote, so the
report never claims a file that is no longer there.

## What a report holds

A row holds the file name, one state (`in_progress`, `completed`, `canceled`),
the bytes received, the total size when the response stated it, and the
absolute path once the browser named the file. It never holds the download URL:
that URL can carry a credential, as the archive in #1994 did.

The file name is the browser's own choice, so the engine keeps only its last
path component, removes control characters, and cuts it to 128 bytes.

## Reading a saved file

Use ordinary read-only tools on the reported path. A canceled download leaves no
file. A page can start more downloads and fill the folder; Horizon does not limit
the folder's size or examine what a page saved, so a person deletes the folder to
reclaim space.
