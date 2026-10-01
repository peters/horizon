#!/usr/bin/env python3
"""Music bed, interface sounds and the voice, mixed to one stereo wav.

usage: audio.py [out.wav]        (default: <demo dir>/video/mix.wav)

The demo dir is $DEMO_DIR, or the parent of the directory this script lives in.
Reads  <demo dir>/video/plan.json  and  <demo dir>/audio/request.wav.

The score is synthesized from scratch (numpy only, seeded, deterministic):
96 BPM, Am9 - Fmaj7(9) - C(maj9) - G6/9 loop, detuned band-limited supersaw pads with a
slow filter sweep, FM kalimba arpeggio with a ping-pong echo, round sub bass, soft kick
with side-chain ducking, swung hats/shaker/clap, a riser into the intro->demo
transition, an FDN reverb, a glue compressor and a final Am9 resolve at the outro hit.
"""
import json, os, sys, wave
import numpy as np

S = os.environ.get("DEMO_DIR") or os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SR = 44100


# ---- small DSP helpers ---------------------------------------------------------------
def hz(midi):
    return 440.0 * 2 ** ((np.asarray(midi, dtype=float) - 69) / 12)


def smooth(x):
    """Smoothstep 0..1 (click-free ramp)."""
    x = np.clip(x, 0.0, 1.0)
    return x * x * (3 - 2 * x)


def note_env(length, attack, release):
    """Smooth attack / release envelope over `length` samples (no clicks at either end)."""
    x = np.arange(length) / SR
    return smooth(x / max(attack, 1e-3)) * smooth((length / SR - x) / max(release, 1e-3))


def pan_gains(p):
    return np.cos(p * np.pi / 2), np.sin(p * np.pi / 2)


def put_into(buf, sig, t0, gl=1.0, gr=1.0):
    """Add a mono signal into a stereo buffer at time t0 (seconds); may start before 0."""
    i0 = int(round(t0 * SR))
    n = buf.shape[1]
    a = max(0, -i0)
    b = min(len(sig), n - i0)
    if b <= a:
        return
    seg = sig[a:b]
    buf[0, i0 + a : i0 + b] += seg * gl
    buf[1, i0 + a : i0 + b] += seg * gr


def put_stereo(buf, sig, t0):
    i0 = int(round(t0 * SR))
    n = buf.shape[1]
    a = max(0, -i0)
    b = min(sig.shape[1], n - i0)
    if b <= a:
        return
    buf[:, i0 + a : i0 + b] += sig[:, a:b]


def _fft_len(m):
    return 1 << int(np.ceil(np.log2(max(m, 2))))


def fft_filter(x, mag_fn):
    """Zero-phase filter a 1-D signal with the magnitude response mag_fn(freqs)."""
    nfft = _fft_len(len(x))
    f = np.fft.rfftfreq(nfft, 1 / SR)
    return np.fft.irfft(np.fft.rfft(x, nfft) * mag_fn(f), nfft)[: len(x)]


def lowpass_mag(fc, order=4):
    return lambda f: 1.0 / np.sqrt(1.0 + (f / fc) ** (2 * order))


def highpass_mag(fc, order=2):
    return lambda f: 1.0 - 1.0 / np.sqrt(1.0 + (f / fc) ** (2 * order))


def bandpass_mag(lo, hi):
    lp, hp = lowpass_mag(hi, 2), highpass_mag(lo, 2)
    return lambda f: lp(f) * hp(f)


def sweep_filter(x, cutoffs, pos, hp=None):
    """Time-varying low-pass: crossfade pre-filtered copies; pos is a float index into cutoffs per sample."""
    x = np.atleast_2d(x)
    out = np.zeros_like(x)
    nfft = _fft_len(x.shape[1])
    f = np.fft.rfftfreq(nfft, 1 / SR)
    hpm = highpass_mag(hp)(f) if hp else 1.0
    for ch in range(x.shape[0]):
        X = np.fft.rfft(x[ch], nfft) * hpm
        for i, fc in enumerate(cutoffs):
            w = np.clip(1.0 - np.abs(pos - i), 0.0, 1.0)
            if not w.any():
                continue
            y = np.fft.irfft(X * lowpass_mag(fc, 4)(f), nfft)[: x.shape[1]]
            out[ch] += y * w
    return out


