#!/usr/bin/env python3
# SPDX-FileCopyrightText: 2026 Patrick Gaskin
# SPDX-License-Identifier: GPL-3.0-or-later
"""Latency end to end, from a Windows RDP session's tone to the null sink:
bursts.py for a far end that is a real Windows machine rather than a guest
answering on a serial port.

  winbursts.py OUT_DIR [--seconds 60] [--every 2] [--lead 5] [--check PID]
               [--marks FILE] [--analyse]

An RDP client must already be connected to the machine as its user and
playing into the null sink (usage.md: sink.sh load, PIPEWIRE_NODE=vento_test,
sink.sh check); the session plays nothing until this starts. WIN_SSH is the
command that runs a command on the machine (default `ssh user@10.33.0.208`;
it must take ssh's -L before the command). What it does there, all of it
undone before it returns, whatever happened:

  1. writes scripts/testwin/tone.ps1 to %USERPROFILE%\\vento-tone.ps1 and
     registers it as the task vento-tone with an Interactive principal, which
     is what runs it in the RDP session, where its sound goes to the client;
  2. measures the machine's clock against this one: a loopback listener on
     the machine, reached through `ssh -L`, answers each byte with its precise
     time, and the answer of the shortest round trip (about 1.5 ms on the
     LAN) gives the offset to within half of it; once before and once after,
     and the two are averaged (they say how far the clocks drifted, too);
  3. runs the task, which plays --lead seconds of silence and then half a
     second of 440 Hz at half scale every --every seconds, and notes the
     moment just before it handed the sound to Windows' player; with
     --check, `sink.sh check PID` runs as the silence starts and the task is
     stopped before its first burst unless PID's stream is on the sink (the
     client opens its stream only once there is sound to play, so the
     silence is what the check has to see);
  4. unregisters the task and deletes both files.

Each burst's latency is from that moment (plus the burst's place in the
sound) to the sink capturing its first loud frame: Windows' own player and
audio engine, RDP's sound channel, the client's queue and device, and the
capture, late by the capture path's least delay as with bursts.py. What
Windows' player takes to start is in every burst alike, so absolute values
carry that constant; how they vary over a run is the client's. The analysis
is bursts.py's; writes OUT_DIR/rec.raw, timeline.json and results.json.
"""

import base64
import json
import os
import shlex
import socket
import subprocess
import sys
import threading
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import bursts  # noqa: E402

TONE = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "testwin", "tone.ps1")
TASK = "vento-tone"
REMOTE_DIR = r"C:\Users\user"
PORT = 47123

PRECISE = (
    "Add-Type -Namespace Vento -Name Clock -MemberDefinition '"
    "[DllImport(\"kernel32.dll\")] static extern void GetSystemTimePreciseAsFileTime(out long t); "
    "public static long Now() { long t; GetSystemTimePreciseAsFileTime(out t); return t; }'; "
)


def ssh_base():
    return shlex.split(os.environ.get("WIN_SSH", "ssh user@10.33.0.208"))


def powershell(script, extra=(), **kw):
    enc = base64.b64encode(script.encode("utf-16le")).decode()
    cmd = ssh_base() + list(extra) + [f"powershell -NoProfile -NonInteractive -InputFormat None -EncodedCommand {enc}"]
    return subprocess.run(cmd, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, **kw)


def upload(text, path):
    # Through stdin, which Windows' sshd hands the command whole at EOF:
    # enough for a file, and no scp to configure alongside WIN_SSH.
    cmd = ssh_base() + [f"powershell -NoProfile -NonInteractive -Command \"[IO.File]::WriteAllText('{path}', [Console]::In.ReadToEnd())\""]
    subprocess.run(cmd, input=text.encode(), stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, check=True)


