# scripts/testrustdesk

A RustDesk far end in a container: their released 1.4.9 `.deb`, pinned by
SHA-256, over Xvfb and Openbox, answering **direct IP access on 21118** and
registered with the public rendezvous network so the id path works too.

The desktop is [`../testvnc`](../testvnc)'s — the same wallpaper, clock,
terminal and document — so a picture from this rig and a picture from that one
are of the same thing.

```sh
scripts/testrustdesk/run.sh              # build if needed, run, print address and id
scripts/testrustdesk/run.sh id           # the peer id, which is generated on first start
scripts/testrustdesk/run.sh shot         # what the far end looks like, without a viewer
scripts/testrustdesk/run.sh logs
scripts/testrustdesk/run.sh stop
```

**The host's GPU is forwarded where there is one** (`/dev/dri`), and the image
carries `va-driver-all` and `intel-media-va-driver`. Their server asks VA-API
what it can encode: with the device it finds `h264_vaapi` and this becomes the
only far end here that sends **H.264 over RustDesk**, which is what the codec
row is checked against; without it, it settles for VP9, VP8 and AV1 in software.
The hardware is the host's, so a measurement says which machine it was taken on.

`PASSWORD` sets the permanent password (`rustdeskpass`), `GEOMETRY` the screen,
`PORT` where 21118 lands on the host. `RENDEZVOUS=host:port` and `KEY=…` point
it at a self-hosted `hbbs` instead of the public network — which is
[`../testrustdeskserver`](../testrustdeskserver), and it prints the pair of them
to paste; `RENDEZVOUS=none` registers with nothing at all, leaving direct IP as
the only way in.

**The peer inside this container is never reachable by the address a rendezvous
server hands out for it**, since that address is on Docker's bridge. That is not
a fault to work around: it is the only far end here that forces the relay, and
it is how both relay paths were driven.

Two things the container has to do that a person does not, both in `start.sh`:

- **The password is written into the config rather than set with their CLI.**
  `rustdesk --password` talks over an ipc socket to a service and refuses
  unless the binary believes it is installed. A plain value in `RustDesk.toml`
  is accepted and is rewritten in their obfuscated form on first start.
- **The background is set after the window manager, not before it.** Xvfb has no
  persistent root pixmap, so the other rigs' order leaves a black screen.

What this rig answered is [45a](../../notes/77-rustdesk-checks.md)'s check 2:
**their X11 capture works under Xvfb**, unprompted, first try — which is the one
[49](../../notes/49-h264.md) lost a day to with a headless compositor.

## And sound

`AUDIO=1` is the same desktop with a sound to send, on an image of its own
(`protorustdesk-audio`, from `Dockerfile.audio`, built on top of
`protorustdesk` as it is):

```sh
AUDIO=1 NAME=myrd PORT=21128 TONE_PORT=5957 RENDEZVOUS=none ./run.sh
printf t | ncat --send-only 127.0.0.1 5957   # one 440 Hz burst
NAME=myrd ./run.sh stop
```

It adds a PulseAudio server for root whose default sink is a null sink,
which their server records from (that sink's monitor, as on any Linux
desktop), and `scripts/testqemu/audio/tone.c` playing into it through
ALSA's PulseAudio plugin: silence, and half a second of 440 Hz at half
scale per `t` on `TONE_PORT`, answered with its own share of the delay, so
that `scripts/testaudio/bursts.py --port 5957` times it as it times the QEMU
guest. Three things it has to do that a desktop does not, all in
`start-audio.sh`:

- **Their connection manager runs from the start, without its window**
  (`rustdesk --cm-no-ui`, started again whenever it exits, which it does
  when a session ends). The manager is what records the sound and hands it
  to the server; on a desktop the server starts it, as a Flutter window,
  when a client connects, and here that window cannot start (no libEGL) —
  the server then logs `Error of audio service` once a second and sends
  nothing.
- **The silence is a square wave of one LSB, not zeros.** Their server stops
  sending eight seconds into a silence and starts again with the next
  sound, so a client's stream would not exist — and could not be checked to
  be on the null sink — before the first burst.
- **`enable-audio` is `Y`** in their config (`start.sh` writes `N`
  otherwise).
