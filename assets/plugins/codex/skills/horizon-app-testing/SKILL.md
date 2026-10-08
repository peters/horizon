---
name: horizon-app-testing
description: Run or inspect declared iOS and Android native app tests through Horizon device_test_run and app_* MCP tools. Use for App Automate sessions, native actions, evidence, and cleanup; desktop fixtures use horizon-device.
---

# Horizon native app testing

Use the separately configured native MCP server for declared iOS and Android tests.
Read the app and companion backend instructions before a device run.
Read [the native app reference](references/native-apps.md) for prerequisites,
interactive sessions, evidence, and recovery. Use current MCP schemas for arguments.

Use only the approved native device quota and declared loopback ports.
A browser quota does not authorize native devices. Keep provider credentials in Horizon.
Use one immutable artifact per platform per run and isolated synthetic backends.
Observe interactive tests through a live native Device panel in the current workspace.
Use `horizon-device` for viewer presentation checks; use `app_*` for native input.
Do not send native input through a browser or through a viewer's Interact toggle.
