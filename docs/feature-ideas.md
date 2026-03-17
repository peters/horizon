# Feature Ideas

## Tier 1 — High Impact

1. **Panel Linking / Piping** — visual connections between panels, output of one feeds into another
2. **Spatial Bookmarks / Waypoints** — named canvas positions you jump to with keybindings
3. **Smart Panel Grouping** — auto-cluster panels by cwd/repo/language
4. **Panel Output Triggers / Watchpoints** — regex watchers on terminal output that fire actions (focus, notify, run command in another panel). Builds on the existing Attention system.
5. **Command Palette** — expand Cmd+K into a full VS Code-style palette: run commands, create panels, search output, toggle settings

## Tier 2 — Differentiating

6. **Panel Output Search** — grep across all panel scrollback buffers simultaneously, highlight matching panels on canvas
7. **Workspace Snapshots** — named save/restore of entire workspace arrangements (like git stash for your layout)
8. **Panel Templates / Snippets** — user-defined presets like "Docker logs" = shell + `docker compose logs -f`
9. **Timeline / Session Replay** — scrubber to replay what happened in a panel (you already record transcripts)
10. ~~Floating Sticky Notes~~ (tried)

## Tier 3 — Power User

11. **Panel Thumbnails in Sidebar** — tiny live previews of terminal output
12. **Cross-Panel Text Selection** — select text spanning multiple panels
13. **Focus Modes** — named layouts per task ("dev mode", "monitoring mode") with animated transitions
14. **Agent Orchestration View** — meta-panel showing all running Claude/Codex agents, status, tokens
15. **Interactive Minimap** — click/drag the minimap to navigate, activity pulse indicators
16. **Split Panel View** — tmux-style splits within a single panel
17. **Environment Indicators** — prod/staging/dev badges on panels, color-coded borders
18. **Broadcast Input** — type into multiple panels simultaneously
