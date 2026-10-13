"""Original synthesized score for the Orbitra Tech launch video.

Deterministic (seeded), offline, no samples. 120 BPM so every scene cut
(3, 8, 13, 18, 23s) lands on a downbeat. Hits are placed on the exact
animation times in index.html.

    python3 audio/score.py assets/audio/score-raw.wav
"""
import sys
import wave

import numpy as np

SR = 48000
DUR = 30.0
N = int(SR * DUR)
BEAT = 0.5  # 120 BPM
rng = np.random.default_rng(7)

L = np.zeros(N)
R = np.zeros(N)


def t_arr(d):
    return np.arange(int(d * SR)) / SR


def add(sig, at, gain=1.0, pan=0.0):
    i = int(at * SR)
    if i >= N:
        return
    sig = sig[: N - i] * gain
    L[i : i + len(sig)] += sig * np.sqrt(0.5 * (1 - pan))
    R[i : i + len(sig)] += sig * np.sqrt(0.5 * (1 + pan))


def onepole_lp(x, cutoff):
    """One-pole lowpass; cutoff may be scalar or per-sample array (Hz)."""
    cutoff = np.broadcast_to(np.asarray(cutoff, dtype=float), x.shape)
    a = 1 - np.exp(-2 * np.pi * cutoff / SR)
    y = np.empty_like(x)
    z = 0.0
    for i in range(len(x)):
        z += a[i] * (x[i] - z)
        y[i] = z
    return y


def hp(x, cutoff):
    return x - onepole_lp(x, cutoff)


def midi(n):
    return 440.0 * 2 ** ((n - 69) / 12)


def saw(f, d, detune=0.0):
    t = t_arr(d)
    ph = (f * (1 + detune)) * t
    return 2 * (ph - np.floor(ph + 0.5))


def env_ad(d, a, decay_tau):
    t = t_arr(d)
    e = np.exp(-t / decay_tau)
    if a > 0:
        e *= np.clip(t / a, 0, 1)
    return e


# ---------- instruments ----------
def kick(d=0.45, hard=1.0):
    t = t_arr(d)
    f = 45 + 110 * np.exp(-t / 0.035)
    ph = 2 * np.pi * np.cumsum(f) / SR
    body = np.sin(ph) * np.exp(-t / (0.16 * hard))
    click = rng.standard_normal(len(t)) * np.exp(-t / 0.003) * 0.3
    return np.tanh((body + click) * 1.6)


def clap(d=0.25):
    t = t_arr(d)
    n = rng.standard_normal(len(t))
    n = hp(onepole_lp(n, 4500), 900)
    e = np.exp(-t / 0.06)
    for k in (0.0, 0.011, 0.022):  # flam
        e += (t >= k) * np.exp(-np.clip(t - k, 0, None) / 0.008) * 0.6
    return n * e


def hat(d=0.06, open_=False):
    t = t_arr(d if not open_ else 0.22)
    n = hp(rng.standard_normal(len(t)), 7000)
    return n * np.exp(-t / (0.018 if not open_ else 0.08))


def bass_note(n, d):
    f = midi(n)
    s = saw(f, d) * 0.6 + np.sin(2 * np.pi * f * t_arr(d)) * 0.7
    s = onepole_lp(s, 600)
    return s * env_ad(d, 0.004, d * 0.9)


def pad_chord(notes, d, cutoff=2200, attack=0.25):
    s = np.zeros(int(d * SR))
    for n in notes:
        for det in (-0.004, 0.0, 0.0045):
            s += saw(midi(n), d, det)
    s /= len(notes) * 3
    s = onepole_lp(s, cutoff)
    t = t_arr(d)
    e = np.clip(t / attack, 0, 1) * np.clip((d - t) / 0.15, 0, 1)
    return s * e


def pluck(n, d=0.35, bright=1.0):
    t = t_arr(d)
    f = midi(n)
    s = (np.sin(2 * np.pi * f * t) + 0.5 * bright * np.sin(4 * np.pi * f * t)
         + 0.25 * bright * np.sin(6 * np.pi * f * t))
    return s * np.exp(-t / 0.12) * np.clip(t / 0.002, 0, 1)


def bell(n, d=1.6):
    t = t_arr(d)
    f = midi(n)
    s = np.sin(2 * np.pi * f * t + 1.8 * np.sin(2 * np.pi * f * 3.5 * t) * np.exp(-t / 0.4))
    return s * np.exp(-t / 0.55) * np.clip(t / 0.003, 0, 1)


def whoosh(d, end_at_peak=True, lo=300, hi=6000):
    """Filtered-noise sweep that swells into its end (a cut)."""
    t = t_arr(d)
    x = t / d
    cut = lo * (hi / lo) ** x
    n = onepole_lp(rng.standard_normal(len(t)), cut)
    e = x ** 2.2 if end_at_peak else np.sin(np.pi * x)
    return n * e * 2.2


def riser(d, lo=200, hi=5000):
    t = t_arr(d)
    x = t / d
    n = onepole_lp(rng.standard_normal(len(t)), lo * (hi / lo) ** x)
    f = midi(57) * 2 ** (x * 1.0)
    tone = np.sin(2 * np.pi * np.cumsum(f) / SR) * 0.25
    return (n + tone) * x ** 1.8


def impact(d=1.6):
    t = t_arr(d)
    f = 38 + 80 * np.exp(-t / 0.05)
    boom = np.sin(2 * np.pi * np.cumsum(f) / SR) * np.exp(-t / 0.45)
    crash = hp(rng.standard_normal(len(t)), 3000) * np.exp(-t / 0.5) * 0.35
    return np.tanh(boom * 1.4) + crash