def chorus(x, rates=(0.23, 0.31), depth=0.0035, base=0.016, mix=0.55):
    """Light stereo chorus: slowly modulated fractional delay, different per channel."""
    n = x.shape[1]
    idx = np.arange(n)
    out = np.empty_like(x)
    for ch in range(2):
        lfo = np.sin(2 * np.pi * rates[ch] * idx / SR + ch * 1.9) + 0.4 * np.sin(2 * np.pi * rates[ch] * 2.7 * idx / SR + ch)
        d = (base + depth * lfo) * SR
        out[ch] = np.interp(idx - d, idx, x[ch], left=0.0)
    return x * (1 - mix * 0.4) + out * mix


def fdn_reverb(send, rt60=2.6, predelay=0.02, damp=0.3, seed=5):
    """8-line feedback delay network with Hadamard mixing, damped lines and a diffused, pre-delayed input."""
    r = np.random.default_rng(seed)
    n = send.shape[1]
    ir_len = int(0.06 * SR)
    ir = r.standard_normal(ir_len) * np.exp(-np.arange(ir_len) / (0.014 * SR))
    ir /= np.sqrt((ir ** 2).sum())
    pre = int(predelay * SR)
    inp = np.zeros((8, n))
    for ch in range(2):
        nfft = _fft_len(n + ir_len)
        diff = np.fft.irfft(np.fft.rfft(send[ch], nfft) * np.fft.rfft(ir, nfft), nfft)[:n]
        for line in range(ch, 8, 2):
            inp[line, pre:] = diff[: n - pre] * (1 / 2.0)
    delays = np.array([1117, 1307, 1523, 1759, 1999, 2239, 2477, 2741])
    B = int(delays.min())
    H = np.array([[1]])
    for _ in range(3):
        H = np.block([[H, H], [H, -H]])
    H = H / np.sqrt(8)
    g = 10 ** (-3 * delays / (SR * rt60))
    off = int(delays.max())
    x = np.zeros((8, n + off))
    out = np.zeros((2, n))
    sgn_l = H[1].copy()
    sgn_r = H[2].copy()
    ylast = np.zeros(8)
    for s in range(0, n, B):
        e = min(s + B, n)
        Y = np.stack([x[i, off + s - delays[i] : off + e - delays[i]] for i in range(8)])
        shifted = np.concatenate([ylast[:, None], Y[:, :-1]], axis=1)
        Yd = (1 - damp) * Y + damp * shifted
        ylast = Y[:, -1].copy()
        x[:, off + s : off + e] = inp[:, s:e] + g[:, None] * (H @ Yd)
        out[0, s:e] = sgn_l @ Y
        out[1, s:e] = sgn_r @ Y
    for ch in range(2):
        out[ch] = fft_filter(out[ch], highpass_mag(180, 2))
    return out * 0.5


def compress(x, thr_db=-22.0, ratio=2.2, atk=0.02, rel=0.28, knee=8.0, frame=128):
    """Gentle feed-forward glue compressor (stereo linked, frame based gain, smoothed)."""
    n = x.shape[1]
    nf = n // frame + 1
    pk = np.pad(np.abs(x).max(0), (0, nf * frame - n)).reshape(nf, frame).max(1)
    lev = 20 * np.log10(pk + 1e-9)
    over = lev - thr_db
    gr = np.where(over <= -knee / 2, 0.0, np.where(over >= knee / 2, over, (over + knee / 2) ** 2 / (2 * knee)))
    gr *= 1 - 1 / ratio
    ca, cr = np.exp(-frame / (atk * SR)), np.exp(-frame / (rel * SR))
    sm = np.empty(nf)
    s = 0.0
    for i in range(nf):
        c = ca if gr[i] > s else cr
        s = c * s + (1 - c) * gr[i]
        sm[i] = s
    gdb = -np.interp(np.arange(n), (np.arange(nf) + 0.5) * frame, sm)
    return x * 10 ** (gdb / 20)


def limit(x, ceil, window=0.012, frame=32):
    """Look-ahead style peak limiter: min-filtered then averaged gain keeps peaks under the ceiling."""
    n = x.shape[1]
    nf = n // frame + 1
    pk = np.pad(np.abs(x).max(0), (0, nf * frame - n)).reshape(nf, frame).max(1)
    g = np.minimum(1.0, ceil / np.maximum(pk, 1e-9))
    w = max(1, int(window * SR / frame / 2))
    gp = np.pad(g, (w, w), mode="edge")
    win = np.lib.stride_tricks.sliding_window_view(gp, 2 * w + 1)
    gmin = win.min(1)
    gp = np.pad(gmin, (w, w), mode="edge")
    gs = np.convolve(gp, np.ones(2 * w + 1) / (2 * w + 1), mode="valid")
    gain = np.interp(np.arange(n), (np.arange(nf) + 0.5) * frame, gs)
    return x * gain


