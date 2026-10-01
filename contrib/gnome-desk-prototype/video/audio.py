#!/usr/bin/env python3
"""Music bed, interface sounds and the voice, mixed to one stereo wav."""
import json, sys, wave
import numpy as np

S = os.environ["DEMO_DIR"]
SR = 44100
plan = json.load(open(S + "/video/plan.json"))
TOTAL = plan["total"]
n = int(TOTAL * SR)
rng = np.random.default_rng(7)
t = np.arange(n) / SR

def hz(midi): return 440.0 * 2 ** ((midi - 69) / 12)
def env_ad(length, attack, release):
    x = np.arange(length) / SR
    a = np.clip(x / max(attack, 1e-3), 0, 1)
    r = np.clip((length / SR - x) / max(release, 1e-3), 0, 1)
    return np.minimum(a, r)

# ---- music: a slow pad on four chords with a soft arpeggio on top -----------------
bpm = 84.0
beat = 60.0 / bpm
chords = [
    [45, 52, 60, 64, 71],   # Am9
    [41, 48, 57, 64, 69],   # Fmaj7
    [48, 55, 64, 71, 74],   # Cmaj7
    [43, 50, 59, 66, 69],   # G6
]
music = np.zeros((2, n))
bar = 4 * beat
for index in range(int(TOTAL / bar) + 2):
    chord = chords[index % 4]
    start = index * bar
    length = int((bar + 1.2) * SR)
    seg = np.zeros(length)
    x = np.arange(length) / SR
    for note in chord:
        f = hz(note)
        for detune in (-0.12, 0.0, 0.14):
            seg += np.sin(2 * np.pi * f * (1 + detune / 100) * x) * 0.5
            seg += np.sin(2 * np.pi * f * 2 * (1 + detune / 100) * x) * 0.12
    seg *= env_ad(length, 1.1, 1.3) * (0.05 / len(chord))
    i0 = int(start * SR)
    end = min(n, i0 + length)
    if i0 < n:
        music[0, i0:end] += seg[: end - i0]
        music[1, i0:end] += seg[: end - i0]
    # arpeggio: eighth notes, alternating sides
    for step in range(8):
        note = chord[[2, 3, 4, 3, 2, 4, 3, 2][step] % len(chord)] + 12
        ts = start + step * beat / 2
        ln = int(1.6 * SR)
        x2 = np.arange(ln) / SR
        pluck = (np.sin(2 * np.pi * hz(note) * x2) + 0.3 * np.sin(2 * np.pi * hz(note) * 2 * x2)) * np.exp(-x2 * 3.4)
        pluck *= 0.045 * (0.6 + 0.4 * (step % 2 == 0))
        j0 = int(ts * SR)
        if j0 >= n:
            continue
        je = min(n, j0 + ln)
        side = 0 if step % 2 == 0 else 1
        music[side, j0:je] += pluck[: je - j0] * 1.0
        music[1 - side, j0:je] += pluck[: je - j0] * 0.45
    # sub bass on the root
    root = chord[0] - 12
    sub = np.sin(2 * np.pi * hz(root) * x[: int(bar * SR)]) * env_ad(int(bar * SR), 0.4, 0.8) * 0.06
    j0 = int(start * SR)
    je = min(n, j0 + len(sub))
    if j0 < n:
        music[:, j0:je] += sub[: je - j0]

def reverb(sig, seconds=1.8, mix=0.28):
    ir_len = int(seconds * SR)
    decay = np.exp(-np.arange(ir_len) / SR * 3.2)
    ir = rng.standard_normal(ir_len) * decay
    ir[: int(0.012 * SR)] *= 0.2
    out = np.zeros_like(sig)
    for ch in range(sig.shape[0]):
        wet = np.fft.irfft(np.fft.rfft(sig[ch], n=len(sig[ch]) + ir_len) * np.fft.rfft(ir[::1] * (1 + 0.1 * ch), n=len(sig[ch]) + ir_len))[: len(sig[ch])]
        out[ch] = sig[ch] * (1 - mix) + wet * mix * 0.04
    return out

