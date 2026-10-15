<!--
SPDX-FileCopyrightText: 2026 Patrick Gaskin
SPDX-License-Identifier: GPL-3.0-or-later
-->

# linux

A native Linux client for the backends under `backends/`. One process that can
hold a session on any of the eight protocols — the four Rust ones directly, the
four C/C++ ones through the shared libraries this crate's `build.rs` has CMake
build — written against the same seam the Android app's `Backend` is, minus the
phone. It is an egui window over that seam, and a headless probe for proving a
backend without one.

## What is here

- **`src/backend.rs`** — the `Backend` trait (`Backend.java` without the
  Android types) and the value types around it (`Facts`, `ConnectionFact`,
  `Monitor`, `Traffic`, `Picture`).
- **`src/events.rs`** — the `Event` enum (`common::Events` plus the C API's
  `question`, `security_needed`, `message`) and the `Mailbox` a session pushes
  them onto, with a wake hook the window hands its repaint request to. Damage
  is coalesced into a dirty-rect list behind a mutex and delivered one
  `Event::Frame` per update, never a message per rectangle; a window that
  falls behind finds one unread `Frame` per desktop size and the latest
  cursor, pointer mode and clipboard, not a backlog.
- **`src/options.rs`** — the per-backend option descriptions, ported from each
  `*Provider.java`, so `-o key=value` and the dialog's rows are generic.
- **`src/backends/`** — the eight adapters. `rfb`, `ironrdp`, `rustdesk` and
  `spice` are the twins of each Rust crate's `bindings/mod.rs`; `c.rs` plus
  `c/{libvnc,tigervnc,freerdp,realvnc}.rs` are the four C backends over the
  vtable in `vento/backend.h`.
