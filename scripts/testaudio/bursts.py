#!/usr/bin/env python3
# SPDX-FileCopyrightText: 2026 Patrick Gaskin
# SPDX-License-Identifier: GPL-3.0-or-later
"""Latency end to end, from a guest's tone to the null sink: asks the QEMU
audio guest (scripts/testqemu, AUDIO=1 or AUDIO=spice) for a 440 Hz burst on a fixed
schedule, records the sink's monitor, and finds each burst in the recording.

  bursts.py OUT_DIR [--seconds 60] [--every 2] [--host 127.0.0.1]
            [--port 5955] [--marks FILE]

A VNC or SPICE client must already be playing the guest's sound into the null sink
(usage.md: sink.sh load, PIPEWIRE_NODE=vento_test, sink.sh check); the guest
plays silence until it is sent a 't'. Each burst is half a second of 440 Hz
at half scale. Per burst: the latency from the moment the guest's card
played its first tone frame to the moment the sink captured it (the guest's
own share, which it answers on the serial port, taken out: what is left is
QEMU's audio path, the VNC connection, the client's queue and device, and the
capture), how much of it was heard, its pitch, level against the source,
discontinuities and 5 ms gaps. --marks names a file of `time.time()` stamps
and words, one per line (e.g. written by a script that minimises the window),
printed on the same clock. Writes OUT_DIR/rec.raw, timeline.json and
results.json. Standard library only.
"""

import array
import json
import math
import os
import socket
import subprocess
import sys
import threading
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import analyse  # noqa: E402

RATE = 48000
CHANNELS = 2
AMPLITUDE = 16384
BURST_S = 0.5


def record(out, sink, chunks, stop):
    """parec into OUT_DIR/rec.raw, noting when each piece arrived."""
    cmd = ["parec", f"--device={sink}.monitor", "--format=s16le", f"--rate={RATE}",
           f"--channels={CHANNELS}", "--raw", "--latency-msec=5"]
    p = subprocess.Popen(cmd, stdout=subprocess.PIPE, bufsize=0)
    frames = 0
    with open(out, "wb") as f:
        while not stop.is_set():
            data = p.stdout.read(4096)
            if not data:
                break
            now = time.monotonic()
            f.write(data)
            frames += len(data) // (2 * CHANNELS)
            chunks.append((now, frames))
    p.terminate()
    p.wait()


def run(out, seconds, every, host, port, sink):
    os.makedirs(out, exist_ok=True)
    chunks, stop = [], threading.Event()
    recorder = threading.Thread(target=record, args=(os.path.join(out, "rec.raw"), sink, chunks, stop))
    recorder.start()
    s = socket.create_connection((host, port))
    s.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
    replies = []

    def read_replies():
        buf = b""
        while True:
            try:
                data = s.recv(256)
            except OSError:
                return
            if not data:
                return
            buf += data
            while b"\n" in buf:
                line, buf = buf.split(b"\n", 1)
                parts = line.split()
                if len(parts) == 2:
                    replies.append((int(parts[0]), int(parts[1])))

    threading.Thread(target=read_replies, daemon=True).start()
    wall0, mono0 = time.time(), time.monotonic()
    sends = []
    start = mono0 + 1.0
    n = int(seconds / every)
    for i in range(n):
        t = start + i * every
        while time.monotonic() < t:
            time.sleep(min(0.01, max(0.0, t - time.monotonic())))
        sent = time.monotonic()
        s.sendall(b"t")
        sends.append(sent)
    time.sleep(every)
    stop.set()
    recorder.join()
    s.close()
    timeline = {
        "rate": RATE, "channels": CHANNELS, "every": every,
        "wall0": wall0, "mono0": mono0,
        "sends": sends, "replies": replies, "chunks": chunks,
    }
    with open(os.path.join(out, "timeline.json"), "w") as f:
        json.dump(timeline, f)
    return timeline


