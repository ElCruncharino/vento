# Test sound

vento's sound, played somewhere nobody hears it and recorded back: a PipeWire
null sink, its monitor, and an analysis of what came out. For the desktop's
audio output (`common::audio` over a PipeWire stream of its own,
`backends/platform/desktop/src/pipewire.rs`, or ALSA, `alsa.rs`, without PipeWire)
and for every backend moved onto it.

**Nothing here may be heard.** Sound goes to the null sink `vento_test` and
nowhere else; the default sink is never changed; and nothing louder than
silence is played until `pactl` has shown the stream on the null sink.

```sh
./rig.sh [OUT_DIR] [--second]  # the output's own rig, end to end (below)
./sink.sh load              # load the null sink, print its module id
./sink.sh check PID [N]     # are PID's streams (N of them) on it? (exit 0 only if so)
./sink.sh unload ID         # unload what load printed
./sink.sh off               # unload any vento_test left behind
./record.sh out.wav [secs]  # record the sink's monitor
./analyse.py summary out.wav           # pitch, level, gaps per half second
./analyse.py rig OUT_DIR               # what rig.sh measures
./bursts.py OUT_DIR --port 5956 [--seconds 60] [--marks FILE]
                                       # a QEMU guest's tone bursts, end to end
./winbursts.py OUT_DIR [--seconds 60] [--check PID] [--marks FILE]
                                       # a Windows RDP session's, the same way
```

`rig.sh` needs NASM on `PATH` the first time it builds the desktop crate
(its build script builds FFmpeg). Python is the standard library only.

## Routing a stream to the sink

vento plays on a PipeWire stream of its own, and libpipewire reads
`PIPEWIRE_NODE` from the environment: `PIPEWIRE_NODE=vento_test vento ...`
plays into the null sink. So does ALSA's `default` device here, PipeWire's
ALSA plugin, which is where vento's sound goes with `--audio-output=alsa`
(or `VENTO_AUDIO_OUTPUT=alsa`; the rig and every other program built on the
desktop crate read that) and where there is no PipeWire daemon. `sink.sh
check` finds vento's own streams by their process id (node `vento`) and the
plugin's, which carry no process id, by its node name `alsa_playback.<program
name>`; it says which output each came through (`PipeWire` or `ALSA`), and
fails unless every one is on the sink. The safe order, which `rig.sh`
follows:

1. `sink.sh load`, keeping the id.
2. Start the program with `PIPEWIRE_NODE=vento_test`, playing silence (or
   not yet connected to anything that sends sound).
3. `sink.sh check $pid`; stop here if it fails.
4. Record (`record.sh`), then let it play.
5. `sink.sh unload $id`, whatever happened (a `trap ... EXIT`).

## The output's rig

`backends/platform/desktop/examples/audio_rig.rs` opens a 48 kHz stereo
stream on the desktop's output (PipeWire's, or ALSA's with
`VENTO_AUDIO_OUTPUT=alsa`) and, once `rig.sh` says `go`, records the
monitor with `parec` and plays a script, timing everything on its own clock:

| seconds   | what                                                        |
|-----------|-------------------------------------------------------------|
| 0-1       | silence while the recording settles                         |
| 1-11      | 440 Hz at half scale; from another thread: session gain 0.5 at 4, 1 at 7, mute at 8, unmute at 9 |
| 11-13     | silence with a 5 ms 3 kHz click every half second           |
| 13-15     | tone                                                        |
| 15        | a burst: 2 s of 660 Hz written at once                      |
| 15-17     | clicks                                                      |
| 17-18     | tone                                                        |
| 18-19     | stall: nothing written                                      |
| 19-21     | tone                                                        |
| 21-22.5   | clicks                                                      |

`analyse.py rig` puts the recording on the rig's clock (each piece `parec`
hands over arrives after its last frame was captured, so the earliest arrival
less its length bounds frame 0; the numbers are late by that bound's slack)
and reports, per stretch of tone clear of every change: level against the
source (1.0, 0.5), pitch, discontinuities (second differences no 440 Hz sine
makes) and 5 ms gaps; the click latencies, write to capture, before the burst,
after it and after the stall; how soon a mute, an unmute, a gain change and the
resume after the stall are heard; how much of the burst was heard, and the
stream's own latency (queue plus device) over the run. `results.json` beside
the recording has all of it.

What to expect, from the policy in `common::audio`: the queue and the
device's buffer together keep about 100 ms and never more than 250 ms, so
the clicks are heard about 100 ms after they are written, the burst is heard
for about the queue's share (the rest is dropped, `dropped` in the stats) and
the clicks after it are no more than a period later than before; after the
stall the tone comes back 100 ms after it is written; a mute is heard after
the device's buffer only, not after what is queued. On ALSA (50 ms buffer;
measured: clicks 102 ms before the burst and after the stall, 114.5 ms after
the burst, 50 ms of the burst heard, the resume 100 ms, a mute 55-65 ms). On
PipeWire the device holds a graph cycle (5-11 ms): clicks 80-87 ms before the
burst, 101-117 ms after it, 86-88 ms after the stall, 90-100 ms of the burst
heard, a mute in 5-10 ms. `--second` opens a second session's output beside
the first (silent): two streams, `sink.sh check PID 2`, and on PipeWire two
nodes of the one client.

## A backend end to end: bursts.py

For a backend whose far end can be made to play on cue — the QEMU audio
guest (`scripts/testqemu`, `AUDIO=1` for TigerVNC, `AUDIO=spice` for SPICE)
and the RustDesk desktop's sound (`scripts/testrustdesk`, `AUDIO=1`, whose
tone answers on `TONE_PORT` 5957 the same way) — `bursts.py` measures the
whole path.
With the client already playing into the sink (steps 1-3 above; the guest
plays silence until asked), it sends the guest's serial port a `t` every
`--every` seconds (2), each a half-second burst of 440 Hz at half scale,
records the monitor, and finds each burst in the recording. Per burst: the
latency from the guest's card playing its first tone frame to the sink
capturing it (the guest's own share, which it answers on the serial port,
taken out; late by the capture path's least delay, as with the rig), how
much was heard, pitch, level against the source (1.0 is as sent), 5 ms gaps
and discontinuities; then the latency over the first and the last minute,
which is what a lag that grows shows up in. `--marks FILE` prints lines of
`<time.time()> <words>` (e.g. a script minimising the window) on the same
clock, which is how a pause and a restore are lined up with the bursts.
`--analyse` reads a previous run's directory again without recording.

```sh
id=$(./sink.sh load)
PIPEWIRE_NODE=vento_test vento tigervnc 127.0.0.1::5962 --password vncpass \
    -o Audio=true --probe 330 & pid=$!
