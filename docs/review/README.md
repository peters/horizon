# Review rules

This folder holds the rules that a code review of this repository uses. It is
the only home of the review rules. `AGENTS.md` holds the full project rules.

The workflow `.github/workflows/claude-review.yml` reads the review rules from
the base branch of a pull request. It never reads them from the pull request.

## What a review reads

- [`review-scope.md`](review-scope.md) sets what a review reports and what it
  does not report.
- Each file in [`trust-boundaries/`](trust-boundaries/) whose `applyTo` globs
  match a changed file states the trust boundary of that file's subsystem.
  Review the subsystem against its boundary. Do not widen the boundary to find
  new attack surfaces.
- The `AGENTS.md` sections that apply to the change.

## The routing key

The front matter of a boundary file is its routing key:

```yaml
---
applyTo: "crates/horizon-core/src/dir_search.rs,crates/horizon-core/src/dir_search/**"
---
```

- Each entry is one plain glob, relative to the repository root. Divide the
  entries with a comma.
- Do not use a brace list such as `{cards*,close*}`. A plain glob reader does
  not match a brace list, so the boundary never routes.
- Name a folder tree with `/**`. A glob such as `cards*` matches `cards.rs` but
  not a file in the folder `cards/`.
- Each entry must match at least one file. Remove an entry that matches no
  file.
- A boundary may apply to a path outside `crates/`, for example a script, a
  Dockerfile or a workflow.

## How to add or change a boundary

1. Write the boundary before the first review of the work that reads the
   untrusted input. State what is untrusted, what the code guarantees and what
   it does not guarantee. `AGENTS.md` requires this step.
2. Keep the boundary in step with its design page in `docs/architecture/` and
   with the module comment.
3. Write the boundary in ASD-STE100 Simplified Technical English. Use the
   names in [`../style/technical-names.md`](../style/technical-names.md).
4. Write each path in the text from the repository root, for example
   `docs/architecture/browser-downloads.md`.
5. Add the `applyTo` globs, and run the checks below.

## The boundary groups

The boundaries divide into two groups. The group shows the neighbours of a
subsystem, not the crate that holds it. [`trust-boundary-audit.md`](trust-boundary-audit.md)
gives the untrusted input, the guarantee and the limit of each boundary.

1. **Local host and private files** — a program on this computer reads a
   private file, a host file or a machine that another person controls.
   Boundaries: `remote-browser-tunnel`, `standalone-remote-host`,
   `checkout-search`, `device-key-input`.
2. **Browser page input** — a page or an agent asks Horizon to attach a file,
   drop a file, answer a passkey request, save a download or judge a semantic
   action. Boundaries: `browser-webauthn`, `browser-semantic-actions`,
   `browser-downloads`.

## Checks

Run these commands before a change to a boundary, and after the audit.

```bash
# Each boundary: the entries of its applyTo front matter.
rg -n 'applyTo:' docs/review/trust-boundaries/

# A brace list in an applyTo entry, or an entry that matches no file.
python3 - <<'EOF'
import glob, re
from pathlib import Path
for page in sorted(Path("docs/review/trust-boundaries").glob("*.md")):
    front = re.match(r"^---\n(.*?)\n---\n", page.read_text(), re.S).group(1)
    entries = re.search(r'applyTo:\s*"?(.*?)"?\s*$', front, re.M).group(1).split(",")
    for entry in (e.strip() for e in entries):
        if "{" in entry or "}" in entry:
            print(page, "brace list:", entry)
        elif not [p for p in glob.glob(entry, recursive=True) if Path(p).is_file()]:
            print(page, "no file:", entry)
EOF
```

A boundary file with no output from these commands routes. The audit page
records which paths and symbols of a boundary were examined against the code.

## Pointers at the old paths

The review rules were in `.github/copilot-instructions.md` until the audit of
issue #1787. One short pointer file stays at the old path, so an old link still
works: `.github/copilot-instructions.md` points to this folder.

The pointers hold no review rule and no `applyTo` glob.
