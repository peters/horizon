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
| [block-panel](procedures/block-panel.md) | Agent cards: create, update, inspect, hide, show, close and restart | none |
| [browser-engine-panels](procedures/browser-engine-panels.md) | One browser panel per engine for each agent | none |
| [browser-firefox-disclosure](procedures/browser-firefox-disclosure.md) | Firefox native webdriver getter and sign-in field | none |
| [browser-passkey-bridge](procedures/browser-passkey-bridge.md) | Browser WebAuthn bridge to an external authenticator command | none |
| [browser-recording](procedures/browser-recording.md) | Browser panel video and toolbar icons | none |
| [browser-site-checks](procedures/browser-site-checks.md) | Browser key presses and page zoom for keyboard and reflow checks | none |
| [remote-ios-click](procedures/remote-ios-click.md) | Trusted clicks and field entry on physical iOS browsers | paid device |
| [device-type-multi-chunk](procedures/device-type-multi-chunk.md) | `horizon-device` text input in several `type` actions | none |
| [vnc-recording](procedures/vnc-recording.md) | Device panel video | none |
| [device-panel-ssh-trust](procedures/device-panel-ssh-trust.md) | Device panel ssh route: untrusted host refusal, fingerprint comparison, trust and changed key | none |
| [machines-skill](procedures/machines-skill.md) | `horizon-machines` skill: rent, card, Connect, confirm delete and proof for each provider | rents compute |
| [sidebar-machines](procedures/sidebar-machines.md) | The Machines section of the sidebar: records, click to connect, restart, forget and a refused record | none |
| [workspace-place](procedures/workspace-place.md) | A workspace that lives on a machine: its new terminals run there, re-attach after a restart and end with a close | none |

