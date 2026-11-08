---
applyTo: "crates/horizon-browser-protocol/src/downloads.rs,crates/horizon-browser/src/downloads.rs,crates/horizon-browser/src/session/startup.rs,crates/horizon-browser/src/session/events.rs,crates/horizon-browser/src/session/semantic.rs,crates/horizon-browser/src/webdriver/session/handshake.rs,crates/horizon-browser/src/webdriver/session/bidi.rs,crates/horizon-browser/src/webdriver/session/downloads.rs,crates/horizon-browser/src/webdriver/session/semantic.rs,crates/horizon-browser-mcp/src/model/downloads.rs"
---

# Trust boundary of browser panel downloads

A browser panel saves every download of its page into one folder that Horizon
owns for that panel, and reports what it saved. The design is in
[`docs/architecture/browser-downloads.md`](../../architecture/browser-downloads.md).

## Trusted: the agent request

The agent asks one read-only question through `browser_downloads` on a panel it
is permitted to control.

- The lookup starts no download and writes no file.
- The answer holds the panel folder and the rows this exact session observed. It
  never holds a download URL, because that URL can carry a credential.
- A row holds the file name, the state, the byte counts and the path. It holds
  no page text and no response headers.
- One optional `fetch` argument names the file name of one completed row. An
  answer holds the bytes of that file when it is inside the panel folder and at
  most 8 MiB, as one `application/octet-stream` resource block, so an agent that
  works through a host of another computer reaches the file it asked for. The
  answer names that file in `fetched_file_name` only when it carries its bytes; a
  larger file sets `inline_withheld: true` and stays on that computer.

The host reads the file that its own report named, never a path from the caller:
the name must be one path component, and the canonical path of the file must stay
inside the canonical panel folder, so a symlink or a reported path cannot reach
outside it. The bytes carry no media type of their own, because the engine does
not sniff an untrusted file.

## Untrusted: the page and its downloads

A page names its own downloads, starts any number of them, and decides what
bytes they contain.

- The browser, not the page, chooses the file it writes, and the folder is the
  panel folder. The engine never joins a page-supplied name into a path.
- A reported name keeps only its last path component, loses control characters
  and is cut to 128 bytes, so a page cannot present a name that looks like a
  host path.
- A row states what the browser reported. A download the browser blocked
  before it began produces no row at all, so an empty list is not proof that the
  page did not try.
- A download a child frame, or another page of the same browser, starts is a row
  too: the report describes the downloads of this browser, which is the unit
  that owns the folder.

## Does not guarantee

- A page can fill the panel folder, and Horizon does not limit the folder's size
  or inspect file contents. A person deletes the folder to reclaim space. The
  files a page saved are as untrusted as the page.
- Chromium saves the panel's downloads without the prompt that would let a person
  refuse the second one. This is a deliberate change: the prompt is not visible
  in an unattended panel, and its absence was the silent failure in #1994.
- A browser that refuses the download folder, or a Firefox older than the
  download events, saves without reporting. The session then states
  `reporting: false`; it never claims an empty folder means no download.
- A panel that shares a browser profile shares that profile's download folder.
  Rows describe the browser's downloads, not one panel's share of them.
- A download that a page cancels, or that the network ends early, leaves a
  partial file that the browser may keep. A `canceled` row names no path.
- A page that downloads two files of one name replaces the first one under
  Chromium, while Firefox saves the second beside it. The rows name the paths
  the browser wrote; Horizon does not keep a copy of a replaced file.
- The report is bounded to 64 rows. Older rows leave the report and their files
  stay in the folder.
