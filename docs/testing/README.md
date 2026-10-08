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
| [browser-recording](procedures/browser-recording.md) | Browser panel video and toolbar icons | none |
| [chromecast-live-mirror](procedures/chromecast-live-mirror.md) | Chromecast live cast, mirror transport | none |
| [chromecast-live-progressive](procedures/chromecast-live-progressive.md) | Chromecast live cast, progressive transport | none |
| [cloud-agent-browser](procedures/cloud-agent-browser.md) | Browser tools of an agent panel in a cloud with agent isolation | rents compute |
| [cloud-agent-panel-start](procedures/cloud-agent-panel-start.md) | Agent panel start in a cloud, host instance and browser runtime root owner | rents compute |
| [cloud-idle-stop](procedures/cloud-idle-stop.md) | Cloud idle stop on RunPod and Hetzner, and the stopped card | rents compute |
| [cloud-panels](procedures/cloud-panels.md) | Cloud panels end to end: 113 tests in 12 area files | rents compute |
| [cloud-quick-start](procedures/cloud-quick-start.md) | Quick start on the base image for a repository without `.horizon/cloud.yml` | rents compute |
| [cloud-settings-replace-key](procedures/cloud-settings-replace-key.md) | Cloud settings saved keys | none |
| [cloud-stopped-panel-restore](procedures/cloud-stopped-panel-restore.md) | Restored panels of a stopped or reconnecting cloud | rents compute |
| [companion-clouds](procedures/companion-clouds.md) | Companion clouds, agent access to the SSH alias, the key and the catalog | rents compute |
| [device-type-multi-chunk](procedures/device-type-multi-chunk.md) | `horizon-device` text input in several `type` actions | none |
| [local-network-bridge-agent-access](procedures/local-network-bridge-agent-access.md) | Local Network Bridge, agent access on the worker | rents compute |
| [new-cloud-catalog-refresh](procedures/new-cloud-catalog-refresh.md) | New cloud dialog, background price refresh and layout height | none |
| [native-app-automate](procedures/native-app-automate.md) | Native app matrix, MCP, CLI and exact cleanup | paid device |
| [new-cloud-picker](procedures/new-cloud-picker.md) | New cloud dialog, worker list, filters, picks and data centers | none |
| [tailnet-stable-device-name](procedures/tailnet-stable-device-name.md) | Cloud tailnet device name after stop and resume | rents compute |
| [vnc-recording](procedures/vnc-recording.md) | Device panel video | none |
| [worker-github-chain](procedures/worker-github-chain.md) | Worker GitHub access with a token chain that refreshes on the worker | none |

## Reports

| Report | Procedure | Date |
|---|---|---|
| [2026-10-05-cloud-panels](reports/2026-10-05-cloud-panels.md) | [cloud-panels](procedures/cloud-panels.md) | 2026-10-05 |