music = reverb(music)
fade = np.clip(t / 2.5, 0, 1) * np.clip((TOTAL - t) / 3.0, 0, 1)
music *= fade

# ---- interface sounds ------------------------------------------------------------
sfx = np.zeros((2, n))
def put(sound, at, gain=1.0, pan=0.5):
    i0 = int(at * SR)
    if i0 < 0 or i0 >= n:
        return
    end = min(n, i0 + len(sound))
    sfx[0, i0:end] += sound[: end - i0] * gain * (1 - pan) * 1.4
    sfx[1, i0:end] += sound[: end - i0] * gain * pan * 1.4

def tick():
    ln = int(0.09 * SR); x = np.arange(ln) / SR
    return (np.sin(2 * np.pi * 1500 * x) * np.exp(-x * 60) + 0.5 * rng.standard_normal(ln) * np.exp(-x * 300)) * 0.5
def whoosh(length=0.55, rising=True):
    ln = int(length * SR); x = np.arange(ln) / SR
    noise = rng.standard_normal(ln)
    # sweep a band by modulating a one-pole filter coefficient
    out = np.zeros(ln); y = 0.0
    for i in range(ln):
        a = 0.02 + 0.5 * (i / ln if rising else 1 - i / ln) ** 1.6
        y += a * (noise[i] - y)
        out[i] = y
    return out * np.sin(np.pi * x / length) ** 1.5 * 0.9
def chime(freqs=(880, 1318.5), length=1.1):
    ln = int(length * SR); x = np.arange(ln) / SR
    return sum(np.sin(2 * np.pi * f * x) * np.exp(-x * (3.0 + k)) for k, f in enumerate(freqs)) * 0.25
def alert():
    return np.concatenate([chime((523.25, 784), 0.6), chime((415.3, 622), 0.9)]) * 0.9

events = plan["events"]
for at in events.get("go", []):
    put(tick(), at, 0.7)
    put(whoosh(0.45), at + 0.02, 0.35)
for at in events.get("done", []):
    put(chime(), at, 0.9, 0.5)
for at in events.get("alert", []):
    put(alert(), at, 0.9, 0.5)
for at in events.get("expand", []):
    put(whoosh(0.5), at, 0.5)
    put(tick(), at + 0.3, 0.5)
for at in events.get("mic", []):
    put(chime((1200,), 0.18), at, 0.7)
put(whoosh(2.2, True), plan["intro_swell"], 0.5)
put(chime((440, 660, 880), 2.4), plan["intro_hit"], 0.8)
put(chime((440, 660, 880), 2.8), plan["outro_hit"], 0.6)

# ---- the voice -------------------------------------------------------------------
w = wave.open(S + "/audio/request.wav")
vr = w.getframerate()
voice = np.frombuffer(w.readframes(w.getnframes()), dtype=np.int16).astype(np.float64) / 32768
voice = np.interp(np.arange(int(len(voice) * SR / vr)) / SR, np.arange(len(voice)) / vr, voice)
voice *= 0.7 / np.abs(voice).max()
vox = np.zeros((2, n))
i0 = int(plan["voice_at"] * SR)
vox[0, i0 : i0 + len(voice)] = voice
vox[1, i0 : i0 + len(voice)] = voice
vox = reverb(vox, 0.5, 0.12) * 1.0

# ---- duck the music under the voice and mix ----------------------------------------
duck = np.ones(n)
d0, d1 = plan["voice_at"] - 0.4, plan["voice_at"] + len(voice) / SR + 0.5
duck[(t > d0) & (t < d1)] = 0.38
k = int(0.25 * SR)
duck = np.convolve(duck, np.ones(k) / k, mode="same")
mix = music * duck * 1.0 + sfx + vox
peak = np.abs(mix).max()
mix = mix / peak * 0.89
pcm = (mix.T * 32767).astype(np.int16)
with wave.open(S + "/video/mix.wav", "wb") as out:
    out.setnchannels(2); out.setsampwidth(2); out.setframerate(SR)
    out.writeframes(pcm.tobytes())
print("mix", TOTAL, "s")
