# horizon-terminal

The click targets and the search of Horizon's terminal panels, as a crate that
any UI or server can use with its own `alacritty_terminal` grid. It has no PTY
and no UI toolkit, and it depends on no other Horizon crate.

- `target_at(&term, point)` gives what a click on one cell opens: the OSC 8
  hyperlink first, then a URL on the logical line of the cell (rows that a
  program or tmux hard-wrapped through a URL join), then a file path on the
  soft-wrapped line. `point` is a grid point, so it can name a line of the
  history as well as of the screen.
- `hyperlink_at(&term, point)` gives only the OSC 8 hyperlink, without a copy
  of its URI.
- `search_lines(&lines, query, &options)` gives at most `MAX_MATCHES`
  literal or regex matches, with case on or off.

## Add it to an application

Pin a Horizon commit, and use the `alacritty_terminal` version of Horizon's
root `Cargo.toml`, so that Cargo builds one copy:

```toml
[dependencies]
horizon-terminal = { git = "https://github.com/peters/horizon.git", rev = "<commit>" }
alacritty_terminal = "0.26.0"
```

The tests stay in this crate: `cargo test -p horizon-terminal`.
