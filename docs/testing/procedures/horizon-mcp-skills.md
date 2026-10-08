---
procedure: horizon-mcp-skills
feature: Horizon MCP skill coverage and installation
platforms: [linux, macos, windows]
cost: none
destructive: no
secrets: none
owner: peters
---

# Horizon MCP skills test procedure

## 1. Purpose

This procedure checks skill coverage, bundled references, installation, and host leases.
It uses source checks and private temporary directories. It does not start resources.

## 2. Applicability

Use the final candidate checkout. Python 3 and the Rust build prerequisites must exist.
These checks do not prove cloud, TV, browser, or paid native device runtime behavior.
No UI layout or input behavior changes in this feature.

## 3. Tasks

### 3.1 COVERAGE — Examine MCP guidance

1. Run `python3 -B scripts/check-horizon-mcp-skills.py`.

   Result: Each shipped tool has a skill route. Operation lists name the exact API.
   Both bundles have the same files. Each reference has a valid link and embedded asset.

2. Examine the six skill entry points under `assets/plugins/codex/skills/`.

   Result: Browser, device, speech, casting, cloud, and native app tasks have clear routes.
   Standalone servers have explicit setup requirements. Speech uses local diagnostics.

### 3.2 INSTALL — Examine isolated installation

1. Run `cargo test -p horizon-ui plugin_install::`.

   Result: Complete skills reach bundled and custom roots. A repeated installation
   makes no change. A bundle reference update changes only that file.

2. Examine the ownership and lease test results.

   Result: Custom user files, partial trees, changed references, and symlinks remain intact.
   A complete tree remains until the last host exits. Later user edits survive cleanup.
   Private caches remove retired references from prior unrecorded bundles.
   A private cache with a symlink refuses writes.

### 3.3 MATRIX — Run repository checks

1. Run the full pre-push matrix in `AGENTS.md` in this checkout.

   Result: Format, maintainability, workspace tests, speech tests, and required
   Clippy tiers pass. Record advisory findings separately.

### 3.4 LOCAL — Refresh authorized installed copies

1. Compare each supported local skill tree with its known previous bundled version.

   Result: User changes and symlinks are identified before any write.

2. Refresh only the matching Horizon-owned skill files from the validated candidate.

   Result: References accompany entry points. User files, MCP configuration,
   host leases, and active processes remain intact.

3. Compare refreshed bytes with candidate assets.

   Result: Each refreshed tree matches the candidate. Record skipped roots and reasons.
   File parity does not prove that an active agent reloaded its instructions.

## 4. Pass criteria

All source and installation checks pass in the final checkout.
Each shipped MCP capability has correct guidance and setup limits.
Local copies match or have an explicit preservation reason.
No paid resource or active process changes during this procedure.

## 5. Cleanup and results

Tests remove their temporary fixture directories. Keep validation logs outside the repository.
Put public test results in the pull request. Keep machine paths and private data out of it.
