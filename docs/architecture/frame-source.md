# Frame source programs

A native Device panel shows a VNC desktop. Since this design it can also show a
**frame source program**: one program of this computer that gives raw pictures
itself. Horizon holds no codec, no library and no model of that program. The
contract below is the whole of what Horizon knows, and
[`crates/horizon-frame-source/`](../../crates/horizon-frame-source/src/lib.rs)
is Horizon's side of it.

The repository names no program that speaks this contract. The tests use the
stand-in `scripts/device-smoke/frame_source_standin.py`.

## The contract

1. Horizon starts the program with the arguments of the settings. The program
   gets a Unix stream socket as file descriptor 3 for its pictures, and pipes as
   its standard input and standard output for control messages. Other text goes
   to its standard error.
2. A picture is a header of 32 bytes and then the planes, with no padding. The
   header is the eight bytes `NV12FRM1`, the sequence number (u64), the time of
   the capture in microseconds since 1970 (u64), the width (u32) and the height
   (u32), each whole number least significant byte first. Then the Y plane
   (`width * height` bytes) and the interleaved UV plane (`width * height / 2`
   bytes). The colours are BT.709 with limited range. Both sides are even and 16
   or more.
3. The size of a picture is the size of its own header. One or two pictures of
   the old size can come after a size change. A sequence number that is missing
   is a picture that the program dropped.
4. A control message is 4 bytes of length, most significant byte first, and then
   one JSON object of at most 65536 bytes with a member `type`. A type and a
   member that Horizon does not know are ignored.
5. The program exits with 0 at the end of its standard input, at `bye`, at
   SIGTERM and when its socket closes. It exits with 1 with one line on its
   standard error when its source fails.

## The settings

The section `frame_sources` of the settings holds a list of `{name, command}`.
The name is 1 to 40 characters of letters, digits, `-` and `_`. The command is
one line, split into the arguments of one program by
[`CommandLine`](../../crates/horizon-core/src/speech/command.rs) and never given
to a shell, with `{target}` at most once as a whole argument. The section follows
the server, as the recorder line of speech input does, with the same fallback: a
program that is not on this computer is a sentence and no viewer.

## The viewer

`device_panel create` takes either `endpoint` for a VNC desktop or
`frame_source: {name, target?}` for a program. An agent passes a name of the
settings and a target, and never a command. The refusals are
`unknown_frame_source`, `invalid_request` and `frame_source_program_missing`.

- **The reader** is a thread of each viewer. It reads the header under its
  bounds, then the planes, converts NV12 to the pixels of the widget and keeps the
  newest picture in a slot of one. A picture is converted only when the frame took
  the one before it.
- **The size** that the program is asked for is the size that the panel draws at,
  in physical pixels, after that size was stable for 150 ms, and at most the size
  that the program captures.
- **The visibility**: a viewer that is hidden, off canvas or behind a fullscreen
  panel is told `visible: false`, and the program sends no picture. It still
  sends its counters, so an inspection of the panel stays live.
- **The end**: `close` sends `bye`, waits up to two seconds, then sends `SIGTERM`
  to the exact process id, and `SIGKILL` two seconds after that. The program
  cannot outlive Horizon: the socket and the pipe of its standard input are
  closed by the kernel when the process that holds them ends.
- **Input** reaches the program only after the person turned Interact on in the
  panel, and only when the program said `input: true` in its `hello`. A point is
  a pixel of the picture that the panel shows.

## Why a program and not a capture library

A capture of a screen, a window or a camera is a program of the platform with
its own permissions, its own devices and its own rules. Horizon keeps none of
that in its own process: a person names the program, and Horizon reads its
pictures under bounds. A program of a system is the person's to install or to
write.

## The trust boundary

Every byte of the program is untrusted. The bounds, the guarantees and their
limits are in
[`docs/review/trust-boundaries/frame-source.md`](../review/trust-boundaries/frame-source.md).