- **`src/gui/`** — the window: `mod.rs` (the winit loop, the wgpu surface, egui
  drawn into it), `session.rs` (a live session: the desktop, the input, the
  toolbar, the prompts, the cursor), `desktop.rs` (the desktop as one
  texture, or as tiles where it is bigger than the device's largest, updated
  a damaged rectangle at a time), `view.rs` (scale modes, panning and the
  pointer's mapping, pure arithmetic with tests), `toolbar.rs` (where the
  toolbar sits and which strip of the edge brings it out, with tests),
  `icons.rs` (the toolbar's font and the symbols it draws, tested against
  the font and its codepoints file), `follow.rs` (the desktop
  following the window's size: the phone's `WindowFollower`, pure logic with
  tests), `keys.rs` (winit keys to X11 keysyms), `platform.rs` (which display
  server the window is on, and the per-server shortcuts and capture choice),
  `wayland.rs` (the Wayland keyboard shortcuts inhibitor), `x11.rs` (the X11
  keyboard grab that stands in for it), `cursor.rs` (the far end's cursor as
  the window's own, with tests), `hud.rs` (the debug overlay's box and rates,
  with tests), `settings.rs` (the client's own settings and their rows),
  `dialog.rs` (the connection dialog), `panel.rs` (the connection-info panel,
  with the live controls), `prompts.rs` (the modal prompts).
- **`src/known_hosts.rs`** — the pin store (`KnownHosts.java`) and RealVNC's
  identities, as files under `$XDG_CONFIG_HOME/vento`, with tests.
- **`src/main.rs`** — the `vento` binary: the window, `--list`, and the
  headless probe.

## Building and running

This crate is a Cargo workspace of its own, not a member of the repository's
(the root `Cargo.toml` says why: a desktop GUI stack's optional features must
not unify into the phone's dependency graph and its licence page). Build it
from this directory, or with `--manifest-path linux/Cargo.toml` from the root;
its binary lands in `linux/target/`.

Host packages (the same the C host build needs, plus a libclang for bindgen):
cmake, ninja, pkg-config, a C/C++ compiler — **clang** for the RealVNC backend
(libc-shim does not build under GCC; without clang that one backend is left
out, with a build note) — and the development files for openssl, gnutls, nettle
(hogweed, gmp), libjpeg-turbo, pixman, zlib, FFmpeg (libavcodec, libavutil,
libswscale), libpulse, alsa-lib, plus libclang. The window needs a Vulkan (or
GLES) driver wgpu can use and a Wayland or X11 session.

The submodules must be initialised (`git submodule update --init --recursive`;
RealVNC's four host-only submodules need `--recursive`) and patched
(`./patch.sh -n` at the root). One of them is this crate's alone: winit, at
the tag of the release `Cargo.lock` names, which `Cargo.toml`'s
`[patch.crates-io]` takes in place of the crates.io one so that our fixes to
it live in `patches/winit/` like every other submodule's. Unpatched, it builds
and is exactly crates.io's winit, with the defects the patches fix.

Licences: the client is GPL-3.0-or-later like the rest of the repository, and
what it takes from crates.io keeps each crate's own licence, as `Cargo.lock`
lists them. One is not code and is built into the binary as it is: the
toolbar's icons are Google's Material Symbols Outlined font, version 2.975
(Apache License 2.0), in `assets/material-symbols/` with its licence and its
codepoints file. It comes from google/material-design-icons at commit
49d4db35df87 (2026-10-09), which ships Material Symbols only as a variable
font; egui sets none of a variable font's axes, so what is here is a static
instance of it made with fontTools (unfilled, weight 400, grade 0, optical
size 24; `src/gui/icons.rs` has the command), 1.4 MB of the binary. Nothing
of this crate's graph reaches the Android app or its licence page.

```sh
cd linux

# The connection dialog: pick a backend, fill in the address, user, password,
# domain and that backend's options, and connect. Nothing is saved.
cargo run

# The same dialog, filled in from the command line — a backend on its own, or
# with an address, a user, a password and options. Nothing connects until
# "Connect" is pressed: a machine on the command line is not a machine to
# connect to, which a typo would otherwise be.
cargo run -- tigervnc 127.0.0.1:5901 --password vncpass
cargo run -- rdp

# The client's own settings, which are every connection's rather than this
# one's: the clipboard in each direction (both on unless told otherwise), the
# keyboard grab that sends Alt+Tab and the rest on (on unless told), and the
# far end's cursor as the window's own (off unless told). The dialog's
# "Settings" section has the same four.
cargo run -- tigervnc 127.0.0.1:5901 --no-clipboard-out --no-clipboard-in
cargo run -- tigervnc 127.0.0.1:5901 --no-grab-keyboard
cargo run -- tigervnc 127.0.0.1:5901 --native-cursor

# Straight into a session, with --connect. Without --password the credentials
# prompt asks.
cargo run -- tigervnc 127.0.0.1:5901 --password vncpass --connect
cargo run -- rdp 127.0.0.1:3389 --user proto --password protopass --connect
cargo run -- rustdesk 127.0.0.1 -o ConnectBy=address --password rustdeskpass --connect
cargo run -- spice 127.0.0.1:5930 --password spicepass --connect

# On a given display server rather than winit's choice (Wayland where
# WAYLAND_DISPLAY is set, X11 otherwise).
cargo run -- --x11 tigervnc 127.0.0.1:5901 --password vncpass --connect
cargo run -- --wayland

# The backends and their options.
cargo run -- --list

# A session, headless: connect, print every event, answer the prompts from the
# flags, write the framebuffer and a thumbnail, print the facts, and leave.
# Any of the probe's flags (--probe, --out, --click, --key, --clipboard,
# --trust, …) selects this instead of the window. A certificate the known
# hosts match is accepted without --trust; --trust accepts any other for this
# run and pins nothing.
cargo run -- tigervnc 127.0.0.1:5901 --password vncpass \
    --out /tmp/shot.ppm --click 500,300 --key 0x61 --clipboard hello
cargo run -- rdp 127.0.0.1:3389 --user proto --password protopass \
    --trust --out /tmp/rdp.ppm
```

`build.rs` runs `backends/CMakeLists.txt` with `-DVENTO_SHARED=ON`, which makes
each backend a `libvento_<name>.so` under the build tree carrying its vendored
static dependencies and the stderr logger; the binary links those four by name
with an rpath into the build directory, so `cargo run` works from the target
directory without anything on `LD_LIBRARY_PATH`. The existing static path and
the C probes are unaffected (`cmake -S backends -B … ` without the option).

The probe answers the first credentials prompt with `--password` and a second
with a cancel, since a second prompt is the far end refusing the first answer
(SPICE and RustDesk ask again for every refused password); it then says the
authentication failed and leaves with 1. The clipboard settings hold in the
probe too: under `--no-clipboard-in` what the far end copies is printed as its
size alone, and `--clipboard` with `--no-clipboard-out` is refused as a
contradiction (status 2).

The probe's exit status is 0 only if it connected within `--connect-timeout`,
stayed connected for the time asked, wrote the framebuffer and the thumbnail
where `--out` asked for them, and closed within 10 s of disconnecting; it is 1
otherwise, and 2 for a command line it refused. `--click`, `--key` and
`--clipboard` are sent, not confirmed: no protocol here says input arrived, so
0 says they went out on a live session and nothing more. What can be known in
advance is refused (status 2): any of the three under `-o ViewOnly=true`,
where every backend drops input. And `--clipboard` fails the run (1) where the
backend can tell the text went nowhere, which today is SPICE without a guest
agent that takes a clipboard.

`--verbose` lets the backends' debug lines through (and not the GUI stack's,
which would be tens of thousands a minute).

## The window

One window, which is either the connection dialog or a session. A session
that ends goes back to the dialog, filled in with what it was connected to and,
unless the disconnect was asked for, told why it ended — and to a window, if
the session had gone fullscreen. The window's title is the desktop's name once
the protocol has said it, and rings the bell by putting one in the title for a
moment — unless the backend's Bell option (`AcceptBell`, on the VNC backends)
is off, in which case a bell is dropped before the window or the probe sees
it.

Disconnecting, or closing the window, ends a session at once, including one
that is still connecting: every backend's dial and handshake give way to it,
where several used to sit out their 20-second connect timeout first. A session
that has still not closed ten seconds after a disconnect is given up on, and
a backend is always released off the window's thread, so neither the dialog
coming back nor the window closing waits for one that will not let go; once
the window has gone the process gives it two seconds, then exits anyway.

A session is told when its window cannot be seen — minimised, or on X11
wholly covered — and the backends pause their updates then, as the phone's do
when its session screen stops: the three VNC backends stop reading, FreeRDP
and IronRDP send a Suppress Output PDU, SPICE stops acknowledging, and
RustDesk drops to a frame a second. They resume when it is shown again. A
window that is on screen without the keyboard focus stays live. X11 says both
through winit: `_NET_WM_STATE_HIDDEN`, checked every frame and on a focus
change, and the visibility events, which under a compositing manager never
say covered. Wayland says neither — xdg-toplevel has no minimised state, and
winit 0.30 does not pass on its `suspended` one — so there a session never
pauses; a hidden window's frames stop with its frame callbacks while the
backend keeps receiving. Checked on `scripts/testx11` for tigervnc, libvnc,
rfb and freerdp: live with another window focused, paused minimised or
covered, live again restored.

**The toolbar** sits flush with the top edge, square-cornered, and stays out of
the way: it slides off the top of the window when the pointer is elsewhere and
slides back when the pointer reaches the top edge within the toolbar's own
span — the rest of the edge is the desktop's, for whatever the far end has at
the top of its screen — and for the first three seconds of a session, so that
it is seen once. In a window that strip reaches 8 points down, since a pointer
coming off the title bar seldom stops on the window's very first row; in
fullscreen, where the screen's edge stops the pointer, it is the top row of
pixels. It can be dragged along the edge, by any part of it, buttons
included — a drag never presses the button it started on — and stops at the
window's sides; where it is left is kept as a fraction of the room either side
of it, so that a resize keeps it centred, or against the edge it was put
against, and the next session's toolbar starts there, for as long as the
process runs (it is not saved). The strip that brings it out follows it.
It has no shadow, and its buttons are flat: an icon (Material Symbols
Outlined, unfilled) and no frame at rest, egui's frame while the pointer is
on one or pressing it. A button with a state shows it in its icon, and its
tooltip names the button, its state and, for Scale, what a click goes to
("Scale: fit width — click for fit height"). View only, Info and Debug, while
on, keep a frame at rest, a dim fill of the selection's blue with the icon in
the selection's colour. There are no words on it: which monitor or display
is showing is in the tooltip too. A grip at the left end shows where to take
hold. On it:

- **Scale** cycles fit width → fit height → fill width → fill height → 1:1,
  its icon `fit_page_width`, `fit_page_height`, `width` (↔), `height` (↕)
  and `view_real_size` (1:1) in turn.
  *Fit* never enlarges the desktop past a device pixel per desktop pixel;
  *fill* does; 1:1 is a desktop pixel per device pixel. A desktop that is
  smaller than the window in an axis is centred in it; one that is bigger is
  panned — move the pointer within 48 points of a window edge, over the
  desktop, and it moves that way, faster the nearer the edge.
- **Monitor** (when the far end reports more than one) shows the whole desktop
  (`tv_displays`, several screens) or one monitor's rectangle (`monitor`, one
  screen), with the scale modes applied to that. Where a
  backend offers *displays* instead (RustDesk sends one screen at a time) the
  button (`monitor`) asks the far end for the next one.
- **View only** stops the mouse, the keyboard and the clipboard going to the
  far end, and tells the backend so: an eye while on, a mouse while off.
- **Info** opens the connection panel, below.
- **Debug** toggles the debug overlay, below; so does **Ctrl+Alt+D**.
- **Fullscreen** fills the screen (winit's borderless fullscreen on the
  current monitor); the toolbar still slides in at the top edge, and the same
  button, or disconnecting, leaves it.
- **Disconnect**.

**The connection panel** is the phone's `ConnectionPanel`: controls above
facts. The controls are the backend's *live* options from the registry — the
ones `set_option` acts on mid-session, such as the VNC encodings' quality and
compression, SPICE's image compression, RustDesk's codec and frame rate — as
a control per kind (a dropdown for a choice, a box for a switch, a field for a
number or text), showing what the session connected with and then whatever has
been set since, with the dialog's rules for a gated option and a choice that
requires a switch. Such a choice is offered only while its switch is on, and
turning the switch off puts the option back to its default, in the dialog and
the panel alike — unticking TigerVNC's "Offer H.264 encoding" takes Encoding
from H.264 back to Tight — rather than leaving it selected and sent; the
command line refuses one given without its switch (`-o Encoding=h264` without
`-o H264=true`). Then the two that are not options, because whether they
can be offered is a fact about the live connection: **Desktop size**, where the
far end takes one (`can_resize`), offering this window's size in desktop
pixels first, the size the desktop has, the far end's own screens and the
usual sizes, and asking `request_desktop_size` for the one picked — what
arrives is what the far end granted, and the dropdown shows that rather than
what was asked; and **Display**, where the far end sends one of several
screens at a time (RustDesk), asking `request_display`. And **Follow window
size**, below, and the four **settings**, below, which a running session
takes a change to at once.

Under them, the desktop's name, the facts the backend reports in its own words
(the diagnostic ones folded away), the bytes transferred, the desktop size and
its monitors. Everything is refreshed once a second while the panel is open,
without disturbing a control being used: an open dropdown stays open, and a
field being typed in keeps its text until it is left, which is when its value
is sent.

**Follow window size** (off by default, per session) asks the far end for a
desktop the size of this window in pixels whenever the window changes shape —
the phone's `WindowFollower`, in `follow.rs`: asked on a tick (the panel's
one-second poll, brought forward to 600 ms after a resize) once the window has
settled for two ticks, never twice for the same size, and only once the far
end says it takes sizes, which a VNC server says some time after connecting.
Turning it on asks at once. With fullscreen it makes the desktop the screen's
size. Granted against the test rigs by TigerVNC's Xvnc (through all three VNC
backends), xrdp (FreeRDP; and IronRDP with `-o Compression=false`, since its
re-activation after the resize does not yet take the compressed FontMap PDU
xrdp sends under RDP 6.1 compression, and the session ends) and QEMU with the
SPICE agent.

