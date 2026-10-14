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
  `Event::Frame` per update, never a message per rectangle.
- **`src/options.rs`** — the per-backend option descriptions, ported from each
  `*Provider.java`, so `-o key=value` and the dialog's rows are generic.
- **`src/backends/`** — the eight adapters. `rfb`, `ironrdp`, `rustdesk` and
  `spice` are the twins of each Rust crate's `bindings/mod.rs`; `c.rs` plus
  `c/{libvnc,tigervnc,freerdp,realvnc}.rs` are the four C backends over the
  vtable in `vento/backend.h`.
- **`src/gui/`** — the window: `mod.rs` (the winit loop, the wgpu surface,
  egui drawn into it), `session.rs` (a live session: the desktop, the input,
  the toolbar, the prompts, the cursor), `desktop.rs` (the desktop as one
  texture, updated a damaged rectangle at a time), `view.rs` (scale modes,
  panning and the pointer's mapping, pure arithmetic with tests), `follow.rs`
  (the desktop following the window's size: the phone's `WindowFollower`,
  pure logic with tests), `keys.rs` (winit keys to X11 keysyms), `wayland.rs`
  (the keyboard shortcuts inhibitor), `cursor.rs` (the far end's cursor as
  the window's own, with tests), `hud.rs` (the debug overlay's box and
  rates, with tests), `settings.rs` (the client's own settings and their
  rows), `dialog.rs` (the connection dialog), `panel.rs` (the connection-info
  panel, with the live controls), `prompts.rs` (the modal prompts).
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
RealVNC's four host-only submodules need `--recursive`).

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
# one's: the clipboard in each direction (both on unless told otherwise) and
# the far end's cursor as the window's own (off unless told). The dialog's
# "Settings" section has the same three.
cargo run -- tigervnc 127.0.0.1:5901 --no-clipboard-out --no-clipboard-in
cargo run -- tigervnc 127.0.0.1:5901 --native-cursor

# Straight into a session, with --connect. Without --password the credentials
# prompt asks.
cargo run -- tigervnc 127.0.0.1:5901 --password vncpass --connect
cargo run -- rdp 127.0.0.1:3389 --user proto --password protopass --connect
cargo run -- rustdesk 127.0.0.1 -o ConnectBy=address --password rustdeskpass --connect
cargo run -- spice 127.0.0.1:5930 --password spicepass --connect

# The backends and their options.
cargo run -- --list

# A session, headless: connect, print every event, answer the prompts from the
# flags, write the framebuffer and a thumbnail, print the facts, and leave.
# Any of the probe's flags (--probe, --out, --click, --key, --clipboard,
# --trust, …) selects this instead of the window.
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

The probe's exit status is zero only if it connected and everything asked for
happened. `--verbose` lets the backends' debug lines through (and not the GUI
stack's, which would be tens of thousands a minute).

## The window

One window, which is either the connection dialog or a session. A session
that ends goes back to the dialog, filled in with what it was connected to and,
unless the disconnect was asked for, told why it ended — and to a window, if
the session had gone fullscreen. The window's title is the desktop's name once
the protocol has said it, and rings the bell by putting one in the title for a
moment.

**The toolbar** floats at the top edge and stays out of the way: it slides off
the top of the window when the pointer is elsewhere and slides back when the
pointer touches the top edge (and for the first three seconds of a session, so
that it is seen once). On it:

- **Scale** cycles fit width → fit height → fill width → fill height → 1:1.
  *Fit* never enlarges the desktop past a device pixel per desktop pixel;
  *fill* does; 1:1 is a desktop pixel per device pixel. A desktop that is
  smaller than the window in an axis is centred in it; one that is bigger is
  panned — move the pointer within 48 points of a window edge, over the
  desktop, and it moves that way, faster the nearer the edge.
- **Monitor** (when the far end reports more than one) shows the whole desktop
  or one monitor's rectangle, with the scale modes applied to that. Where a
  backend offers *displays* instead (RustDesk sends one screen at a time) the
  button asks the far end for the next one.
- **View only** stops the mouse, the keyboard and the clipboard going to the
  far end, and tells the backend so.
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
requires a switch. Then the two that are not options, because whether they
can be offered is a fact about the live connection: **Desktop size**, where the
far end takes one (`can_resize`), offering this window's size in desktop
pixels first, the size the desktop has, the far end's own screens and the
usual sizes, and asking `request_desktop_size` for the one picked — what
arrives is what the far end granted, and the dropdown shows that rather than
what was asked; and **Display**, where the far end sends one of several
screens at a time (RustDesk), asking `request_display`. And **Follow window
size**, below, and the three **settings**, below, which a running session
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
(both on by default; `--no-clipboard-out`, `--no-clipboard-in`), and **Native
cursor** (off by default; `--native-cursor`), below. They are a section of
the connection dialog, apart from the backend's options because they are not
the connection's, and rows of the connection panel, where a session shows
what it is running with and takes a change at once. Nothing is saved: a
session hands them back to the dialog when it ends, and the process forgets
them when it does.

**Keys** go to the far end as X11 keysyms, from winit's *logical* key — what
the layout says the key means — as Latin-1 keysyms where there are such, the
Unicode keysym (`0x01000000 | codepoint`) otherwise, and a table for the named
keys; the number pad sends `KP_*`, and modifiers are told apart by side. Each
physical key gets a stable id so a release finds its press whatever the
modifiers did meanwhile; every key still down is released when the window
loses focus. There is one local chord, **Ctrl+Alt+D** for the debug overlay
(and Ctrl+Alt on its own in relative mode, below): the D stays here, and the
chord's keys are let go of at the far end, as the escape does. Every other
key is the far end's while a session has focus — including the compositor's
own chords, see below.

**The mouse** sends absolute desktop coordinates, mapped through the view and
clamped to it, with an RFB button mask (left 1, middle 2, right 4) and the
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

**Relative pointer mode** — the far end owning the cursor (QEMU's VNC, SPICE
in server mode) — locks the local pointer to the window while it has focus,
hides it, and sends raw motion as deltas; the far end draws its own cursor
into the picture. **Hold Ctrl and Alt together to let go** of the pointer; a
click on the desktop takes it back. Losing focus or disconnecting also lets
go.

**The clipboard**: what the far end copies becomes the local clipboard
(**Receive clipboard**); what was copied here while the window was away is
offered to the far end when the window regains focus, once per change (**Send
clipboard**). The rule is the phone's `SessionClipboard`: the last text seen
crossing either way is remembered and is never sent back where it came from,
and what is read here is remembered whether or not it may be sent, so that
turning sending on offers the next copy and not an old one. Proven against
`scripts/testvnc` with `wl-copy`/`wl-paste` and the far end's `xclip`, both
directions, both ways round.

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
field only where the scheme has one), a certificate's fingerprint to trust for
this session (nothing is persisted), an unverifiable server, TigerVNC's own
questions, RealVNC's security prompt with its identity and encryption states in
words and its *remember server* box, and messages (OK, or confirm/cancel when
the far end asks a question).

