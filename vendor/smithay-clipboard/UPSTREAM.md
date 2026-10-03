# Toolkit clipboard extension

Source: smithay-clipboard 0.7.3, MIT licensed, upstream commit
`26c2f53f15f6bdc4f41a442d0ae2c2d63bbc617c`.
The package source and its LICENSE are retained here; unused example and
development dependency declarations are omitted from the normalized manifest.

The extension subscribes to file drag/drop and image clipboard offers on the
toolkit's existing Wayland data device. Clipboard handles across viewports share
one worker per display, with serialized text reads. A second device on that connection can
compete with the text clipboard device for compositor offers. Text and primary
selection APIs keep their upstream behavior. Native transfers use bounded,
nonblocking reads with a deadline; their completion wakes Horizon rather than
making the UI poll continuously. Failed, empty, expired and rejected reads
complete as cancellation; worker loss and queue overflow reset pending native
input, so bounded admission cannot strand paste requests or synthetic hover.

`native.rs` and its tests are new. The additions in `lib.rs`, `state.rs`, and
`worker.rs` connect the subscription and transfers to the existing worker.
The copied upstream assertion policy is retained in those files; new native
input code denies unwrap and expect and forbids unsafe code. The crate denies
unsafe code except at the copied foreign-display and calloop/flags boundaries,
which retain explicit safety contracts. No new unsafe operations are introduced.
The fork is a workspace member so the normal tests and lint checks cover it.
Its code and dependencies are Linux-only, matching Horizon's native Wayland
adapter; the workspace's macOS and Windows builds use their existing clipboard
backends. The minimum Rust version matches the workspace (1.95).

Session boundaries advance a worker generation, cancel pending reads, and discard queued events. Paste requests and drag offers retain their originating generation, so delayed completions cannot reach a replacement board that reuses panel IDs.
