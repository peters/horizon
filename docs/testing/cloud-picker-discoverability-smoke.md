# Cloud picker discoverability smoke

Temporary validation plan for `fix/cloud-picker-discoverability`. Delete after the final native UI validation completes.

Use the final debug executable from this branch, frozen under a private task-owned
path. Record the commit, sha256 and actual child PID; verify `/proc/PID/exe`.
Use only an isolated Xvfb/Openbox desktop, private Horizon home and task-owned
loopback VNC server. Watch it through a native VNC Device panel in the calling
agent's workspace. Establish live presentation with three timestamped public
inspections, two seconds apart, including advancing frames during changing output.
Follow the bounded viewer health procedure in `scripts/device-smoke/README.md`.
Drive only the exact fixture's device target. Never allocate a worker during this
picker test. Record the whole feature flow from that display, and verify decoded
frames before creating a two-pass palette GIF under 10 MB for the PR.

## Fixtures and baseline

- Create and commit a synthetic repository with `version: 1`, default CPU profile,
  public example image, `min_cpu: 8`, `min_memory_gb: 32`, container 30 GB,
  standard workspace volume 80 GB and a GPU profile with `min_gpu_memory_gb: 24`.
- Use private machine bindings for read-only provider price checks. Never show,
  record, publish or print the settings or credentials. Do not press Start.
- Capture the launched app before opening New cloud. Verify normal terminals,
  canvas navigation and the Cloud > New cloud path still work.
- Open New cloud on the fixture; verify title, profiles, summary, cost, actions,
  chosen worker, price freshness and refresh button remain present.

## CPU catalog

- Requirements say at least 8 vCPU and 32 GB memory, with matching/excluded counts.
- Search and In stock only are visible without expanding any section.
- Search a matching CPU size; clear it and confirm the selected worker did not change.
- Search a nonexistent name; verify a helpful empty-result explanation.
- Reveal workers below requirements and search a small size. Their rows explain
  exclusion; clicking or keyboard activation cannot select them or enable Start.
- Select a qualifying row/card. Summary, price and resource selection agree.
- Toggle stock filtering. Counts update and the selected worker is preserved.

## Geography and storage

- All configured datacenters appear immediately, grouped by region.
- Choose an exact datacenter directly from Any. Choose a different region, then
  an exact datacenter in another region without returning to Any first.
- Sold-out places remain visible/selectable; Start/watch behavior is unchanged.
- Change to High-performance storage. Incompatible datacenters remain visible,
  disabled with Storage unavailable; counts exclude them from available stock.
- Existing incompatible selection is preserved and blocks Start with a reason.
- Choose a region containing datacenters with mixed storage support on Standard.
  Switch to High-performance; the retained region must block Start if any chosen
  center is incompatible. Choose a compatible exact center to clear the reason.
- Return to Standard; compatible choices become selectable again.
- Configured machine-level datacenter restrictions remain enforced and explained.

## GPU, refresh and migration

- Change to GPU. Search and stock filters reset for the new profile.
- The cheapest qualifying in-stock GPU is chosen; below-floor GPUs can be inspected
  but cannot be selected. Sold-out qualifying GPUs remain selectable.
- Arm/stop a watch only if possible without renting anything; it fixes the exact
  chosen hardware/location and existing price ceiling.
- Refresh while browsing; selected worker/location are preserved. Automated tests
  cover stale/failed/empty catalogs, higher watched prices and no-match admission.
- Legacy cpu/memory_gb YAML and explicit min_cpu/min_memory_gb YAML parse identically.
  Duplicate aliases are rejected. Saved worker JSON retains cpu/memory_gb keys.
- Cancel and reopen; requirements remain from the repository, browsing state resets
  appropriately, and no file/profile/settings were edited by browsing.

## Visual and cleanup

- Capture screenshots after launch and after window shrink/grow/fit.
- Verify narrow and short windows scroll correctly, footer actions remain reachable,
  both themes show readable disabled explanations and keyboard focus is visible.
- Decode representative video/GIF frames and inspect synthetic content only.
- Close the exact application normally, then stop fixture-owned recorder, display,
  VNC and window manager. Expire device target and close only the task-owned viewer.
- Retain private evidence and email the user the PR/demo with honest test status.

## Current handoff status

The initial feature smoke and attached GIF passed on commit
`cd4b8b8ad015b92025b91f5bb614671caed217d9`. Subsequent review fixes add the
all-centers storage guard and update the Hetzner profile example. Unit tests cover
the retained mixed-support region and a removed datacenter.

The final candidate native UI retest and replacement recording are pending:
the task-owned viewer connects and receives advancing decoded frames, but reports
`image_displayed: false`, `frame_sequence: 0`, and `presentation: not_rendered`.
One public reveal attempt did not establish presentation. The host exclusion
reason is absent; the existing diagnostics follow-up is issue #801.
Resume with a new task-owned isolated fixture when native presentation is
available, run this plan on the current PR head, attach its GIF, then remove
this temporary plan. Never use the developer desktop or a browser VNC viewer.
