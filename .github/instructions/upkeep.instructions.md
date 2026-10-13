---
applyTo: "crates/horizon-core/src/upkeep.rs,crates/horizon-core/src/upkeep/**,crates/horizon-ui/src/upkeep_widget.rs,crates/horizon-ui/src/upkeep_widget/**,crates/horizon-ui/src/app/upkeep.rs"
---

# Trust boundary of the Upkeep board

The Upkeep board shows a status document that an agent on a cloud worker writes.
The user-facing description is in `docs/upkeep.md`, section "Trust boundary of the
status document".

## Untrusted: the status document

Every field can hold any value: very long texts, very many entries, unknown states,
times far in the past or the future, and URLs to other sites. Check that:

- No file read, parse or other blocking work on the status runs on the UI thread.
- The size limit and the list and text bounds apply before the board draws anything.
- Arithmetic over counts from the document cannot overflow or panic.
- An unknown enum value, a field of the wrong type or one broken list entry never makes
  the whole document invalid.
- Nothing assumes the order of lists in the document.
- Only `PullRequest::github_url` decides which page opens, and it accepts only the
  exact GitHub pull request page of a plain `owner/name` repository.

## Trusted: the person and the panel configuration

The panel's `command` (the status path) comes from the person's config or session.
The board reads that path and nothing else.