./sink.sh check $pid && ./bursts.py out --port 5956 --seconds 300
kill $pid; ./sink.sh unload $id
```

## An RDP session end to end: winbursts.py

The same measurement against a real Windows machine (the one in
`scripts/testwin`, 10.33.0.208), whose sound reaches an RDP client only from
the RDP session itself. An ssh command runs in session 0, whose sound goes
nowhere; so `winbursts.py` writes `scripts/testwin/tone.ps1` over there and
runs it as a scheduled task with an Interactive principal, which Windows runs
in the session the client is connected to, and removes the task and the file
again before it returns (also when it fails). The tone is one WAV through
`System.Media.SoundPlayer`: `--lead` seconds of silence (5), then half a
second of 440 Hz at half scale every `--every` seconds.

Latency is absolute, by the clocks: the task stamps the moment just before it
hands the sound to the player with `GetSystemTimePreciseAsFileTime`, and the
offset between the machine's clock and this one is measured over the LAN
before and after (a loopback listener there reached through `ssh -L`; the
shortest of 40 round trips, about 1.5 ms, bounds the error to under 1 ms).
Windows' time service steps its clock now and then — by 1.8 s once, in the
middle of a run — and the task's start and end stamps say whether the step
fell inside the sound (`place` in the script explains). So each burst's
latency is Windows' player and audio engine, the RDP sound channel, the
client and the capture; Windows' share is the same in every burst of a run,
and what varies over a run is the client's. `--check PID` stops the task
during the silence unless PID's stream is on the null sink: an RDP client
opens its stream only when the server starts sending, so the silence is what
the check sees.

```sh
id=$(./sink.sh load)
PIPEWIRE_NODE=vento_test vento freerdp 10.33.0.208 --user user --password password \
    --trust --state-dir <scratch> -o Sound=local --probe 400 & pid=$!
sleep 10   # connected and logged on
WIN_SSH='ssh user@10.33.0.208' ./winbursts.py out --seconds 300 --check $pid
kill $pid; ./sink.sh unload $id
```

`WIN_SSH` is any command that runs a command there and takes ssh's `-L`
(a wrapper around `sshpass` with a scratch known-hosts file, say). What
Windows 10 22H2 offers and picks is in the client's log (`sound: the server
offers ...`, `sound: the server sends ...`): with AAC among what the client
takes, it sends AAC at 44.1 kHz stereo.