### Wayland

Wayland first, through winit. What the window uses beyond `xdg_wm_base`:

- `zwp_keyboard_shortcuts_inhibit_manager_v1` — while the window has focus
  and a session is connected, the compositor's own shortcuts are inhibited so
  that Alt+Tab and the like reach the far end. winit does not speak this
  protocol, so `src/gui/wayland.rs` borrows winit's connection and surface
  through the window's raw handles, binds the manager and a seat from the
  registry on an event queue of its own, and creates the inhibitor; it is
  released on focus loss and on disconnect. The log says when it is created,
  granted (`active`) and released. The compositor may show its own indicator
  while it is in force (niri does), and keeps one chord of its own to take
  focus away.
- `zwp_pointer_constraints_v1` and `zwp_relative_pointer_manager_v1` — the
  pointer lock and raw motion of relative mode, through winit's
  `CursorGrabMode::Locked` and `DeviceEvent::MouseMotion`.
- The clipboard goes through arboard (egui-winit's clipboard on Linux).
- The native cursor goes through winit's `CustomCursor`, which on Wayland is
  a `wl_shm` buffer in the pointer surface. winit takes straight alpha and
  premultiplies it (the shapes arrive straight, which is what the phone's
  `Bitmap.setPixels` takes too, and TigerVNC's library makes of
  `CursorWithAlpha`'s premultiplied pixels), refuses anything over 2048 pixels a side, and copies
  the pixels into a fresh buffer on *every* `set_cursor`, even of the same
  cursor — which is why the window's cursor is set on a change and never
  once a frame. The buffer is in device pixels: with `wp_viewporter` winit
  sets its destination to the buffer's size over the window's scale factor,
  and without it sets the buffer scale to the output's integer scale, so a
  shape resampled to device pixels per desktop pixel is the size of the
  desktop pixels under it on any scale (tried on niri at scale 1; the
  fractional case rests on the viewporter path). winit re-applies the
  cursor on every pointer enter, so one set lasts. `grim` leaves the
  cursor out of a screenshot unless asked (`-c`).

The dependency choice: winit, egui-winit and egui-wgpu are driven directly
rather than through eframe, because eframe wraps away the things this window
needs — the raw display and surface handles for the inhibitor, device events
for raw motion, keyboard events as winit delivers them with the layout's
logical key, and the event loop's wake-up (the backend's mailbox wakes it
through an event-loop proxy). It costs a page of surface setup.

### X11

Untested and not a priority. winit's X11 backend is compiled in, so the
window should open there; the shortcuts inhibitor is Wayland-only (there is
no X11 equivalent short of grabbing the keyboard), and the pointer lock goes
through winit's X11 confinement, which has not been tried.

## RealVNC

RealVNC's viewer core is proprietary and nothing of it is in this repository.
The backend is built only when `clang` is on `PATH`; when it is not, the other
seven are built and RealVNC is left out with a note. To use it, take
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
phone has encrypted ones, and where the other backends keep their own per-host
state) to `$XDG_STATE_HOME/vento`.
