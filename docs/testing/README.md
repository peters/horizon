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
| [chromecast-live-progressive](procedures/chromecast-live-progressive.md) | Chromecast live cast, progressive transport | none |
| [cloud-idle-stop](procedures/cloud-idle-stop.md) | Cloud idle stop on RunPod and Hetzner, and the stopped card | rents compute |
| [cloud-settings-replace-key](procedures/cloud-settings-replace-key.md) | Cloud settings saved keys | none |
| [new-cloud-catalog-refresh](procedures/new-cloud-catalog-refresh.md) | New cloud dialog, background price refresh and layout height | none |
| [new-cloud-picker](procedures/new-cloud-picker.md) | New cloud dialog, worker list, filters, picks and data centers | none |
