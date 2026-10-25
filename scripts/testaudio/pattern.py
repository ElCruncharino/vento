#!/usr/bin/env python3
# SPDX-FileCopyrightText: 2026 Patrick Gaskin
# SPDX-License-Identifier: GPL-3.0-or-later
"""The client's share of a session's latency, for a far end's rhythm: run by
pattern.sh once the `audio_pattern` example's stream is on the null sink.
Records the sink's monitor, says `go` on OUT_DIR/ctl, and matches each
ONSET line the example printed (the wall time its burst was written into the
stream) to the burst's first loud frame in the recording. Per burst: the
latency, 5 ms gaps and discontinuities in its half second; and the queue's
underruns, target and margin over the run from the example's STAT lines.

  pattern.py OUT_DIR SECONDS        (record, then analyse)
  pattern.py OUT_DIR --analyse      (analyse a previous run again)

Writes OUT_DIR/rec.raw, timeline.json and pattern.json. Standard library only.
"""

import json
import os
import re
import sys
import threading
import time

sys.dont_write_bytecode = True
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import analyse  # noqa: E402
import bursts  # noqa: E402

RATE = 48000
AMPLITUDE = 16384


def run(out, seconds):
    chunks, stop = [], threading.Event()
    rec = threading.Thread(target=bursts.record, args=(os.path.join(out, "rec.raw"), "vento_test", chunks, stop))
    rec.start()
    time.sleep(0.7)
    wall0, mono0 = time.time(), time.monotonic()
    with open(os.path.join(out, "ctl"), "w") as f:
        f.write("go\n")
    time.sleep(seconds + 2.5)
    stop.set()
    rec.join()
    with open(os.path.join(out, "timeline.json"), "w") as f:
        json.dump({"wall0": wall0, "mono0": mono0, "chunks": chunks}, f)


def analyse_run(out):
    tl = json.load(open(os.path.join(out, "timeline.json")))
    wall0, mono0 = tl["wall0"], tl["mono0"]
    x, rate, _ = analyse.load(os.path.join(out, "rec.raw"), RATE, 2)
    # Each piece parec hands over arrives after its last frame was captured,
    # so the earliest arrival less its length bounds frame 0 (as analyse.py).
    offset = min(t - n / rate for t, n in tl["chunks"])
    client = open(os.path.join(out, "client.out"), errors="replace").read()
    onsets = []
    for m in re.finditer(r"ONSET wall=([\d.]+) idx=(\d+) frames=(\d+) rate=(\d+)", client):
        onsets.append(float(m[1]) - wall0 + mono0 + int(m[2]) / int(m[4]))
    expected = AMPLITUDE / 2 ** 0.5
    n5 = rate // 200
    per = []
    for t in onsets:
        a = int((t - offset) * rate)
        if a < 0 or a >= len(x):
            continue
        i = next((j for j in range(a, min(a + rate, len(x))) if abs(x[j]) > 2000), None)
        if i is None:
            per.append({"t": round(t - mono0, 2), "latency_ms": None, "gaps_5ms": None})
            continue
        end = min(i + rate // 2 - n5, len(x))
        gaps = sum(1 for j in range(i, end - n5, n5) if analyse.rms(x, j, j + n5) < expected / 4)
        disc = len(analyse.glitches(x, i + rate // 50, end - rate // 50, rate, 440, AMPLITUDE))
        per.append({"t": round(t - mono0, 2), "latency_ms": round((offset + i / rate - t) * 1000, 1),
                    "gaps_5ms": gaps, "discontinuities": disc})
    stats = []
    for m in re.finditer(r"STAT t=([\d.]+) (.*)", client):
        line = m[2]
        num = lambda key: int(re.search(key + r" (\d+)", line)[1]) if re.search(key + r" (\d+)", line) else None
        stats.append({"t": float(m[1]), "queue_ms": num("queue"), "target_ms": num("target"),
                      "margin_ms": num("margin"), "underruns": num("underruns"), "dropped": num("dropped")})
    lat = sorted(b["latency_ms"] for b in per if b["latency_ms"] is not None)
    res = {
        "bursts": per,
        "stats": stats,
        "n": len(lat),
        "onsets": len(onsets),
        "missing": sum(1 for b in per if b["latency_ms"] is None),
        "latency_ms": {"min": lat[0], "median": lat[len(lat) // 2], "max": lat[-1]} if lat else None,
        "gaps_5ms": sum(b["gaps_5ms"] or 0 for b in per),
        "discontinuities": sum(b.get("discontinuities") or 0 for b in per),
        "bursts_with_gaps": [b["t"] for b in per if b["gaps_5ms"]],
    }
    json.dump(res, open(os.path.join(out, "pattern.json"), "w"), indent=1)
    print("client share ms:", res["latency_ms"], "bursts", res["n"], "of", res["onsets"],
          "gaps_5ms", res["gaps_5ms"], "discontinuities", res["discontinuities"])
    if res["bursts_with_gaps"]:
        print("bursts with gaps at t =", res["bursts_with_gaps"])
    if stats:
        last = stats[-1]
        print("queue at the end:", {k: last[k] for k in ("queue_ms", "target_ms", "margin_ms", "underruns", "dropped")})


def main(argv):
    out = argv[1]
    if argv[2] != "--analyse":
        run(out, float(argv[2]))
    analyse_run(out)


if __name__ == "__main__":
    main(sys.argv)