# ---- band-limited saw oscillator (wavetable per octave, linear interpolation) ------------
TBL_N = 2048
_TABLES = []


def _build_tables():
    ph = np.arange(TBL_N) / TBL_N
    for o in range(11):
        fmax = 40.0 * 2 ** o
        K = max(1, int(0.45 * SR / fmax))
        k = np.arange(1, K + 1)
        _TABLES.append((np.sin(2 * np.pi * np.outer(ph, k)) / k).sum(1) * (2 / np.pi))


def osc_saw(freq, length, phase0=0.0):
    if not _TABLES:
        _build_tables()
    freq = np.broadcast_to(np.asarray(freq, dtype=float), (length,))
    o = int(np.clip(np.log2(max(freq.max(), 20.0) / 20.0), 0, 10))
    tab = _TABLES[o]
    ph = (phase0 + np.cumsum(freq / SR)) % 1.0 * TBL_N
    i0 = ph.astype(int) % TBL_N
    fr = ph - np.floor(ph)
    return tab[i0] * (1 - fr) + tab[(i0 + 1) % TBL_N] * fr


# ---- musical material ------------------------------------------------------------------
BPM = 96.0
BEAT = 60.0 / BPM
BAR = 4 * BEAT                  # 2.5 s: downbeats land on 1.5 s and 4.0 s (intro hit / intro -> demo)
SWING = 0.3                     # odd 16ths are delayed by this fraction of a 16th

CHORDS = [
    dict(pad=[55, 59, 64, 67, 72], pool=[69, 72, 76, 79, 83], root=45),   # Am9
    dict(pad=[57, 60, 64, 67, 69], pool=[69, 72, 76, 79, 81], root=41),   # Fmaj7(9)
    dict(pad=[55, 59, 64, 67, 74], pool=[71, 74, 76, 79, 83], root=48),   # Cmaj9
    dict(pad=[59, 62, 64, 69, 74], pool=[67, 71, 74, 76, 81], root=43),   # G6/9
]
FINAL_PAD = [57, 60, 64, 67, 71, 76]                                      # Am9, resolve
FINAL_RUN = [69, 72, 76, 79, 83, 88]

ARP_SPARSE = [(0, 2, 0.8), (6, 4, 0.6), (10, 3, 0.7)]
ARP_MED = [(0, 1, 0.8), (3, 3, 0.5), (6, 2, 0.7), (8, 4, 0.7), (11, 3, 0.55), (14, 2, 0.65)]
ARP_FULL = [(0, 1, 0.85), (2, 3, 0.5), (3, 4, 0.65), (6, 2, 0.7), (8, 3, 0.8), (10, 4, 0.55), (11, 3, 0.6), (14, 2, 0.7)]


def pad_chunk(notes, length_s, attack, release, seed):
    """One chord of detuned band-limited saws (5 voices per note), slow vibrato, spread across the field."""
    r = np.random.default_rng(seed)
    L = int(length_s * SR)
    x = np.arange(L) / SR
    out = np.zeros((2, L))
    dets = (-13, -6, 0, 6, 12)
    for k, note in enumerate(notes):
        f = float(hz(note))
        for v, det in enumerate(dets):
            vib = 1 + 0.0010 * np.sin(2 * np.pi * (0.16 + 0.043 * ((k + v) % 5)) * x + r.uniform(0, 6.28))
            sig = osc_saw(f * 2 ** (det / 1200) * vib, L, r.uniform())
            p = np.clip(0.5 + 0.42 * (v - 2) / 2 * (1 if k % 2 else -1) + r.uniform(-0.05, 0.05), 0, 1)
            gl, gr = pan_gains(p)
            w = 1.0 / (1 + 0.12 * k)
            out[0] += sig * gl * w
            out[1] += sig * gr * w
    # airy octave-up sine on the top note
    top = float(hz(notes[-1] + 12))
    sh = np.sin(2 * np.pi * top * x + 0.8) * (0.5 + 0.5 * np.sin(2 * np.pi * 0.11 * x))
    out[0] += sh * 0.35
    out[1] += sh * 0.35 * 0.8
    out /= np.sqrt((out ** 2).mean()) * 10
    return out * note_env(L, attack, release)


