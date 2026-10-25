# Test sound

vento's sound, played somewhere nobody hears it and recorded back: a PipeWire
null sink, its monitor, and an analysis of what came out. For the desktop's
audio output (`runtime::media::audio` over a PipeWire stream of its own,
`backends/platform/linux/src/pipewire.rs`, or ALSA, `alsa.rs`, without PipeWire)
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
```

`rig.sh` needs NASM on `PATH` the first time it builds the platform crate
(its build script builds FFmpeg). Python is the standard library only.

## Routing a stream to the sink

vento plays on a PipeWire stream of its own, and libpipewire reads
`PIPEWIRE_NODE` from the environment: a program started with
`PIPEWIRE_NODE=vento_test` plays into the null sink. So does ALSA's `default`
device here, PipeWire's ALSA plugin, which is where vento's sound goes with
`VENTO_AUDIO_OUTPUT=alsa` (which the rig and every other program built on the
platform crate read) and where there is no PipeWire daemon. `sink.sh check`
finds vento's own streams by their process id (node `vento`) and the plugin's,
which carry no process id, by its node name `alsa_playback.<program name>`; it
says which output each came through (`PipeWire` or `ALSA`), and fails unless
every one is on the sink. The safe order, which `rig.sh` follows:

1. `sink.sh load`, keeping the id.
2. Start the program with `PIPEWIRE_NODE=vento_test`, playing silence (or
   not yet connected to anything that sends sound).
3. `sink.sh check $pid`; stop here if it fails.
4. Record (`record.sh`), then let it play.
5. `sink.sh unload $id`, whatever happened (a `trap ... EXIT`).

## The output's rig

`backends/platform/linux/probe/audio_rig/` (built by `rig.sh` with
the platform crate's `probe` feature, which it needs) opens a 48 kHz stereo
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

What to expect, from the policy in `runtime::media::audio`: the queue and the
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
