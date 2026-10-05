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

Clipboard image data takes precedence over URI offers; URI selections remain native file candidates. A completed URI-list drop that is empty or names anything other than local files, such as a browser's remote image URL, reads PNG/JPEG from the same drag offer instead when one is offered. A bounded companion read captures text from the same offer for decoding failures; complete encoded images do not wait for text. Clipboard availability is replaced from the current focused-seat snapshot. Queue overflow advances the native transfer generation as well as cancelling queued events.

The companion text representation is retained through image validation, including image payloads with a valid signature but corrupt content. Completed native reads hand an incomplete companion pipe to the file worker pool. Valid native payloads finish independently; only decoding failures read that same-offer pipe up to its original deadline. Fallback text uses the toolkit's MIME selection, lossy UTF-8, and MIME-specific newline rules. Image validation and persistence run in Horizon's bounded file worker, outside the UI and clipboard event loops.