def analyse_run(out, marks_path=None):
    with open(os.path.join(out, "timeline.json")) as f:
        tl = json.load(f)
    x, rate, _ = analyse.load(os.path.join(out, "rec.raw"), tl["rate"], tl["channels"])
    expected = AMPLITUDE / math.sqrt(2)
    # As analyse.py's rig: each piece arrived after its last frame was
    # captured, so the earliest arrival less its length bounds frame 0, and
    # everything below is late by that bound's slack.
    offset = min(t - frames / rate for t, frames in tl["chunks"])

    def at(t):
        return int(round((t - offset) * rate))

    def when(i):
        return offset + i / rate

    n5 = rate // 200
    bursts = []
    for k, sent in enumerate(tl["sends"]):
        # A far end that plays on a schedule of its own (winbursts.py) has no
        # share to take out: its sends are when the tone started there.
        guest = None if tl.get("guest", True) else 0.0
        if k < len(tl["replies"]):
            us, delay = tl["replies"][k]
            guest = us / 1e6 + delay / rate
        a, b = max(at(sent), 0), min(at(sent + tl["every"] - 0.05), len(x))
        onset = next((i for i in range(a, b) if abs(x[i]) > 2000), None)
        r = {"t": round(sent - tl["mono0"], 3)}
        if onset is None:
            r["heard_ms"] = 0
            bursts.append(r)
            continue
        loud, last = 0, onset
        for i in range(onset, min(onset + int(rate * (BURST_S + 0.1)), len(x)) - n5, n5):
            if analyse.rms(x, i, i + n5) > expected / 4:
                loud += 1
                last = i + n5
        blocks = (last - onset) // n5
        lo, hi = onset + rate // 50, last - rate // 50
        level = analyse.rms(x, lo, hi)
        hz = analyse.pitch(x, lo, hi, rate)
        total = when(onset) - sent
        r.update({
            "latency_ms": round((total - guest) * 1000, 1) if guest is not None else None,
            "total_ms": round(total * 1000, 1),
            "guest_ms": round(guest * 1000, 1) if guest is not None else None,
            "heard_ms": loud * 5,
            "gaps_5ms": blocks - loud,
            "hz": round(hz, 2) if hz else None,
            "level": round(level / expected, 4),
            "discontinuities": len(analyse.glitches(x, lo, hi, rate, 440, AMPLITUDE)),
        })
        bursts.append(r)

    marks = []
    if marks_path:
        with open(marks_path) as f:
            for line in f:
                parts = line.split(None, 1)
                if parts:
                    marks.append((round(float(parts[0]) - tl["wall0"], 3), parts[1].strip() if len(parts) > 1 else ""))

    lat = [b["latency_ms"] for b in bursts if b.get("latency_ms") is not None]

    def stats(v):
        if not v:
            return None
        v = sorted(v)
        return {"min": v[0], "median": v[len(v) // 2], "max": v[-1], "n": len(v)}

    first = [b["latency_ms"] for b in bursts if b.get("latency_ms") is not None and b["t"] < 60]
    lastmin = [b["latency_ms"] for b in bursts if b.get("latency_ms") is not None and b["t"] >= bursts[-1]["t"] - 60]
    results = {
        "capture_offset_s": offset,
        "seconds": round(len(x) / rate, 2),
        "latency_ms": stats(lat),
        "latency_first_minute_ms": stats(first),
        "latency_last_minute_ms": stats(lastmin),
        "missing": sum(1 for b in bursts if b["heard_ms"] == 0),
        "short": sum(1 for b in bursts if 0 < b["heard_ms"] < BURST_S * 1000 - 10),
        "gaps_5ms": sum(b.get("gaps_5ms", 0) for b in bursts),
        "discontinuities": sum(b.get("discontinuities", 0) for b in bursts),
        "hz": stats([b["hz"] for b in bursts if b.get("hz")]),
        "level": stats([b["level"] for b in bursts if b.get("level")]),
        "marks": marks,
        "bursts": bursts,
    }
    with open(os.path.join(out, "results.json"), "w") as f:
        json.dump(results, f, indent=1)

    events = sorted([(b["t"], b) for b in bursts] + [(t, w) for t, w in marks], key=lambda e: e[0])
    print("     t  latency  (total guest)  heard  gaps     Hz   level  discont")
    for t, e in events:
        if isinstance(e, str):
            print(f"{t:6.1f}  --- {e}")
        elif e["heard_ms"] == 0:
            print(f"{t:6.1f}  not heard")
        else:
            print(f"{t:6.1f} {e['latency_ms'] or 0:7.1f}  ({e['total_ms']:6.1f} {e['guest_ms'] or 0:5.1f})"
                  f" {e['heard_ms']:5} {e['gaps_5ms']:5} {e['hz'] or 0:7.2f} {e['level']:7.4f} {e['discontinuities']:5}")
    for k in ("seconds", "latency_ms", "latency_first_minute_ms", "latency_last_minute_ms", "missing", "short",
              "gaps_5ms", "discontinuities", "hz", "level"):
        print(f"{k}: {results[k]}")
    return results


def main(argv):
    if not argv or argv[0].startswith("-"):
        print(__doc__)
        return 2
    out = argv[0]
    opts = {"--seconds": "60", "--every": "2", "--host": "127.0.0.1", "--port": "5955", "--marks": None,
            "--sink": os.environ.get("SINK", "vento_test")}
    rest = argv[1:]
    analyse_only = "--analyse" in rest
    rest = [a for a in rest if a != "--analyse"]
    for k, v in zip(rest[::2], rest[1::2]):
        if k not in opts:
            print(__doc__)
            return 2
        opts[k] = v
    if not analyse_only:
        run(out, float(opts["--seconds"]), float(opts["--every"]), opts["--host"], int(opts["--port"]), opts["--sink"])
    analyse_run(out, opts["--marks"])
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
