#!/usr/bin/env python3
# SPDX-FileCopyrightText: 2026 Patrick Gaskin
# SPDX-License-Identifier: GPL-3.0-or-later
"""What a recording of vento's sound says: pitch, level, dropouts, and,
for the desktop rig's recording, latency, mute and the drop policy.

  analyse.py summary FILE [--rate 48000 --channels 2] [--tone 440]
      FILE is a .wav (record.sh) or raw S16LE. Per half second: level in
      dBFS, the pitch, and discontinuities; then the stretches of sound and
      silence. For a session played into the null sink (stages 2-6).

  analyse.py rig DIR
      DIR is what audio_rig wrote (rec.raw, timeline.json); prints the
      measurements and writes DIR/results.json.

Standard library only (no numpy on the build host). See usage.md.
"""

import array
import json
import math
import os
import sys
import wave

# --- reading -----------------------------------------------------------------


def load(path, rate=48000, channels=2):
    """The first channel as an array of ints, the rate, and the others."""
    if path.endswith(".wav"):
        with wave.open(path, "rb") as w:
            assert w.getsampwidth() == 2, "16-bit WAV only"
            rate, channels = w.getframerate(), w.getnchannels()
            data = w.readframes(w.getnframes())
    else:
        with open(path, "rb") as f:
            data = f.read()
    samples = array.array("h")
    samples.frombytes(data[: len(data) // (2 * channels) * 2 * channels])
    if sys.byteorder != "little":
        samples.byteswap()
    chans = [samples[c::channels] for c in range(channels)]
    return chans[0], rate, chans[1:]


# --- measuring ---------------------------------------------------------------


def rms(x, a, b):
    a, b = max(a, 0), min(b, len(x))
    if b <= a:
        return 0.0
    return math.sqrt(sum(v * v for v in x[a:b]) / (b - a))


def pitch(x, a, b, rate):
    """Hz, from rising zero crossings placed between samples; None when there
    are too few to say."""
    a, b = max(a, 1), min(b, len(x))
    crossings = []
    for i in range(a, b):
        p, q = x[i - 1], x[i]
        if p < 0 <= q and q != p:
            crossings.append(i - 1 + (-p) / (q - p))
    if len(crossings) < 3:
        return None
    return (len(crossings) - 1) * rate / (crossings[-1] - crossings[0])


def glitches(x, a, b, rate, hz, amplitude):
    """Where the second difference is far past what a sine of this pitch and
    level can make: a skip, a dropped run, a gap starting or ending."""
    limit = max(500.0, 10 * amplitude * (2 * math.pi * hz / rate) ** 2)
    a, b = max(a, 2), min(b, len(x))
    found = []
    last = -rate
    for i in range(a, b):
        if abs(x[i] - 2 * x[i - 1] + x[i - 2]) > limit and i - last > rate // 100:
            found.append(i)
            last = i
    return found


def gaps(x, a, b, rate, expected):
    """5 ms blocks whose level is under a tenth of `expected` RMS."""
    n = rate // 200
    return [i for i in range(max(a, 0), min(b, len(x)) - n, n) if rms(x, i, i + n) < expected / 10]


def db(v):
    return 20 * math.log10(v / 32768) if v > 0 else float("-inf")


# --- summary -----------------------------------------------------------------


def summary(path, rate, channels, tone):
    x, rate, _ = load(path, rate, channels)
    half = rate // 2
    print(f"{path}: {len(x) / rate:.2f} s at {rate} Hz")
    print("   t(s)    dBFS     Hz  discontinuities")
    for a in range(0, len(x) - half + 1, half):
        level = rms(x, a, a + half)
        hz = pitch(x, a, a + half, rate) if level > 30 else None
        g = glitches(x, a, a + half, rate, hz or tone, level * math.sqrt(2)) if hz else []
        print(f"{a / rate:7.2f} {db(level):7.1f} {hz or 0:7.1f}  {len(g)}")
    # Stretches of sound and silence, by 10 ms blocks.
    n = rate // 100
    state, start = None, 0
    print("stretches:")
    for i in range(0, len(x) - n + 1, n):
        loud = rms(x, i, i + n) > 30
        if loud != state:
            if state is not None:
                print(f"  {'sound  ' if state else 'silence'} {start / rate:7.3f} - {i / rate:7.3f}")
            state, start = loud, i
    if state is not None:
        print(f"  {'sound  ' if state else 'silence'} {start / rate:7.3f} - {len(x) / rate:7.3f}")


# --- rig ---------------------------------------------------------------------


def rig(directory):
    with open(os.path.join(directory, "timeline.json")) as f:
        tl = json.load(f)
    x, rate, others = load(os.path.join(directory, "rec.raw"), tl["rate"], tl["channels"])
    amp, tone = tl["amplitude"], tl["tone"]
    expected = amp / math.sqrt(2)
    results = {"stats": tl["stats"]}

    # The recording on the rig's clock: each piece arrived after its last
    # frame was captured, so the earliest arrival less its length bounds where
    # frame 0 was. What is measured below is late by that bound's slack, the
    # capture path's least delay.
    offset = min(t - frames / rate for t, frames in tl["chunks"])
    results["capture_offset_s"] = offset

    def at(t):
        return int(round((t - offset) * rate))

    def when(i):
        return offset + i / rate

    events = tl["events"]

    def times(what):
        return [e["t"] for e in events if e["what"] == what]

    # Latency, write to capture, by the clicks.
    clicks = []
    for w in times("click"):
        a, b = at(w), at(w + 1.0)
        i = next((i for i in range(max(a, 0), min(b, len(x))) if abs(x[i]) > 8000), None)
        clicks.append(None if i is None else round((when(i) - w) * 1000, 1))
    results["click_latency_ms"] = clicks
    burst_t = times("burst")[0]
    resume_t = times("resume")[0]
    before = [c for t, c in zip(times("click"), clicks) if c is not None and t < burst_t]
    after_burst = [c for t, c in zip(times("click"), clicks) if c is not None and burst_t < t < resume_t]
    after_stall = [c for t, c in zip(times("click"), clicks) if c is not None and t > resume_t]

    # The tone, in stretches clear of every change.
    toolbar = {e["what"]: e["t"] for e in events if e["what"] in ("gain 0.5", "gain 1", "mute", "unmute")}
    windows = {
        "tone, gain 1": (1.4, toolbar["gain 0.5"]),
        "tone, session gain 0.5": (toolbar["gain 0.5"] + 0.4, toolbar["gain 1"]),
        "tone, gain 1 again": (toolbar["gain 1"] + 0.4, toolbar["mute"]),
        "muted": (toolbar["mute"] + 0.4, toolbar["unmute"]),
        "unmuted": (toolbar["unmute"] + 0.4, 11.0),
        "tone 13-15 s": (13.4, burst_t),
        "tone after the stall": (resume_t + 0.4, 21.0),
    }
    measured = {}
    for name, (a, b) in windows.items():
        ia, ib = at(a), at(b)
        level = rms(x, ia, ib)
        hz = pitch(x, ia, ib, rate) if level > 30 else None
        g = glitches(x, ia, ib, rate, tone, amp) if hz else []
        gp = gaps(x, ia, ib, rate, expected) if name != "muted" else []
        measured[name] = {
            "seconds": round(b - a, 2),
            "level": round(level / expected, 4),
            "dBFS": round(db(level), 2),
            "hz": round(hz, 2) if hz else None,
            "discontinuities": len(g),
            "gaps_5ms": len(gp),
        }
    results["windows"] = measured
    if others:
        results["channels_equal"] = all(o[: len(x)] == x[: len(o)] for o in others)

    # How soon a change is heard, by 5 ms blocks of level.
    n = rate // 200

    def first_block(t, test, within=1.0):
        for i in range(at(t), at(t + within), n):
            if test(rms(x, i, i + n)):
                return round((when(i) - t) * 1000, 1)
        return None

    results["reaction_ms"] = {
        "mute": first_block(toolbar["mute"], lambda v: v < expected / 100),
        "unmute": first_block(toolbar["unmute"], lambda v: v > expected / 2),
        "gain 0.5": first_block(toolbar["gain 0.5"], lambda v: v < expected * 0.6),
        "resume after the stall": first_block(resume_t, lambda v: v > expected / 2),
    }

    # The burst: how much of its 660 Hz was heard, by 10 ms blocks.
    m = rate // 100
    heard = 0
    for i in range(at(burst_t), at(burst_t + 0.5), m):
        if rms(x, i, i + m) > expected / 2:
            hz = pitch(x, i, i + m, rate)
            if hz and abs(hz - tl["burst_hz"]) < 40:
                heard += 1
    lat = tl["latency"]
    results["burst"] = {
        "written_s": 2.0,
        "heard_ms": heard * 10,
        "latency_after_write_ms": next(e["latency_after"] for e in events if e["what"] == "burst"),
        "queue_latency_max_ms": max(v for _, v in lat),
        "queue_latency_median_ms": sorted(v for _, v in lat)[len(lat) // 2],
    }

    def med(v):
        return sorted(v)[len(v) // 2] if v else None

    results["latency_ms"] = {
        "before_burst": before,
        "after_burst": after_burst,
        "after_stall": after_stall,
        "median": med(before + after_burst + after_stall),
    }

    with open(os.path.join(directory, "results.json"), "w") as f:
        json.dump(results, f, indent=1)

    print(f"recording: {len(x) / rate:.2f} s; capture offset {offset:.4f} s on the rig's clock")
    print(f"queue: {tl['stats']}")
    print("window                     s   level   dBFS      Hz  discont  gaps")
    for name, r in measured.items():
        print(f"{name:24} {r['seconds']:5.2f} {r['level']:7.4f} {r['dBFS']:6.1f} {r['hz'] or 0:7.2f} {r['discontinuities']:8} {r['gaps_5ms']:5}")
    print(f"click latency ms: before burst {before}, after burst {after_burst}, after stall {after_stall}")
    print(f"reaction ms: {results['reaction_ms']}")
    print(f"burst: {results['burst']}")
    if "channels_equal" in results:
        print(f"channels equal: {results['channels_equal']}")


def main(argv):
    if len(argv) >= 2 and argv[0] == "rig":
        rig(argv[1])
    elif len(argv) >= 2 and argv[0] == "summary":
        opts = {"--rate": "48000", "--channels": "2", "--tone": "440"}
        rest = argv[2:]
        for k, v in zip(rest[::2], rest[1::2]):
            opts[k] = v
        summary(argv[1], int(opts["--rate"]), int(opts["--channels"]), float(opts["--tone"]))
    else:
        print(__doc__)
        return 2
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
