# Test desktop, with a relative pointer

The third rig, and the only one where **the far end owns the cursor**. QEMU's
built-in VNC server is the one free implementation of a relative pointer in RFB
— pseudo-encoding −257, `PointerTypeChange` — and this rig is how to see what
that does to a control stack built entirely on owning the cursor here. TigerVNC has no such thing, so `testvnc` cannot be asked this question.

```sh
./run.sh                 # build if needed, run, print the address for the phone
./run.sh logs            # QEMU's own output
./run.sh stop
POINTER=absolute ./run.sh   # the same guest with a usb-tablet, for the A/B
```

Defaults: port `5902`, SPICE on `5930` and its TLS port on `5931`, 256 MB,
`VncAuth` with password `vncpass` and a SPICE ticket of `spicepass`, KVM if this
machine has it. There is no `shot` command: the guest's screen is not an X display anybody can reach
from the host — use the client under test.

```
$ cargo run -q --example rfb-probe -- 127.0.0.1:5902 vncpass /tmp/f.ppm
connected: 1024x768
pointer mode: relative
…
pointer:      relative
buttons:      8
```

## And SPICE, on the same QEMU

The one far end here that speaks **two protocols to the same guest**, which is
what makes a comparison between them a comparison rather than an analogy. It is
one package (`qemu-system-modules-spice`, which Debian splits out of
`qemu-system-x86` — without it `-spice` is not an option group at all) and two
more ports; `scripts/measure/spice-probe` drives the crates and
`backends/modules/spice`'s own example drives the backend.

**TLS is a second port rather than a negotiation**, so it is one here too: 5931,
with a certificate and a throwaway CA made when the image is *built*. Built
rather than started, so the fingerprint a phone pins survives a restart of the
container — and a rebuild is how to make the identity change on purpose, which
is the case the pin prompt exists for. `./run.sh` prints the fingerprint, and so
does the container's log.

```sh
SPICE_WAN=1 ./run.sh     # jpeg and zlib-glz, which a client cannot ask for
```

That switch turns on the two encodings a server only reaches for when it thinks
the link is a WAN. It is here because they are otherwise unreachable — though
this guest does not trip QEMU's own heuristic for either, since a dithered
desktop is not a photograph.

`POINTER` decides SPICE's mouse mode as well as RFB's, and the same way: with
no `usb-tablet` the server offers *server* mode alone, and with one it offers
*server+client* and lets the client ask. That is the pairing a relative pointer
needs and the one no reachable RDP server has
([68](../../notes/68-relative-pointer-usable.md) §7).

## The guest

**KolibriOS**, from its own 1.4 MB floppy image: a graphical desktop at
1024×768 in about three seconds, with windows to drag, a text editor, a
calculator and a dozen small games — enough to judge a pointer by. It is
downloaded at build time and pinned by SHA-256; upstream publishes nightly
builds and no dated URL, so the pin will fail one day, which is the intended
behaviour and the Dockerfile says what to do about it.

The important property is not that it is small: **the pointer is drawn by the
guest, into the framebuffer.** A PS/2 mouse has no hardware cursor for QEMU to
forward, so there is no `Cursor` rectangle and no hotspot — the client is not
told the shape of the pointer or where it is, which is exactly the world a
relative client lives in.

Tiny Core was tried first and does not work here: its Xvesa reaches the VESA
BIOS through `/dev/mem`, which a current kernel refuses (`Interrupt pointer …
doesn't point at ROM`, then `no screens found`), so X never starts and the
console has no pointer at all. The container booted, the pointer mode
negotiated, and there was nothing to look at.

## What this rig can be asked

- **Which end owns the cursor**, and that our client changes its behaviour when
  told: `pointer mode: relative` arrives unprompted, right after `SetEncodings`.
- **Whether the deltas mean what we think.** `RFB_MOVE=dx,dy,count` walks the
  pointer, and two frames either side of the walk locate the cursor to the
  pixel — the guest draws it, so it is in the picture.
- **Whose acceleration.** The answer is visible in the walk: 40 × (10, 5) moves
  this guest's pointer by (187, 64), not (400, 200), because the transfer curve
  at the far end is now the one that counts.
- **The A/B against the same guest.** `POINTER=absolute` plugs in a USB tablet,
  QEMU announces absolute, and the client goes back to owning the cursor with
  nothing else about the session changing.

`RFB_TYPE='ls /usr/local/bin\n'` types into the guest through the same client,
which is the only way into a virtual machine that has no shell of its own on
this side. Note that **QEMU does not synthesise Shift**: it maps a keysym to a
scancode, so `*` sent with nothing held arrives as `8` — the probe holds Shift
itself for the characters a US keyboard puts behind it, which is the same
thing the RDP client has to do for a layout that needs Shift.

## And sound: the audio guest

QEMU's VNC server is also the one free implementation of **QEMU's audio
extension**, which TigerVNC's client asks for when its `Audio` option is on.
KolibriOS cannot be told from here to play anything, so the sound has a
guest of its own, opt-in and on its own image:

```sh
AUDIO=1 NAME=myqemu PORT=5962 SPICE_PORT=5964 SPICE_TLS_PORT=5965 TONE_PORT=5956 ./run.sh
printf t | nc -q1 127.0.0.1 5956     # one 440 Hz burst (the reply: guest µs, frames)
printf s | nc -q1 127.0.0.1 5956     # the guest's stream stopped for 5 s (AudioEnd, AudioBegin)
NAME=myqemu ./run.sh stop
```

`AUDIO=1` builds `protoqemu-audio` from `Dockerfile.audio` on top of the
`protoqemu` image as it is — so a private instance serves the same SPICE
certificate, and building it never touches the image the shared container
runs — and boots Debian's kernel with a busybox initramfs instead of
KolibriOS. Its one program, `audio/tone.c`, keeps the guest's HDA card
playing silence and plays half a second of 440 Hz at half scale whenever a
`t` arrives on the serial port, which is the container's port 4555
(published as `TONE_PORT`, default 5955), and answers how long the guest
itself took: microseconds from the byte to the first tone frame, and the
frames already queued in the card ahead of it. The stream never stops
between bursts, as with a guest playing music: one AudioBegin, then data.

On the host side the sound goes nowhere: `-audiodev none` plays nothing but
still runs QEMU's mixer, which is what the VNC server's capture taps, so the
guest is heard only by a VNC client that asks — 48 kHz stereo S16 at every
step, TigerVNC's format, so nothing is resampled, and the codec's mixer is
off so that no guest volume changes the level. The screen is the guest's
text console (720×400). `NAME=` and the ports are for running it beside the
shared `protoqemu` rather than in its place; the image is `protoqemu`'s
whatever the container is called (`IMAGE=` to change that).