# ---------- arrangement ----------
# I–V–vi–IV in C: C, G, Am, F  (one chord per 2 s bar)
PROG = [
    ([60, 64, 67, 71], 36),  # Cmaj7
    ([59, 62, 67, 74], 43),  # G/B-ish
    ([57, 60, 64, 67], 45),  # Am7
    ([57, 60, 65, 69], 41),  # Fmaj7
]

# Intro 0–3: soft pad, logo hits, riser into the drop
add(pad_chord([57, 60, 64, 69], 3.0, cutoff=900, attack=0.8), 0.0, 0.35)
add(impact(1.4), 0.05, 0.55)                       # disc pop
add(bell(81), 0.18, 0.16, -0.3)                     # logo spin-in
add(bell(88), 0.85, 0.12, 0.3)                      # wordmark
add(bell(84), 1.2, 0.09, -0.2)                      # tagline
add(riser(1.3), 1.7, 0.22)
add(whoosh(0.5), 2.5, 0.28)                         # disc floods frame

# Groove 3–29
pad_bus = np.zeros(N)
for b, start in enumerate(np.arange(3.0, 29.0, 2.0)):
    notes, root = PROG[b % 4]
    lift = start >= 23.0
    i = int(start * SR)
    seg = pad_chord([n + (12 if lift else 0) for n in notes], 2.0, cutoff=2600 if lift else 1900, attack=0.05)
    pad_bus[i : i + len(seg)] += seg[: N - i]
    for k in range(8):  # bass 8ths
        t0 = start + k * 0.25
        add(bass_note(root + (12 if k % 2 else 0), 0.24), t0, 0.32)
    for k in range(4):
        t0 = start + k * BEAT
        add(kick(hard=1.15 if lift else 1.0), t0, 0.85)
        if k in (1, 3):
            add(clap(), t0, 0.32, 0.1)
        add(hat(open_=(k == 3)), t0 + 0.25, 0.10, 0.35)
        add(hat(), t0 + 0.125, 0.05, -0.35)
        add(hat(), t0 + 0.375, 0.05, -0.35)
    if 8.0 <= start < 23.0:  # feature scenes: 16th arp
        arp = [notes[0] + 12, notes[1] + 12, notes[2] + 12, notes[3] + 12]
        for k in range(16):
            add(pluck(arp[(k * 3) % 4], 0.2), start + k * 0.125, 0.085, -0.4 if k % 2 else 0.4)

# Sidechain pump on the pad (duck on each beat)
duck = np.ones(N)
tt = np.arange(N) / SR
for t0 in np.arange(3.0, 29.0, BEAT):
    i = int(t0 * SR)
    seg = 1 - 0.65 * np.exp(-tt[: int(BEAT * SR)] / 0.09)
    duck[i : i + len(seg)] = seg[: N - i]
add(pad_bus * duck, 0.0, 0.42)

# Kinetic headline word pops (index.html wt[])
for i, (t0, n) in enumerate(zip([3.08, 3.6, 3.98, 4.38, 4.98, 5.36], [72, 76, 79, 84, 79, 88])):
    add(pluck(n, 0.3, bright=1.4), t0, 0.22, (-0.3, 0.3)[i % 2])
add(whoosh(0.35, end_at_peak=False, lo=800, hi=8000), 6.05, 0.16)  # headline stack
for k, n in enumerate([72, 76, 79]):
    add(bell(n + 12, 1.0), 6.15 + k * 0.1, 0.06)

# Scene wipes (cover at t-0.24, reveal at t+0.02)
for t0 in (8.0, 13.0, 18.0, 23.0):
    add(whoosh(0.3), t0 - 0.3, 0.3)
    add(whoosh(0.35, end_at_peak=False, lo=2000, hi=9000), t0 + 0.02, 0.12)

# Feature reveal accents
for s in (8.0, 13.0, 18.0):
    add(bell(84, 0.9), s + 0.16, 0.07, 0.2)
for t0, n in ((13.85, 79), (14.15, 83), (14.45, 86)):  # floating pills
    add(pluck(n + 12, 0.25, bright=1.6), t0, 0.12, 0.3)

# CTA 23–30
add(impact(1.8), 23.0, 0.6)
add(bell(84), 23.05, 0.12)
add(pluck(84, 0.4, 1.5), 24.0, 0.2)
add(pluck(88, 0.4, 1.5), 24.05, 0.16)
add(bell(91, 1.2), 24.8, 0.08)                      # URL underline
for t0 in (25.8, 26.4, 28.0):                       # button pulses
    add(pluck(91, 0.25, 1.2), t0, 0.09)

# Ending: final chord + boom at 29, drums out
add(impact(1.2), 29.0, 0.5)
add(pad_chord([48, 60, 64, 67, 72, 76], 1.0, cutoff=3000, attack=0.01), 29.0, 0.55)
add(bell(84, 1.0), 29.0, 0.12)

# ---------- master ----------
mix = np.stack([L, R], axis=1)
fade_in = np.clip(tt / 0.02, 0, 1)
fade_out = np.clip((DUR - tt) / 0.45, 0, 1)
mix *= (fade_in * fade_out)[:, None]
mix = np.tanh(mix * 1.3) / np.tanh(1.3)
mix /= np.max(np.abs(mix)) / 0.89

out = sys.argv[1] if len(sys.argv) > 1 else "score-raw.wav"
pcm = (mix * 32767).astype("<i2")
with wave.open(out, "wb") as w:
    w.setnchannels(2)
    w.setsampwidth(2)
    w.setframerate(SR)
    w.writeframes(pcm.tobytes())
print("wrote", out)
