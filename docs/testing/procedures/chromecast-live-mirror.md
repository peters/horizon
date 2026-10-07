---
procedure: chromecast-live-mirror
feature: horizon-chromecast live cast, mirror transport
platforms: [linux, macos, windows]
cost: none
destructive: no
secrets: none
owner: peters
---

# Chromecast mirror live cast test procedure

## 1. Purpose

This procedure proves that a live cast with the mirror transport starts on a
real receiver and plays about a tenth of a second behind the sender. It also
proves that sound plays and that a receiver restart does not end the cast.

## 2. Applicability

- Candidate: a Horizon commit that contains `crates/horizon-chromecast` with
  `Transport::Mirror`.
- Platforms: Linux, macOS and Windows hosts on the same local network as the
  receiver.
- The receiver must have the screen mirroring application (`0F5096E8`), as
  Chromecast devices and TVs with Chromecast built-in do.
- This procedure does not test the Horizon UI.

## 3. Equipment and preconditions

- A receiver on the local network, for example a TV with Chromecast built-in.
- The IP address of the receiver.
- The owner of the receiver gave permission for this run.
- A Rust toolchain and `ffmpeg` with `libx264` on the host.
- A shell that can run the commands in this procedure: Bash, Zsh or PowerShell.
- A stopwatch.
- No other sender casts to the receiver.

## 4. Setup

1. Go to the root of the candidate checkout.

   Result: The directory contains `Cargo.toml`.

2. Make a test file with one keyframe each 0.5 seconds and no B-frames:

   ```bash
   ffmpeg -f lavfi -i testsrc=size=1280x720:rate=30 -t 120 -c:v libx264 -g 15 -bf 0 -bsf:v h264_metadata=aud=insert -f h264 live-test.h264
   ```

   Result: The file `live-test.h264` exists.

3. Make a test sound file with a 440 Hz tone:

   ```bash
   ffmpeg -f lavfi -i "sine=frequency=440:sample_rate=48000:duration=120" -ac 2 -c:a aac -b:a 128k -f adts live-test.aac
   ```

   Result: The file `live-test.aac` exists.

4. Build the live example:

   ```bash
   cargo build -p horizon-chromecast --example live
   ```

   Result: The build completes without errors.

## 5. Tasks

### 5.1 CC-MIRROR-START — Start a mirroring cast

1. Start the stopwatch and the live example at the same time:

   ```bash
   cargo run -p horizon-chromecast --example live -- --mirror <receiver-ip> live-test.h264 30
   ```

   Result: The terminal shows `mirroring to <receiver-ip>:8009`.

2. Look at the terminal.

   Result: The state goes from `Connecting` to `Buffering` and then to `Playing`.

3. Look at the receiver.

   Result: The receiver shows the test picture, and the picture moves smoothly.

### 5.2 CC-MIRROR-LAG — Check the lag

1. Wait 15 seconds after the state `Playing`.

   Result: The playback on the receiver is at normal speed.

2. Compare the seconds counter in the test picture with the stopwatch.

   Result: The counter changes at the same moment as the stopwatch, to the eye.

3. Wait 60 seconds more.

   Result: The state stays `Playing`. The picture does not stop or jump.

4. Stop the live example with Ctrl+C.

   Result: The receiver stops showing the picture.

### 5.3 CC-MIRROR-AUDIO — Mirror with sound

1. Set the receiver volume to a low level.

   Result: The volume indicator on the receiver shows a low level.

2. Wait 5 seconds after the previous cast stopped. Then start the live example
   with the sound file:

   ```bash
   cargo run -p horizon-chromecast --example live -- --mirror <receiver-ip> live-test.h264 30 live-test.aac
   ```

   Result: The state goes to `Playing`.

3. Listen to the receiver.

   Result: A steady tone plays together with the moving test picture. The tone has no gaps.

4. Stop the live example with Ctrl+C.

   Result: The receiver stops the playback.

### 5.4 CC-MIRROR-RESTART — The receiver restarts under the cast

1. Start the live example as in task CC-MIRROR-START, within 1 second after the
   previous cast stopped.

   Result: The state goes to `Playing`.

2. Wait 30 seconds.

   Result: If the receiver restarts its cast runtime (the picture disappears
   about 20 seconds after the start), the state goes to `Buffering` and back to
   `Playing` within 10 seconds. The live example does not stop.

3. Stop the live example with Ctrl+C.

   Result: The receiver stops showing the picture.

## 6. Pass criteria

- The state became `Playing` in 5 seconds or less in task CC-MIRROR-START.
- The picture showed no visible lag against the stopwatch in task CC-MIRROR-LAG.
- The tone played without gaps in task CC-MIRROR-AUDIO.
- The cast continued after a receiver restart in task CC-MIRROR-RESTART.

## 7. Cleanup

> **Caution:** The next step deletes `live-test.h264` and `live-test.aac` from the
> current directory. Check that both files are the ones this run made in task 4
> setup, and not files you need.

1. Remove the test files:

   ```bash
   rm live-test.h264 live-test.aac
   ```

   Result: The files do not exist.

## 8. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). If not, put the results in the
pull request. Do not record the IP address of the receiver or the network names.
