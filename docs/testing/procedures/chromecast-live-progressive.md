---
procedure: chromecast-live-progressive
feature: horizon-chromecast live cast, progressive transport
platforms: [linux, macos, windows]
cost: none
destructive: no
secrets: none
owner: peters
---

# Chromecast progressive live cast test procedure

## 1. Purpose

This procedure proves that a live cast with the progressive transport starts on
a real receiver. It also proves that the lag decreases and that the live cast
ends when the receiver or another sender stops it.

## 2. Applicability

- Candidate: a Horizon commit that contains `crates/horizon-chromecast` with
  `Transport::Progressive`.
- Platforms: Linux, macOS and Windows hosts on the same local network as the
  receiver.
- This procedure does not test the HLS transport. The `horizon-chromecast` unit
  tests cover it.
- This procedure does not test the Horizon UI.

## 3. Equipment and preconditions

- A receiver on the local network, for example a TV with Chromecast built-in.
- The IP address of the receiver.
- The owner of the receiver gave permission for this run.
- A Rust toolchain and `ffmpeg` with `libx264` on the host.
- A stopwatch.
- No other sender casts to the receiver.

## 4. Setup

1. Go to the root of the candidate checkout.

   Result: The directory contains `Cargo.toml`.

2. Make a test file with one keyframe each 0.5 seconds:

   ```bash
   ffmpeg -f lavfi -i testsrc2=size=1280x720:rate=30 -t 120 \
     -c:v libx264 -g 15 -bsf:v h264_metadata=aud=insert -f h264 live-test.h264
   ```

   Result: The file `live-test.h264` exists.

3. Build the live example:

   ```bash
   cargo build -p horizon-chromecast --example live
   ```

   Result: The build completes without errors.

## 5. Tasks

### 5.1 CC-PROG-START — Start a live cast

1. Start the stopwatch and the live example at the same time:

   ```bash
   cargo run -p horizon-chromecast --example live -- <receiver-ip> live-test.h264 30
   ```

   Result: The terminal shows `serving http://<host-ip>:<port>/... (0.50 s segments)`.

2. Look at the terminal.

   Result: The state goes from `Connecting` to `Buffering` and then to `Playing`.

3. Look at the receiver.

   Result: The receiver shows the test picture, and the picture moves.

### 5.2 CC-PROG-LAG — Measure the lag

1. Wait 15 seconds after the state `Playing`.

   Result: The playback on the receiver is at normal speed.

2. Compare the time counter in the test picture with the stopwatch.

   Result: The difference is 1 second or less.

3. Wait 60 seconds more.

   Result: The state stays `Playing`. The picture does not stop or jump back.

### 5.3 CC-PROG-END — Stop the cast at the receiver

1. Stop the cast with the remote control of the receiver or the Google Home app.

   Result: The receiver stops the playback.

2. Look at the terminal.

   Result: The state changes to `Ended`. The live example stops without an error.

### 5.4 CC-PROG-TAKEOVER — Another sender takes over

1. Do the steps in task CC-PROG-START again.

   Result: The state is `Playing`.

2. Start a cast from a different sender to the same receiver, for example from a phone.

   Result: The receiver shows the media from the other sender.

3. Look at the terminal.

   Result: The state changes to `Ended`. The live example stops without an error.

4. Look at the receiver.

   Result: The media from the other sender continues to play.

## 6. Pass criteria

- The state became `Playing` in 10 seconds or less in task CC-PROG-START.
- The lag was 1 second or less in task CC-PROG-LAG.
- The live example ended without an error in task CC-PROG-END.
- The live example ended and left the other sender playing in task
  CC-PROG-TAKEOVER.

## 7. Cleanup

1. If the other sender still casts, stop its cast.

   Result: The receiver shows its home screen.

2. Remove the test file:

   ```bash
   rm live-test.h264
   ```

   Result: The file `live-test.h264` does not exist.

## 8. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). If not, put the results in the
pull request. Do not record the IP address of the receiver or the network names.