**Settings** are the client's own, one answer for every connection, as the
phone's `AppSettings` has them: **Send clipboard** and **Receive clipboard**
(both on by default; `--no-clipboard-out`, `--no-clipboard-in`), **Grab
keyboard** (on by default; `--no-grab-keyboard`), and **Native cursor** (off
by default; `--native-cursor`), below. Grab keyboard is whether the window
holds the window manager's or the compositor's shortcuts off at all — the X11
keyboard grab, or Wayland's shortcuts inhibitor, below — so that Alt+Tab,
Super and the rest reach the far end; off, neither is ever taken, and a
running session lets go of whichever it holds the moment it is unticked. They are a section of
the connection dialog, apart from the backend's options because they are not
the connection's, and rows of the connection panel, where a session shows
what it is running with and takes a change at once. Nothing is saved: a
session hands them back to the dialog when it ends, and the process forgets
them when it does.

**Keys** go to the far end as X11 keysyms, from winit's *logical* key — what
the layout says the key means — as Latin-1 keysyms where there are such, the
Unicode keysym (`0x01000000 | codepoint`) otherwise, and a table for the named
keys; the number pad sends `KP_*` with Num Lock on or off (`KP_7`, or
`KP_Home`; winit names the latter Home and says where it was), and modifiers
are told apart by side. A level winit has no name for is sent as the key's
own unmodified level, so Alt pressed after Shift (`Meta_L` on most layouts)
is still Alt — the far end has the Shift and its own layout to make of them
what it does — and a key with no name at any level as its keysym, such as
the pad's 5 without Num Lock (`KP_Begin`). Each
physical key gets a stable id so a release finds its press whatever the
modifiers did meanwhile; every key still down is released when the window
loses focus. There is one local chord, **Ctrl+Alt+D** for the debug overlay
(and Ctrl+Alt on its own in relative mode, below): the D stays here, and the
chord's keys are let go of at the far end, as the escape does. Every other
key is the far end's while a session has focus and takes input (not in view
only, nor under a prompt) — including the compositor's or window manager's own
chords (unless **Grab keyboard** is off), see below — except while the pointer
is on the toolbar: the toolbar is this end's, so while it is under the pointer
the keyboard is too, the grab or the inhibitor is let go (Alt+Tab is the window
manager's again), the keys the far end has down are released there, and what
is typed goes nowhere. Moving off the toolbar takes the keyboard back.

**The mouse** sends absolute desktop coordinates, mapped through the view and
clamped to it — the desktop pixel drawn under the pointer, by the centre of
the device pixel it is in, so that a scaled desktop's last row and column can
be pointed at — with an RFB button mask (left 1, middle 2, right 4) and the
wheel as momentary presses of 8/16/32/64 (up, down, left, right); smooth
scrolling is turned into notches of fifty points. The far end's cursor shape,
where the protocol sends one, is drawn under the local pointer with the OS
cursor hidden over the desktop, as the Android app does.

**Native cursor** (a setting, off by default) makes that shape the window's
own pointer instead — winit's custom cursor, created through the event loop
from the shape's pixels and hotspot and set on the window — so the
compositor moves it, and it is as sharp as the compositor's own. It is used
only where this end owns the pointer: in relative mode the far end draws its
cursor into the picture and the local one is locked and hidden. A shape is
uploaded once (cached by its pixels and hotspot) and set only when what the
cursor should be changes: a new shape, the pointer arriving on the desktop
or leaving it for the toolbar, the default when the far end hides its cursor
or has sent none. Where the desktop is not drawn one to one the shape is
resampled to the view's scale, nearest neighbour, and its hotspot with it;
the log says the size and hotspot of each one set. `cursor.rs` says how this
shares the window's cursor with egui-winit, which sets it from egui's icon.

**Relative pointer mode** — the far end owning the cursor (QEMU's VNC, SPICE in
server mode) — locks (Wayland) or hides and centres (X11) the local pointer
while the window has focus, and sends raw motion as deltas; the far end draws
its own cursor into the picture. **Hold Ctrl and Alt together to let go** of
the pointer; a click on the desktop takes it back. Losing focus or
disconnecting also lets go.


**The clipboard**: what the far end copies becomes the local clipboard
(**Receive clipboard**); what was copied here while the window was away is
offered to the far end when the window regains focus, once per change (**Send
clipboard**). The rule is the phone's `SessionClipboard`: the last text seen
crossing either way is remembered and is never sent back where it came from,
and what is read here is remembered whether or not it may be sent, so that
turning sending on offers the next copy and not an old one. Proven against
`scripts/testvnc` with `wl-copy`/`wl-paste` and the far end's `xclip`, both
directions, both ways round, and on X11 with `xclip` at both ends.

**The debug overlay** (the **Debug** button, or **Ctrl+Alt+D**) is the
phone's `SessionHud`: a small monospace box in the bottom left of the
desktop, with its lines in its order where they apply here, and sampled
rates over a one-second window. The state, the desktop's size, frames a
second, damage rectangles in all and a second, the pixel path (texture
updates this frame, bytes a second through `read_region`, the last
rectangle), and the clipboard's length as the far end would be given it;
the cursor's desktop position (or *theirs* in relative mode), the buttons
held, the scale mode and scale in device pixels per desktop pixel, the
desktop's origin and size on screen; whether the mouse is captured or
hovering, the buttons, how many keys are held at the far end, and VIEW ONLY
when it is; the shortcuts inhibitor's state, the follower's (off, waiting
for the far end, or the size it asked for), the cursor shape (drawn or
native, its size and hotspot) and the two clipboard switches. The counters
are kept whether or not it is on, and the box is a few labels a frame while
it is.

**Prompts** are modal dialogs over the session, which is blocked until one is
answered — so Escape, or clicking outside, is a no: credentials (the user name
field only where the scheme has one; for RDP a name typed without a domain
keeps the session's, and `.\user` is a local account), a certificate the
[known hosts](#known-hosts) do not match, an unverifiable server (asked every
time, and never remembered), TigerVNC's question about an RSA-AES server key
(likewise), RealVNC's security prompt with its identity and encryption states in
words and its *remember server* box, and messages (OK, or confirm/cancel when
the far end asks a question).

### Known hosts

A server's certificate is one it made for itself, so — as on the phone, and as
over SSH — its identity is the key: the first time a certificate is accepted
with **Remember server** ticked, its fingerprint is pinned against the address,
and from then on a connection whose certificate matches asks nothing, and one
whose certificate does not asks loudly. The prompt says which it is: *you have
not connected to this server before*, *this server's identity belongs to
`<address>`, which you have connected to before* (the same certificate pinned
under another address — one machine by name and by number), or *this server's
identity has changed since you last connected*, where the box starts unticked.
Cancel, or Continue with the box unticked, pins nothing. A certificate that a
certificate authority would vouch for is asked about like any other: the
question is whether this is the machine it was last time.

The pins are in **`$XDG_CONFIG_HOME/vento/known_hosts`** (by default
`~/.config/vento/known_hosts`), one line per address: `<address> <fingerprint>`.
The address is the one typed, folded for case and nothing else, so
`host` and `host:3389` are two entries; the fingerprint is SHA-256, as the
backend printed it, and compared as its hex digits whatever their case or
separators. RustDesk pins the peer's key under `<id>@<rendezvous server>` and
takes a first one without asking, since the rendezvous server has signed it;
only a changed one is a question. The file is replaced whole (written beside
itself and renamed over) with mode 0600; a line that does not parse is ignored
and kept, a missing file is an empty one, and one that cannot be read is
treated as empty and left alone. To forget a server, delete its line.

Only the window writes it. The probe reads it — a pinned host connects
headless without `--trust` — and never writes it: `--trust` says yes for that
run and nothing else.

No backend keeps a store of certificates of its own: FreeRDP is told to read
and write none, TigerVNC hands its certificates over rather than keep its
viewer's `x509_known_hosts` (on the phone as well), and libvnc and the four
Rust clients never had one. What the backends do still keep is RealVNC's, under the state
directory (`--state-dir`, by default `$XDG_STATE_HOME/vento`):

| Backend | What | Where |
|---------|------|-------|
| RealVNC | the core's private key, its own record of identities accepted with *remember server*, its config and logs | `<state>/realvnc/net.pgaskin.remotedesktop/.vnc/` (`rsaKeys`, `identities`, `config.d/`), `<state>/realvnc/vnclog/` |
| RealVNC | the identity the core asks to have remembered, handed back as `Identity` next time — kept by this client, as `RealVncBackend` keeps it | `$XDG_CONFIG_HOME/vento/realvnc_identities` |
| FreeRDP | nothing; the directory exists because the library will not connect without a path for a store it is never allowed to use | `<state>/freerdp/` |

RealVNC's identity is the core's opaque blob rather than a fingerprint, and
only the core can compare one, so it is not a pin and the phone does not make
it one: the core asks its own security question, and the phone and this client
both leave the core's store where it is. TigerVNC's library used to accept a
certificate that verified against the system's certificate authorities (or
TigerVNC's own `~/.config/tigervnc/x509_ca.pem`) and matched the name without
handing it over; a patch in `patches/tigervnc` hands it over too, so it is
asked about and pinned like any other, here and on the phone.

### Display servers

winit picks the display server: Wayland where `WAYLAND_DISPLAY` (or
`WAYLAND_SOCKET`) is set, X11 where only `DISPLAY` is, with no falling back
from one to the other. `--x11` and `--wayland` override that, and the log's
`display server:` line says which the window got. What differs between the two
is behind one `match` in `src/gui/platform.rs` rather than a cfg: the keyboard
shortcuts, the pointer capture, and the native cursor's alpha.

The dependency choice: winit, egui-winit and egui-wgpu are driven directly
rather than through eframe, because eframe wraps away the things this window
needs — the raw display and window handles for the inhibitor and the X11
keyboard grab, device events for raw motion, keyboard events as winit delivers
them with the layout's logical key, and the event loop's wake-up (the backend's
mailbox wakes it through an event-loop proxy). It costs a page of surface
setup.


#### Wayland

What the window uses beyond `xdg_wm_base`:

- `zwp_keyboard_shortcuts_inhibit_manager_v1` — while the window has focus
  and a session takes input (connected, not view only, no prompt up) and the
  pointer is not on the toolbar, the compositor's own shortcuts are inhibited
  so that Alt+Tab and the like reach the far end. winit does not speak this
  protocol, so `src/gui/wayland.rs` borrows winit's connection and surface
  through the window's raw handles, binds the manager and a seat from the
  registry on an event queue of its own, and creates the inhibitor; it is
  released on focus loss, on disconnect, in view only, under a prompt and
  while the pointer is on the toolbar, and never created with **Grab
  keyboard** off. The log says when it is created, granted (`active`) and
  released. The compositor may show its own indicator
  while it is in force (niri does), and keeps one chord of its own to take
  focus away.
- `zwp_pointer_constraints_v1` and `zwp_relative_pointer_manager_v1` — the
  pointer lock and raw motion of relative mode, through winit's
  `CursorGrabMode::Locked` and `DeviceEvent::MouseMotion`.
- The clipboard goes through smithay-clipboard over winit's own connection:
  egui-winit makes both of its clipboards on Linux and uses smithay's where
  the display handle is Wayland, arboard's otherwise.
- The native cursor goes through winit's `CustomCursor`, which on Wayland is
  a `wl_shm` buffer in the pointer surface. winit takes straight alpha and
  premultiplies it (the shapes arrive straight, which is what the phone's
  `Bitmap.setPixels` takes too, and what TigerVNC's library makes of
  `CursorWithAlpha`'s premultiplied pixels), refuses anything over 2048
  pixels a side, and copies the pixels into a fresh buffer on *every*
  `set_cursor`, even of the same cursor — which is why the window's cursor
  is set on a change and never once a frame. The buffer is in device pixels:
  with `wp_viewporter` winit sets its destination to the buffer's size over
  the window's scale factor, and without it sets the buffer scale to the
  output's integer scale, so a shape resampled to device pixels per desktop
  pixel is the size of the desktop pixels under it on any scale (tried on
  niri at scale 1; the fractional case rests on the viewporter path). winit
  re-applies the cursor on every pointer enter, so one set lasts. `grim`
  leaves the cursor out of a screenshot unless asked (`-c`).

#### X11

Proven on `scripts/testx11` — Xvfb and Openbox, a plain X server and a real
window manager with global keybindings of its own, and the client on the
host as an ordinary X client of it — against `scripts/testvnc` and
`scripts/testqemu`. Xvfb has no DRI3, so wgpu takes llvmpipe there; on an X
server with a GPU it takes the hardware's Vulkan or GL through the Xlib
handles.

- **The keyboard is grabbed** in place of the inhibitor: an active
  `GrabKeyboard` on the window while it has focus and a session takes input,
  released on focus loss, on disconnect, in view only, while a prompt is
  up and while the pointer is on the toolbar (`src/gui/x11.rs`): there the
  keys go nowhere at the far end, so the window manager has them — in view
  only, and with the pointer on the toolbar, Super+Return reached Openbox's
  witness and Alt+Tab moved the focus. It goes over
  winit's own connection — the Xlib display from the raw handle, as XCB through
  x11rb — because X reports a grabbed key only to the client that grabbed it;
  winit keeps receiving every key. On the rig, Super+Return, Ctrl+Alt+T and
  Alt+Tab all reached the far end (whose own Openbox switched windows) while
  the window manager's witness log stayed empty and the active window did not
  change; with an xterm focused, Super+Return reached Openbox. Every chord
  goes, Alt+F4 included, so **the way out is the mouse**: a click on another
  window has the window manager move the focus, which ends the grab. Not every
  mouse binding survives the grab: Openbox's Alt+drag did not move the window
  while it was held (it did with another window focused). The log says `X11
  keyboard grabbed` and `released`, and the overlay's inhibitor line says
  `grabbed`. With **Grab keyboard** off it is never taken.
- **A refused grab is retried** every 250 ms while the window has focus and
  a session takes input, with the loop woken for it; the overlay says why
  meanwhile (`refused (already grabbed)`). Tried with Openbox's Alt+Tab
  switcher holding the keyboard, and with another client's grab: the grab
  was taken 9 ms after the other one ended.
- **The grab's own focus events are filtered.** A grab and an ungrab make
  the server send focus-out and focus-in events of their own, which winit
  reports as the focus moving; with the focus PointerRoot (no window
  manager) that is an endless ungrab/grab loop. A focus-out while the grab
  is held is checked against the server's focus (the window, or PointerRoot
  with the pointer inside it) and dropped if it is still ours, and a repeated
  focus event is dropped too. On a bare Xvfb with PointerRoot focus and
  another client's passive grab firing, one focus-out was filtered and keys
  kept arriving. Without a window manager the grab then holds until
  disconnect even when the pointer leaves the window, since the server sends
  a grabbing client no focus-out for that — not a supported setup.
- **Relative mode captures without a pointer grab.** winit's X11 `Locked` is
  unsupported, and `Confined` (an active pointer grab) has the server deliver
  every XI2 raw motion twice, once for the grab and once for the root
  window's selection, which would be double speed. So the cursor is hidden,
  raw motion is sent as before, and the pointer is warped back to the
  window's centre whenever it moves; a warp makes no raw motion, and the
  window swallows the motion events it causes. Until the warp, though, the
  pointer is wherever the move took it, and unfenced a fast move and a
  click went to the window beside (`xdotool mousemove_relative -- 700 0
  click 1` activated the witness xterm and ended the capture five times in
  five). So while captured there are XFixes pointer barriers on the
  window's four edges (`x11::Fence`), moved with the window: they stop the
  pointer on its edge, change nothing of what is delivered, and pass the
  warps. With them that move and click, and moves of up to 3000 pixels
  each way and diagonally, stayed in the window every time, also after the
  window was moved; Super+Return and Alt+Tab still went to the far end.
  They need XFixes 5 (2011); without it the capture is as before, unfenced,
  with a warning in the log. Ctrl+Alt, a focus loss and the disconnect take
  them down with the capture, and letting go puts the pointer back where it
  was before the capture. The overlay's mouse line shows the raw motion
  sent (`Δ x,y`): ten moves of 20 read 200,0 over VNC (`rfb`) and over
  SPICE, and the guest's cursor moved. A single report of 900 pixels, more
  than half the window, read 900,0 against the barrier and was warped
  straight back. Raw values
  from an absolute device (a VM's tablet, xdotool's absolute `mousemove`)
  are positions rather than deltas, which relative mode cannot use — as on
  Wayland. XI2 raw events stay selected whatever the focus, since winit
  deselects them on every focus-out, the filtered ones included.
- **A wheel notch is one notch.** A wheel without smooth-scroll valuators
  comes as a press and a release of buttons 4–7 per notch, and winit 0.30
  made a wheel event of both, so each notch went to the far end twice; it
  makes one of the press only with `patches/winit/0001`, as winit's next
  X11 backend does. A real wheel under libinput was not affected (its
  buttons are flagged emulated and dropped, the valuators read instead);
  XTEST's were, and those of an X server that is itself a remote session
  (Xvnc, xrdp). On the rig, `xdotool click 5` three times and `click 4`
  once over a TigerVNC session gave the far end 6 and 2 presses before,
  3 and 1 after.
- **The native cursor is an Xcursor, premultiplied.** winit copies the
  pixels it is given into a Render cursor as they are, which wants them
  premultiplied, so `cursor.rs` premultiplies them for X11 (Wayland's side
  does that itself). Read back through XFixes (`scripts/testx11/cursor.py`),
  each shape had the size and hotspot the log said, and no pixel had a
  colour above its alpha. There is no buffer scale: the cursor is in device
  pixels, so the resampling is right at any scale factor (×2 at
  `WINIT_X11_SCALE_FACTOR=2` in a fill mode, ×1 at 1:1).
- **The clipboard is arboard's**, which serves `CLIPBOARD` from a thread of
  its own over a connection of its own. Proven both ways with `xclip`. On
  exit it hands the text to a clipboard manager if one runs; without one,
  what the far end copied is gone when the window closes, as with any X
  client (on the rig, `xclip -o` then found no owner). arboard waits up to
  four seconds for an unresponsive selection owner, and the clipboard is
  read on focus-in, so a hung owner stalls that one frame.
- **Fullscreen** is `_NET_WM_STATE_FULLSCREEN`, which needs an EWMH window
  manager; under Openbox the window became the screen's 1600×1000, and
  with follow window size on the far end followed it there and back.
- **The scale factor** is `WINIT_X11_SCALE_FACTOR` if set (a number, or
  `randr`), otherwise `Xft.dpi` over 96 from XSETTINGS or the resources,
  otherwise RandR's idea of the monitor's DPI (1 on Xvfb, which reports a
  0 mm screen).

**Xwayland** — `--x11` on a Wayland session — is for testing, and the
Wayland backend is the one to use there; the log says so when the X server is
Xwayland. Tried on niri through xwayland-satellite 0.8.2 (Xwayland 24.1,
rootless): the window opens on the hardware's Vulkan, connects, draws the far
end, and takes its keyboard grab, and in relative mode the capture engages
and Xwayland puts its pointer at the window's centre. What is *unverified*,
because it needs hands on the real keyboard and mouse and no input was
injected into the live session: whether that grab holds off niri's own
shortcuts (that is the compositor's decision; niri does not offer ordinary
clients the Xwayland keyboard grab protocol, and its shortcuts sit behind
satellite), whether the real pointer stays in the window while captured
(Xwayland honours a warp through pointer-constraint emulation that depends
on its compositor), and absolute pointing and the native cursor there.

## RealVNC

RealVNC's viewer core is proprietary and nothing of it is in this repository.
The backend is built only when `clang` is on `PATH`; when it is not, the other
seven are built and RealVNC is left out with a note. The next `cargo build`
after `PATH` changes (clang installed, say) looks again, and when the answer
differs the C tree is configured afresh with the other compilers, so nothing
has to be cleaned by hand. To use it, take
`libvncviewer.so` and `libc++_shared.so` for your host's ABI out of the viewer
APK of the build `realvnc/.../Library.java` pins (the session verifies them by
SHA-256 and refuses any other), and point `--library-dir` at them:

```sh
mkdir -p ~/.local/share/vento/realvnc
unzip -j com.realvnc.viewer.android_4.9.3.60175.apk \
    'lib/x86_64/libvncviewer.so' 'lib/x86_64/libc++_shared.so' \
    -d ~/.local/share/vento/realvnc
cargo run -- realvnc 127.0.0.1::5901 --password vncpass
```

`--library-dir` defaults to `$XDG_DATA_HOME/vento/realvnc` and `--state-dir`
(where the core keeps its key, identities and logs — as plain files where the
phone has encrypted ones; see [Known hosts](#known-hosts)) to
`$XDG_STATE_HOME/vento`.