def pluck(note, dur=1.7):
    """Kalimba-like decaying FM pluck."""
    L = int(dur * SR)
    x = np.arange(L) / SR
    f = float(hz(note))
    idx = 1.9 * np.exp(-x / 0.10) + 0.12
    s = np.sin(2 * np.pi * f * x + idx * np.sin(2 * np.pi * f * x))
    s += 0.14 * np.sin(2 * np.pi * f * 5.4 * x) * np.exp(-x / 0.045)
    s += 0.12 * np.sin(2 * np.pi * f * 2 * x) * np.exp(-x / 0.25)
    env = np.exp(-x / 0.62) * smooth(x / 0.0025) * smooth((dur - x) / 0.25)
    return s * env


def bass_note(midi, dur, vel=1.0):
    """Round sub: pitch-dropped sine with a little saturation, soft attack and release."""
    L = int(dur * SR)
    x = np.arange(L) / SR
    f = float(hz(midi))
    freq = f * (1 + 0.035 * np.exp(-x / 0.025))
    ph = 2 * np.pi * np.cumsum(freq) / SR
    s = np.tanh(1.7 * np.sin(ph) + 0.25 * np.sin(2 * ph)) / np.tanh(1.7)
    env = smooth(x / 0.010) * np.exp(-x / 2.2) * smooth((dur - x) / 0.06)
    return s * env * vel


def kick_hit():
    L = int(0.36 * SR)
    x = np.arange(L) / SR
    freq = 46 + 95 * np.exp(-x / 0.032)
    ph = 2 * np.pi * np.cumsum(freq) / SR
    body = np.sin(ph) * np.exp(-x / 0.15) * smooth(x / 0.002)
    click = np.sin(2 * np.pi * 1800 * x) * np.exp(-x / 0.004) * 0.12
    return (body + click) * smooth((0.36 - x) / 0.06)


