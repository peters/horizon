# Horizon test documents

This directory holds the manual and agent test documents for Horizon.

## Layout

| Directory | Contents | Language |
|---|---|---|
| `procedures/` | Permanent test procedures. One procedure for each feature. | STE, mandatory |
| `reports/` | Records of runs that must be kept. | STE descriptive rules |
| `docs/testing/*.md` | Older plans and evidence records. | Not yet STE |

## Rules

1. Use [the procedure template](procedures/TEMPLATE.md) for a new procedure.
2. Use [the report template](reports/TEMPLATE.md) for a new report.
3. Obey [the STE rules](../style/ste-rules.md).
4. Use [the technical names](../style/technical-names.md).
5. Convert an older plan to a procedure when you change its feature.

[Issue #1265](https://github.com/peters/horizon/issues/1265) tracks the
documents that are not yet STE.

## Procedures

| Procedure | Feature | Cost |
|---|---|---|
| [bound-worker-pull](procedures/bound-worker-pull.md) | Exact read-only registry material for CPU hosts | none |
| [browser-engine-panels](procedures/browser-engine-panels.md) | One browser panel per engine for each agent | none |
| [browser-firefox-disclosure](procedures/browser-firefox-disclosure.md) | Firefox native webdriver getter and sign-in field | none |
| [browser-passkey-bridge](procedures/browser-passkey-bridge.md) | Browser WebAuthn bridge to an external authenticator command | none |
| [browser-recording](procedures/browser-recording.md) | Browser panel video and toolbar icons | none |
| [browser-site-checks](procedures/browser-site-checks.md) | Browser key presses for keyboard focus checks | none |
| [chromecast-live-mirror](procedures/chromecast-live-mirror.md) | Chromecast live cast, mirror transport | none |
| [chromecast-live-progressive](procedures/chromecast-live-progressive.md) | Chromecast live cast, progressive transport | none |
| [cloud-agent-browser](procedures/cloud-agent-browser.md) | Browser tools of an agent panel in a cloud with agent isolation | rents compute |
| [cloud-agent-panel-start](procedures/cloud-agent-panel-start.md) | Agent panel start in a cloud, host instance and browser runtime root owner | rents compute |
| [cloud-agent-stop](procedures/cloud-agent-stop.md) | The agent stop of a dedicated worker without an idle period, its refusal while another agent works, and the request that Horizon takes and carries out | rents compute |
| [cloud-idle-period](procedures/cloud-idle-period.md) | An idle period that still parses but has no effect: the New cloud and Rebuild note, no idle period on a new worker, and the reconnect of a worker created with one | rents compute |
| [cloud-launch-format](procedures/cloud-launch-format.md) | Saved cloud launch format and publisher confirmation fences | none |
| [cloud-panels](procedures/cloud-panels.md) | Cloud panels end to end: 121 tests in 12 area files | rents compute |
| [cloud-quick-start](procedures/cloud-quick-start.md) | Quick start on the base image for a repository without `.horizon/cloud.yml` | rents compute |
| [cloud-settings-replace-key](procedures/cloud-settings-replace-key.md) | Cloud settings saved keys | none |
| [cloud-setup-gate](procedures/cloud-setup-gate.md) | Setup gate: the proof and the desktop preview of the setup agent on real repositories | none |
| [cloud-stopped-panel-restore](procedures/cloud-stopped-panel-restore.md) | Restored panels of a stopped or reconnecting cloud | rents compute |
| [cloud-uncommitted-build-context](procedures/cloud-uncommitted-build-context.md) | Uncommitted files of an image build context: what goes in, refusals and links | none |
| [companion-clouds](procedures/companion-clouds.md) | Companion clouds, agent access to the SSH alias, the key and the catalog | rents compute |
| [connect-github](procedures/connect-github.md) | Connect GitHub: the GitHub card, per-cloud sign-in in both modes, reconnect, skip and access requests | rents compute |
| [device-type-multi-chunk](procedures/device-type-multi-chunk.md) | `horizon-device` text input in several `type` actions | none |
| [docker-host-provider](procedures/docker-host-provider.md) | Clouds on an existing Docker host: create, stop, resume, delete, owned resources, the Cloud settings card and Check, the New cloud host card, the cloud card and `cloud_offers` | none |
| [github-app-tokens](procedures/github-app-tokens.md) | GitHub App user tokens: device sign-in, renewal, refusal and expiry | none |
| [image-publication-credentials](procedures/image-publication-credentials.md) | Operation-private publishing login, retained-account recovery and explicit selection | none |
| [local-image-preferences](procedures/local-image-preferences.md) | Machine-local image destinations, source identity and transactional save | none |
| [local-network-bridge-agent-access](procedures/local-network-bridge-agent-access.md) | Local Network Bridge, agent access on the worker | rents compute |
| [new-cloud-catalog-refresh](procedures/new-cloud-catalog-refresh.md) | New cloud dialog, background price refresh and layout height | none |
| [new-cloud-source-search](procedures/new-cloud-source-search.md) | New cloud source field: search over checkouts and GitHub repositories, folder paths and links | none |
| [native-app-automate](procedures/native-app-automate.md) | Native app matrix, MCP, CLI and exact cleanup | paid device |
| [new-cloud-picker](procedures/new-cloud-picker.md) | New cloud dialog, worker list, filters, picks and data centers | none |
| [tailnet-enrollment-ledger](procedures/tailnet-enrollment-ledger.md) | Saved enrollment receipts, allocation identity and capture attempts | none |
| [tailnet-receipt-capture](procedures/tailnet-receipt-capture.md) | Device ID capture before Ready and after stop/resume | rents compute (LIVE only) |
| [tailnet-stable-device-name](procedures/tailnet-stable-device-name.md) | Cloud tailnet device name after stop and resume | rents compute |
| [tailnet-tagged-enrollment](procedures/tailnet-tagged-enrollment.md) | Marker 4 and one-time enrollment/resume tag checks; optional real-tailnet lane needs separate authorization | rents compute (optional R01 only) |
| [tailnet-marker4-rollout](procedures/tailnet-marker4-rollout.md) | Immutable CPU pin, strict private storage, provider scope and unsupported RunPod modes | rents compute |
| [tailnet-api-credential-store](procedures/tailnet-api-credential-store.md) | Desktop API storage foundation, staged entries and independent auth keys | none |
| [tailnet-node-api](procedures/tailnet-node-api.md) | Unwired desktop exact-node HTTP transport and binding tests | offline none; authorized task-node deletion only |
| [tailnet-api-removal](procedures/tailnet-api-removal.md) | Native owner API credential removal with CLI/MCP requests | none |
| [tailnet-api-entry](procedures/tailnet-api-entry.md) | Masked owner API entry/replacement with metadata-only CLI/MCP requests | none |
| [tailnet-api-status](procedures/tailnet-api-status.md) | Nonsecret API credential status in Settings, CLI and cached MCP discovery | none |
| [vnc-recording](procedures/vnc-recording.md) | Device panel video | none |
| [worker-github-chain](procedures/worker-github-chain.md) | Worker GitHub access with a token chain that refreshes on the worker | none |

## Reports

| Report | Procedure | Date |
|---|---|---|
| [2026-10-05-cloud-panels](reports/2026-10-05-cloud-panels.md) | [cloud-panels](procedures/cloud-panels.md) | 2026-10-05 |
| [2026-10-14-tailnet-receipt-capture](reports/2026-10-14-tailnet-receipt-capture.md) | [ledger](procedures/tailnet-enrollment-ledger.md), [capture](procedures/tailnet-receipt-capture.md) | 2026-10-14 |
| [2026-10-14-tailnet-api-removal](reports/2026-10-14-tailnet-api-removal.md) | [tailnet-api-removal](procedures/tailnet-api-removal.md) | 2026-10-14 |
| [2026-10-15-tailnet-node-api-first-live](reports/2026-10-15-tailnet-node-api-first-live.md) | [tailnet-node-api](procedures/tailnet-node-api.md) | 2026-10-15 |
| [2026-10-15-tailnet-node-api-final-live](reports/2026-10-15-tailnet-node-api-final-live.md) | [tailnet-node-api](procedures/tailnet-node-api.md) | 2026-10-15 |
