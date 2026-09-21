# Cloud panels: labelled design fixtures

These fixtures preserve the approved three-provider composition. They run locally
and do not provision workers. For the operational RunPod feature, configuration,
credentials and per-agent remote worktrees, see [Cloud Workspaces](../cloud-workspaces.md).

Build `cargo build -p horizon-ui --features cloud-panel-mock`.
Launch the candidate with an **empty private config**, `--ephemeral`, and an
absolute `HORIZON_CLOUD_MOCK_DIR` pointing to a private, disposable directory.
For interactive testing use the isolated desktop/native VNC workflow in
`scripts/device-smoke/README.md`; never launch a test over an existing desktop.

Example private configuration (JSON is valid YAML):

```json
{"version":11,"appearance":{"theme":"dark"},"workspaces":[]}
```

The prototype starts with three scenarios: a Daytona website browser, a
Claude + Grok grid, and a Claude + Codex grid with VNC and a second browser.
Set `HORIZON_CLOUD_MOCK_VNC` to an explicit numeric loopback VNC endpoint for
the third scenario. Without one, the ordinary Device panel requests connection
setup. Agent executables and real sign-in must be available in the environment.

Double-click a cloud header to edit its title; Enter saves and Escape cancels.
**New cloud** adds an empty cloud from five prepared prototype worktrees. There
is no issue integration. Use Horizon’s existing Ctrl-double-click picker inside
a cloud to add panels. Each cloud supports multiple instances of every agent.
Agent-created browser and VNC panels inherit their agent’s cloud.

The adjacent runtime card selects a repository profile and independently sets
Default, Rows, Cols or Grid using the workspace layout controls and calculations.
**Full screen** fills the display with one cloud and its runtime card; **All clouds**
or Escape restores the overview without hiding or stopping other sessions. RunPod
retains its official logo. The starter clouds select Daytona for the web preview,
RunPod GPU for pair programming, and Fly.io CPU for the full stack sandbox. **Deploy cloud** plays a mock
worker → Docker image → workspace flow; each step exposes verbose mock output.
The final step launches the real local agent panels. **Replay deployment**
animates the flow again without creating duplicate agents or restarting them.
Provider changes affect mock launch intent only; they never move local sessions.

Each frame owns panel membership and geometry; actual panel implementations,
input and process lifecycles remain Horizon's. Additional agents within one
cloud intentionally share its checkout. Different clouds use different
worktrees. Collapse changes visibility only. A bound panel cannot migrate to another cloud or workspace; dragging stays
inside its original frame. Only empty frames can be removed. Closing the application ends local processes; restart uses Horizon's
saved agent session bindings where available. This is not remote continuity.

State is stored only in `cloud-panels.json` under the selected directory. Relaunch
with the same directory to restore it; worktree changes are never reset. Launch
without the environment variable to retain ordinary Horizon behavior, even when
the feature is compiled in. Production cloud and session references use the normal persistence path independently of this private fixture snapshot.

The fixture descriptor accepts the labelled Daytona, RunPod and Fly.io scenarios
from [`design-fixtures.yml`](../../crates/horizon-cloud/examples/design-fixtures.yml).
Daytona and Fly.io remain design fixtures; they are not operational providers.
The production configuration accepts RunPod only and uses
[`cloud.yml`](../../crates/horizon-cloud/examples/cloud.yml). Do not copy the
fixture configuration into a repository intended for real deployment.

The production `horizon-cloud` crate owns validated portable configuration and
RunPod REST lifecycle operations. Horizon core separately coordinates local image
builds, source transfer, SSH and independent persistent agent worktrees. None of
that remote execution is implied by a fixture's simulated deployment animation.
Fixture browser labels report actual local control ownership. Fixture VNC labels
identify viewing separately from input; production ownership comes from the
worker's device-control journal.

RunPod logo source: [official RunPod site](https://www.runpod.io/),
[original SVG](https://cdn.prod.website-files.com/69ce570adca53340abab8376/69cfb722ebc6a6cfe4f48961_runpod-logo-white.svg).
The bundled PNG is a rasterization of that SVG for the existing image loader.