def render_music(plan, mrng=None, stems=None):
    """Return the music bed (2, n), before voice ducking. `stems` (a dict) receives the dry buses if given."""
    mrng = mrng or np.random.default_rng(2024)
    TOTAL = plan["total"]
    n = int(TOTAL * SR)
    t = np.arange(n) / SR
    INTRO = plan["intro"]
    FINAL = plan["outro_hit"]
    first = -int(np.ceil(INTRO / BAR))                     # first bar index (starts at or before t = 0)
    nbars = int((FINAL - 0.9 - INTRO) / BAR)               # complete bars before the resolve
    last_bar = nbars - 1                                   # thin "outro" bar: no kick
    bar_t = lambda i: INTRO + i * BAR
    step_t = lambda i, s: bar_t(i) + (s * 0.25 + (SWING * 0.25 if s % 2 else 0.0)) * BEAT

    # --- pads ---
    pad_raw = np.zeros((2, n))
    chunks = [pad_chunk(c["pad"], BAR + 1.0, 0.8, 1.0, 100 + k) for k, c in enumerate(CHORDS)]
    for i in range(first, nbars):
        put_stereo(pad_raw, chunks[i % 4], bar_t(i))
    put_stereo(pad_raw, pad_chunk(FINAL_PAD, TOTAL - FINAL + 0.5, 0.35, 3.0, 200), FINAL)
    pad_gain = np.interp(t, [0, 1.5, INTRO, INTRO + 5, INTRO + 10, INTRO + 20, FINAL - 8, FINAL - 1.0, FINAL, TOTAL],
                         [0.50, 0.62, 0.85, 0.70, 0.78, 0.92, 0.92, 0.65, 1.0, 1.0])
    pad_raw *= pad_gain
    pad = chorus(pad_raw)
    cutoffs = [350, 620, 1050, 1800, 3000, 5000, 8000]
    pos = np.interp(t, [0, 1.5, INTRO, INTRO + 10, INTRO + 24, FINAL - 8, FINAL, TOTAL],
                    [0.6, 1.2, 2.6, 3.2, 4.0, 4.0, 3.0, 2.0])
    pos = np.clip(pos + 0.75 * np.sin(2 * np.pi * t / (4 * BAR)) + 0.3 * np.sin(2 * np.pi * t / 7.3), 0, len(cutoffs) - 1)
    pad = sweep_filter(pad, cutoffs, pos, hp=110)

    # --- arpeggio ---
    arp = np.zeros((2, n))
    for i in range(max(first, 0), nbars):
        if i < 1:
            continue
        pool = CHORDS[i % 4]["pool"]
        if i == last_bar:
            pattern, lvl = ARP_SPARSE, 0.7
        elif i < 4:
            pattern, lvl = ARP_SPARSE, 0.85
        elif i < 8:
            pattern, lvl = ARP_MED, 0.95
        else:
            pattern, lvl = ARP_FULL, 1.0
        for k, (s, idx, vel) in enumerate(pattern):
            if i % 2 and pattern is ARP_FULL:
                idx = 4 - idx if idx in (1, 3, 4) and k % 2 == 0 else idx
            p = 0.5 + 0.28 * (1 if (k + i) % 2 else -1)
            gl, gr = pan_gains(p)
            put_into(arp, pluck(pool[idx]), step_t(i, s), gl * vel * lvl, gr * vel * lvl)
    # final resolve: a soft rising run on the last chord, then one high ring
    for k, (note, off) in enumerate(zip(FINAL_RUN, [0.10, 0.25, 0.43, 0.65, 0.92, 1.30])):
        gl, gr = pan_gains(0.3 + 0.08 * k)
        put_into(arp, pluck(note, 3.0 if k == len(FINAL_RUN) - 1 else 1.7), FINAL + off, gl * (0.75 - 0.05 * k), gr * (0.75 - 0.05 * k))
    # ping-pong echo, dotted eighth, darkened
    mono = fft_filter(arp.sum(0) * 0.5, lowpass_mag(3500, 2))
    echo = np.zeros((2, n))
    for k in range(1, 6):
        d = int(k * 0.75 * BEAT * SR)
        g = 0.5 ** k * 1.15
        gl, gr = pan_gains(0.82 if k % 2 else 0.18)
        echo[0, d:] += mono[: n - d] * g * gl
        echo[1, d:] += mono[: n - d] * g * gr
    arp = arp + echo

    # --- drums ---
    kick_t, kick_amp = [], []
    for i in range(4, last_bar):
        if i in (16, 28):                                  # breathers
            continue
        pat = [(0, 1.0), (6, 0.55), (10, 0.85)] + ([(14, 0.4)] if i % 4 == 3 else [])
        for s, a in pat:
            kick_t.append(step_t(i, s)); kick_amp.append(a)
    kicks = np.zeros((2, n))
    kh = kick_hit()
    for tt, a in zip(kick_t, kick_amp):
        put_into(kicks, kh, tt, 0.7 * a, 0.7 * a)

    hat_noise = fft_filter(mrng.standard_normal(4 * SR), bandpass_mag(7500, 13000))
    hat_noise /= np.abs(hat_noise).max()
    shk_noise = fft_filter(mrng.standard_normal(4 * SR), bandpass_mag(3500, 9500))
    shk_noise /= np.abs(shk_noise).max()
    clp_noise = fft_filter(mrng.standard_normal(4 * SR), bandpass_mag(900, 3800))
    clp_noise /= np.abs(clp_noise).max()

    def noise_hit(src, dur, attack, decay):
        L = int(dur * SR)
        o = int(mrng.integers(0, len(src) - L))
        x = np.arange(L) / SR
        return src[o : o + L] * smooth(x / attack) * np.exp(-x / decay) * smooth((dur - x) / 0.01)

    hats = np.zeros((2, n))
    clap = np.zeros((2, n))
    for i in range(3, last_bar + 1):
        quiet = 0.55 if i == last_bar else 1.0
        for s in range(16):
            acc = [1.0, 0.32, 0.6, 0.32][s % 4]
            gl, gr = pan_gains(0.5 + 0.2 * (1 if s % 2 else -1))
            put_into(hats, noise_hit(shk_noise, 0.11, 0.012, 0.05), step_t(i, s), 0.07 * acc * quiet * gl, 0.07 * acc * quiet * gr)
        if i < 4:
            continue
        for s in (2, 6, 10, 14):
            op = (s == 14 and i % 2 == 1)
            gl, gr = pan_gains(0.62 if s % 4 else 0.4)
            if i == last_bar:
                continue
            put_into(hats, noise_hit(hat_noise, 0.2 if op else 0.07, 0.002, 0.075 if op else 0.016),
                     step_t(i, s), (0.10 if op else 0.075) * gl, (0.10 if op else 0.075) * gr)
        if i >= 8:
            for s in (1, 3, 5, 7, 9, 11, 13, 15):
                gl, gr = pan_gains(0.5 + 0.15 * (1 if s % 4 == 1 else -1))
                put_into(hats, noise_hit(hat_noise, 0.05, 0.002, 0.011), step_t(i, s), 0.03 * gl, 0.03 * gr)
            if i < last_bar:
                for s in (4, 12):
                    for off, a in ((0.0, 1.0), (0.011, 0.7), (0.022, 0.9)):
                        put_into(clap, noise_hit(clp_noise, 0.22, 0.002, 0.07 if off == 0.022 else 0.012),
                                 step_t(i, s) + off, 0.15 * a * 0.707, 0.15 * a * 0.707)

    # --- bass ---
    bass = np.zeros((2, n))
    for i in range(max(first, 0), nbars):
        root = CHORDS[i % 4]["root"]
        if i < 4:
            put_into(bass, bass_note(root, BAR + 0.05, 0.9), bar_t(i), 0.18, 0.18)
            continue
        if i == last_bar:
            put_into(bass, bass_note(root, BAR - 0.1, 0.9), bar_t(i), 0.18, 0.18)
            continue
        pat = [(0, 5), (6, 3), (10, 5)] if i % 2 else [(0, 5), (6, 3), (10, 3), (14, 2)]
        for k, (s, ln) in enumerate(pat):
            nt = root + (12 if (i % 2 == 0 and s == 14) else 0)
            put_into(bass, bass_note(nt, ln * 0.25 * BEAT * 1.02, 1.0 if s == 0 else 0.8), step_t(i, s), 0.18, 0.18)
    put_into(bass, bass_note(45, 5.5, 1.0), FINAL, 0.2, 0.2)

    # --- riser into the intro -> demo transition, soft impact on the downbeat ---
    riser = np.zeros((2, n))
    r0, r1 = 0.3, INTRO
    rl = int((r1 - r0) * SR)
    rx = np.arange(rl) / SR
    rp = (rx / (r1 - r0))
    nz = np.stack([mrng.standard_normal(rl), mrng.standard_normal(rl)])
    rc = [250, 500, 1000, 2000, 4000, 7500, 12000]
    nz = sweep_filter(nz, rc, rp ** 1.5 * 6.0)
    tonal = np.sin(2 * np.pi * np.cumsum(110 * 2 ** (2.6 * rp)) / SR) * 0.5
    tonal += np.sin(2 * np.pi * np.cumsum(110.7 * 2 ** (2.6 * rp)) / SR) * 0.5
    renv = (rp ** 2.2) * smooth(rx / 0.4) * smooth((r1 - r0 - rx) / 0.03)
    riser[:, int(r0 * SR) : int(r0 * SR) + rl] = (nz * 0.11 + tonal * 0.05) * renv
    bx = np.arange(int(1.6 * SR)) / SR
    boom = np.sin(2 * np.pi * np.cumsum(38 + 52 * np.exp(-bx / 0.12)) / SR) * np.exp(-bx / 0.5) * smooth(bx / 0.004) * smooth((1.6 - bx) / 0.2)
    put_into(riser, boom, INTRO, 0.30, 0.30)

    # --- sidechain: pads and bass duck on each kick ---
    def sidechain(depth, rel_t):
        d = np.ones(n)
        k = int((0.008 + rel_t * 1.2) * SR)
        kx = np.arange(k) / SR
        kern = smooth(kx / 0.008) * np.exp(-np.maximum(kx - 0.008, 0) / (rel_t * 0.45))
        kern = kern * smooth((rel_t * 1.2 + 0.008 - kx) / 0.05)
        for tt, a in zip(kick_t, kick_amp):
            i0 = int(tt * SR)
            if i0 >= n:
                continue
            e = min(n, i0 + k)
            d[i0:e] *= 1 - depth * a * kern[: e - i0]
        return d

    pad *= sidechain(0.42, 0.30)
    bass *= sidechain(0.50, 0.22)
    arp *= sidechain(0.12, 0.22)

    # --- bus: dry + reverb send, glue compressor, calibrated level ---
    pad, arp, bass, kicks, hats, clap = pad * 2.8, arp * 0.32, bass * 1.5, kicks * 0.68, hats * 8.0, clap * 8.0
    dry = pad + arp + bass + kicks + hats + clap + riser
    send = pad * 0.5 + arp * 1.0 + clap * 0.9 + hats * 0.15 + riser * 0.6
    wet = fdn_reverb(send)
    if stems is not None:
        stems.update(pad=pad, arp=arp, bass=bass, kicks=kicks, hats=hats, clap=clap, riser=riser, wet=wet * 0.8)
    music = dry + wet * 0.8
    music = compress(music)
    ref = (t > INTRO + 25) & (t < FINAL - 8)
    rms = np.sqrt((music[:, ref] ** 2).mean())
    music *= 10 ** (-20.5 / 20) / rms                       # full section sits around -20.5 dBFS RMS
    # arrangement dynamics: sparse intro, fuller middle, gentle outro (dB re the full section)
    dyn_db = np.interp(t, [0, 1.5, INTRO, INTRO + 2.5, INTRO + 10, INTRO + 20, FINAL - 12, FINAL - 3, FINAL, TOTAL],
                       [-9.0, -7.0, -3.5, -5.0, -2.5, 0.0, 0.0, -1.5, -1.0, -1.0])
    music = music * 10 ** (dyn_db / 20)
    fade = smooth(t / 1.2) * smooth((TOTAL - t) / 2.8)
    return music * fade


