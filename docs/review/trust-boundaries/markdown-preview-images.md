---
applyTo: "crates/horizon-ui/src/editor_widget.rs,crates/horizon-ui/src/editor_widget/**,docs/testing/procedures/markdown-preview.md"
---

# Trust boundary of the pictures of a markdown preview

A markdown note can hold pictures. The preview of the editor panel shows the
pictures that are files on this computer, and a line of text in the place of
every picture that it does not show. The module comment of
[`crates/horizon-ui/src/editor_widget/preview_images.rs`](../../../crates/horizon-ui/src/editor_widget/preview_images.rs)
gives the design, and
[`docs/testing/procedures/markdown-preview.md`](../../testing/procedures/markdown-preview.md)
gives the steps that a person does.

## Untrusted: the text of the note

The text of the note is untrusted input. A note can come from a repository that
the person did not write. The text decides the path of every picture, the
number of pictures and the content of every file that the preview reads.

- **The preview makes no network request for a picture.** The application
  installs one image loader, and it answers only the private scheme of the
  preview. No loader of this application reads `http:`, `https:` or `data:`
  bytes. A picture with a scheme other than `file:`, and a protocol-relative
  path, becomes one line of text and never a request.
- **The preview reads a regular file only.** The worker opens the file and
  reads the file type of the open handle. A pipe, a device such as
  `/dev/zero`, a folder and a socket are refused before the first read. A path
  check before the open cannot give this guarantee, because a race can replace
  the file.
- **The preview reads at most 16 MiB of one file.** The worker reads the length
  of the open handle and refuses a longer file before the first read. A shorter
  file is read with an explicit limit, so a file that grows while it is read
  stops at the limit.
- **The decoded picture has a bound on its pixels.** The worker reads the size
  from the header of the file, with a limit of 16 777 216 pixels, and refuses a
  larger picture before the decode. A small file cannot ask for a large
  allocation. The decoder also gets the same limit, so it refuses a picture
  that its header did not state.
- **The preview decodes the formats that the workspace already decodes.** They
  are PNG and JPEG, the formats that the clipboard of the editor and the image
  transfer already decode. The preview decodes no SVG and no picture format of
  its own.
- **Nothing is read, file-typed or decoded on the UI thread.** The UI thread
  changes a state and reads an answer that a worker wrote; the worker opens,
  types, reads, checks and decodes. The guard of
  [`crates/horizon-core/src/ui_thread.rs`](../../../crates/horizon-core/src/ui_thread.rs)
  holds in its strict mode through a frame that shows a picture.
- **A path may name any file that the person can read.** A picture of this
  computer, shown to the person at this computer, crosses no boundary. The
  preview does not confine a path to the folder of the note, because a note in
  a repository usually points at a picture beside it with `..`, and because
  the same text must show the same picture at 100 percent of the canvas zoom and
  after a move of the note.
- **The number of pictures of one note is bounded.** The preview loads at most
  32 pictures of one note. A 33rd picture becomes one line of text and no file
  is opened for it.
- **The memory of the pictures of a note is bounded and ends with the panel.**
  The decoded pictures of one note share a budget of 16 777 216 pixels, and the
  oldest picture leaves the budget first. The panel keeps the identifiers of its
  pictures and drops its cache and its textures when it closes.

## The line that replaces a picture

The preview writes the line itself, so the line carries no markdown of the
untrusted path. A path, an alternative text and a host name enter the line as
escaped text.

- A file that is not there: `Image not found: <path as written>`.
- A file that the preview refuses: the reason in plain words, so `Picture too
  large:`, `Not a picture file:`, `Not a regular file:`, `Cannot read:` or
  `Too many pictures in this note`.
- A picture with a scheme other than `file:`: `Remote image not loaded:` and the
  alternative text of the picture, or its host name.
- A picture that is on its way: one line, and the placeholder of the final size
  when the size is known.

No state panics and no state writes a log line at `ERROR`. A file that changes
on disk keeps its old picture until the note is opened again or its text is
saved.

## What the preview does not guarantee

- The correctness of a decoder against a hostile file. The `image` crate
  decodes in safe Rust, so a hostile file cannot reach memory that is not its
  own, and it can still take CPU time inside the bound of the file length and
  the pixel limit.
- That the picture is the one that the author of the note intended. The preview
  shows the file that the path names at this moment.
- That another application of this computer agrees with the preview about a
  path, a format or a size limit.
- Nothing about a picture that another application draws, for example a WebP
  picture that a browser panel shows.
