#!/usr/bin/env python3
"""Report capture level for a WAV file and judge its fitness for speech recognition.

Standard library only, so it runs wherever Python 3 does. Reads PCM WAV at any
common sample width and prints peak, RMS, clipping and a verdict.
"""

import math
import struct
import sys
import wave


def read_mono_samples(path):
    """Return (samples normalized to [-1, 1], sample rate, channels)."""
    with wave.open(path, "rb") as handle:
        channels = handle.getnchannels()
        width = handle.getsampwidth()
        rate = handle.getframerate()
        raw = handle.readframes(handle.getnframes())

    if width == 1:
        # 8-bit WAV is unsigned, centred on 128.
        values = [(byte - 128) / 128.0 for byte in raw]
    elif width == 2:
        count = len(raw) // 2
        values = [v / 32768.0 for v in struct.unpack(f"<{count}h", raw[: count * 2])]
    elif width == 3:
        values = []
        for i in range(0, len(raw) - 2, 3):
            v = int.from_bytes(raw[i : i + 3], "little", signed=True)
            values.append(v / 8388608.0)
    elif width == 4:
        count = len(raw) // 4
        values = [v / 2147483648.0 for v in struct.unpack(f"<{count}i", raw[: count * 4])]
    else:
        raise SystemExit(f"unsupported sample width: {width} bytes")

    if channels > 1:
        frames = len(values) // channels
        values = [
            sum(values[f * channels : (f + 1) * channels]) / channels for f in range(frames)
        ]
    return values, rate, channels


def main():
    if len(sys.argv) != 2:
        raise SystemExit("usage: level.py <file.wav>")

    samples, rate, channels = read_mono_samples(sys.argv[1])
    if not samples:
        raise SystemExit("file contains no audio")

    peak = max(abs(s) for s in samples)
    rms = math.sqrt(sum(s * s for s in samples) / len(samples))
    clipped = sum(1 for s in samples if abs(s) >= 0.999)
    pct = rms * 100
    duration = len(samples) / rate

    print(f"duration:  {duration:.1f}s  ({rate} Hz, {channels}ch source)")
    print(f"peak:      {peak * 100:.1f}% of full scale")
    print(f"rms:       {pct:.2f}% of full scale")
    print(f"clipped:   {clipped} samples")

    if peak < 0.006:
        verdict = "NO SIGNAL - capture is muted, or no microphone is connected"
    elif clipped > 20:
        verdict = f"CLIPPING ({clipped} samples) - reduce gain, turn boost off first"
    elif pct > 25:
        verdict = "TOO HOT - reduce gain slightly"
    elif pct >= 5:
        verdict = "IDEAL for speech recognition"
    elif pct >= 2:
        verdict = "USABLE - a little louder would help"
    else:
        verdict = "TOO QUIET - raise gain or move closer"

    print(f"verdict:   {verdict}")

    if peak < 0.006:
        print()
        print("A transcript produced from this file is hallucinated, not misheard.")
        print("Fix capture before drawing any conclusion about the model.")


if __name__ == "__main__":
    main()