def movavg(x, k):
    """Centred moving average of window k, in O(n)."""
    k = max(1, int(k))
    pad = np.pad(x, (k // 2, k - k // 2 - 1), mode="edge")
    c = np.cumsum(np.insert(pad, 0, 0.0))
    return (c[k:] - c[:-k]) / k


def main(out_path=None):
    plan = json.load(open(S + "/video/plan.json"))
    TOTAL = plan["total"]
    n = int(TOTAL * SR)
    rng = np.random.default_rng(7)
    t = np.arange(n) / SR

    music = render_music(plan)

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

    # ---- the voices: the person's request and what the assistant said ------------------
    vox = np.zeros((2, n))
    active = np.zeros(n)
    for item in plan["voices"]:
        w = wave.open(item["file"])
        vr = w.getframerate()
        v = np.frombuffer(w.readframes(w.getnframes()), dtype=np.int16).astype(np.float64) / 32768
        v = np.interp(np.arange(int(len(v) * SR / vr)) / SR, np.arange(len(v)) / vr, v)
        v *= item.get("peak", 0.7) / max(np.abs(v).max(), 1e-6)
        i0 = int(item["at"] * SR)
        if i0 < 0 or i0 >= n:
            continue
        end = min(n, i0 + len(v))
        v = v[: end - i0]
        vox[0, i0:end] += v
        vox[1, i0:end] += v
        win = int(0.15 * SR)
        envelope = np.sqrt(np.maximum(movavg(v * v, win), 0.0))
        active[i0:end] = np.maximum(active[i0:end], (envelope > 0.02).astype(float))
    vox = vox * 0.88 + fdn_reverb(vox, rt60=0.6, predelay=0.008) * 0.2

    # ---- duck the music ~10 dB wherever someone speaks, then mix -----------------------
    hold = int(0.9 * SR)
    spoken = movavg(active, hold) > 0.002
    duck = np.where(spoken, 10 ** (-10 / 20), 1.0)
    k = int(0.3 * SR)
    duck = movavg(duck, k)
    mix = music * duck + sfx + vox
    mix = limit(mix, 0.78)
    mix = mix / np.abs(mix).max() * 0.708                   # peak -3 dBFS
    dither = (np.random.default_rng(3).random((2, n)) - np.random.default_rng(4).random((2, n))) / 32768
    pcm = (np.clip(mix + dither * 0.7, -0.7079, 0.7079).T * 32767).astype(np.int16)
    out_path = out_path or (S + "/video/mix.wav")
    with wave.open(out_path, "wb") as out:
        out.setnchannels(2); out.setsampwidth(2); out.setframerate(SR)
        out.writeframes(pcm.tobytes())
    print("mix", TOTAL, "s")


if __name__ == "__main__":
    main(sys.argv[1] if len(sys.argv) > 1 else None)