def clock_offset(samples=40):
    """The machine's clock less this one's, in seconds, and the round trip
    it was measured over. The listener runs over ssh in session 0, which is
    fine for a clock; Windows' sshd hands a command its stdin only at EOF,
    hence a socket rather than a pipe. It gives up by itself if nothing
    connects within a minute or sends for ten seconds."""
    script = PRECISE + (
        f"$l=[Net.Sockets.TcpListener]::new([Net.IPAddress]::Loopback, {PORT}); $l.Start(); "
        "$until=(Get-Date).AddSeconds(60); while(-not $l.Pending() -and (Get-Date) -lt $until){ Start-Sleep -Milliseconds 50 }; "
        "if($l.Pending()){ $c=$l.AcceptTcpClient(); $c.NoDelay=$true; $s=$c.GetStream(); $s.ReadTimeout=10000; "
        "$b=New-Object byte[] 1; try { while($s.Read($b,0,1) -eq 1 -and $b[0] -ne 113){ "
        "$t=[Text.Encoding]::ASCII.GetBytes([string][Vento.Clock]::Now() + \"`n\"); $s.Write($t,0,$t.Length) } } catch {}; "
        "$c.Close() }; $l.Stop()")
    enc = base64.b64encode(script.encode("utf-16le")).decode()
    cmd = ["timeout", "60"] + ssh_base() + ["-L", f"{PORT}:127.0.0.1:{PORT}",
                                            f"powershell -NoProfile -NonInteractive -InputFormat None -EncodedCommand {enc}"]
    p = subprocess.Popen(cmd, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    c = f = None
    try:
        for _ in range(100):
            time.sleep(0.2)
            try:
                c = socket.create_connection(("127.0.0.1", PORT), timeout=5)
                c.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
                c.sendall(b"x")
                f = c.makefile("rb")
                if f.readline():
                    break
            except OSError:
                c = None
        if c is None:
            raise RuntimeError("no clock listener on the machine")
        best = None
        for _ in range(samples):
            t0 = time.time()
            c.sendall(b"x")
            line = f.readline()
            t1 = time.time()
            win = (int(line) - 116444736000000000) / 1e7
            if best is None or t1 - t0 < best[0]:
                best = (t1 - t0, win - (t0 + t1) / 2)
            time.sleep(0.02)
        return best[1], best[0]
    finally:
        # A 'q' ends the listener; the connection closing does not reach it
        # through Windows' sshd, and a listener left behind would hold the
        # port until its read timed out.
        if c is not None:
            try:
                c.sendall(b"q")
            except OSError:
                pass
            c.close()
        try:
            p.wait(3)
        except subprocess.TimeoutExpired:
            p.terminate()
            p.wait()


def run(out, seconds, every, lead, check, sink):
    os.makedirs(out, exist_ok=True)
    stamps = REMOTE_DIR + r"\vento-tone.txt"
    script = REMOTE_DIR + r"\vento-tone.ps1"
    with open(TONE) as fh:
        upload(fh.read(), script)
    register = (
        f"$p = New-ScheduledTaskPrincipal -UserId \"$env:COMPUTERNAME\\user\" -LogonType Interactive -RunLevel Highest; "
        f"$a = New-ScheduledTaskAction -Execute 'powershell.exe' -Argument '-NoProfile -ExecutionPolicy Bypass "
        f"-WindowStyle Hidden -File {script} -Seconds {int(seconds)} -Every {every} -Lead {lead} -Out {stamps}'; "
        f"Register-ScheduledTask -TaskName '{TASK}' -Action $a -Principal $p -Force | Out-Null")
    chunks, stop = [], threading.Event()
    recorder = None
    try:
        powershell(register, check=True)
        before, rtt0 = clock_offset()
        recorder = threading.Thread(target=bursts.record, args=(os.path.join(out, "rec.raw"), sink, chunks, stop))
        recorder.start()
        wall0, mono0 = time.time(), time.monotonic()
        time.sleep(1.0)
        powershell(f"Start-ScheduledTask -TaskName '{TASK}'", check=True)
        if check:
            # The task takes a few seconds to build its sound, and sink.sh
            # waits ten for a stream to appear.
            sink_sh = os.path.join(os.path.dirname(os.path.abspath(__file__)), "sink.sh")
            for _ in range(3):
                r = subprocess.run([sink_sh, "check", str(check)], env=dict(os.environ, SINK=sink))
                if r.returncode == 0:
                    break
            else:
                raise RuntimeError("the client's stream is not on the sink; stopped the task")
        lines = []
        deadline = time.monotonic() + seconds + 60
        time.sleep(seconds)
        while time.monotonic() < deadline:
            lines = powershell(f"if (Test-Path {stamps}) {{ Get-Content {stamps} }}").stdout.decode().split()
            if "end" in lines:
                break
            time.sleep(2)
        time.sleep(1.0)
        stop.set()
        recorder.join()
        recorder = None
        after, rtt1 = clock_offset()
    finally:
        stop.set()
        if recorder is not None:
            recorder.join()
        powershell(f"Stop-ScheduledTask -TaskName '{TASK}' -ErrorAction SilentlyContinue; "
                   f"Unregister-ScheduledTask -TaskName '{TASK}' -Confirm:$false -ErrorAction SilentlyContinue; "
                   f"Remove-Item {script},{stamps} -ErrorAction SilentlyContinue")
    words = dict(zip(lines[::2], lines[1::2]))
    if "start" not in words:
        raise RuntimeError(f"the task left no start time: {lines}")
    timeline = {
        "rate": bursts.RATE, "channels": bursts.CHANNELS, "every": every, "lead": lead,
        "count": int(seconds // every), "wall0": wall0, "mono0": mono0, "replies": [], "guest": False,
        "chunks": chunks,
        "clock": {"offset_before_s": before, "rtt_before_s": rtt0, "offset_after_s": after, "rtt_after_s": rtt1,
                  "start": float(words["start"]), "end": float(words.get("end", "nan"))},
    }
    with open(os.path.join(out, "timeline.json"), "w") as fh:
        json.dump(timeline, fh)
    return timeline


def place(out):
    """Puts each burst on this machine's clock from the task's start time,
    as `sends` in timeline.json, which is what bursts.py's analysis reads.

    Windows' time service steps the clock now and then (by 1.8 s, once, on
    the machine here, in the middle of a run), so the offset measured before
    and the one measured after can disagree. The task's own two stamps say
    where the step fell: the sound takes its length to play whatever the
    clock does, so stamps further apart than that by the step mean it fell
    during the sound, after the start was stamped, and the start goes with
    the offset from before. A step that fell outside the sound is in the few
    seconds between a measurement and the task, and is said, not guessed
    at."""
    path = os.path.join(out, "timeline.json")
    with open(path) as fh:
        tl = json.load(fh)
    c = tl["clock"]
    before, after = c["offset_before_s"], c["offset_after_s"]
    played = c["end"] - c["start"]
    length = tl["lead"] + tl["count"] * tl["every"]
    print(f"clock: machine - here {before * 1000:.1f} ms before (rtt {c['rtt_before_s'] * 1000:.1f}), "
          f"{after * 1000:.1f} ms after (rtt {c['rtt_after_s'] * 1000:.1f}); "
          f"the sound took {played:.3f} s to play, {length:.3f} s long")
    offset = before
    if abs(after - before) > 0.05:
        stepped = played - length
        if abs(stepped - (after - before)) < abs(stepped):
            print(f"clock: stepped by {(after - before) * 1000:.0f} ms while the sound played; "
                  f"the start is placed with the offset from before")
        else:
            print(f"clock: stepped by {(after - before) * 1000:.0f} ms outside the sound; "
                  f"placed with the offset from before, which may be wrong by the step")
    else:
        offset = (before + after) / 2
    start = c["start"] - offset  # on this machine's clock
    tl["sends"] = [tl["mono0"] + (start + tl["lead"] + k * tl["every"] - tl["wall0"]) for k in range(tl["count"])]
    with open(path, "w") as fh:
        json.dump(tl, fh)


def main(argv):
    if not argv or argv[0].startswith("-"):
        print(__doc__)
        return 2
    out = argv[0]
    opts = {"--seconds": "60", "--every": "2", "--lead": "5", "--check": None, "--marks": None,
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
        run(out, float(opts["--seconds"]), float(opts["--every"]), float(opts["--lead"]), opts["--check"],
            opts["--sink"])
    place(out)
    bursts.analyse_run(out, opts["--marks"])
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
