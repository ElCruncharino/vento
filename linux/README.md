<!--
SPDX-FileCopyrightText: 2026 Patrick Gaskin
SPDX-License-Identifier: GPL-3.0-or-later
-->

# linux

A native Linux client for the backends under `backends/`. One process that can
hold a session on any of the eight protocols — the four Rust ones directly, the
four C/C++ ones through the shared libraries this crate's `build.rs` has CMake
build — written against the same seam the Android app's `Backend` is, minus the
phone.

This is the foundation: the `Backend` trait, the `Event` channel, the eight
adapters, the option registry, and a headless probe CLI in the spirit of
`backends/platform/desktop/c/probe.c`. There is no window yet; `main.rs` is
structured so a GUI mode is one arm of its dispatch and one module away.

## What is here

- **`src/backend.rs`** — the `Backend` trait (`Backend.java` without the
  Android types) and the value types around it (`Facts`, `ConnectionFact`,
  `Monitor`, `Traffic`, `Picture`).
- **`src/events.rs`** — the `Event` enum (`common::Events` plus the C API's
  `question`, `security_needed`, `message`) and the `Mailbox` a session pushes
  them onto, with an optional wake hook for a future GUI's `request_repaint`.
  Damage is coalesced into a dirty-rect list behind a mutex and delivered one
  `Event::Frame` per update, never a message per rectangle.
- **`src/options.rs`** — the per-backend option descriptions, ported from each
  `*Provider.java`, so `-o key=value` and a future dialog are generic.
- **`src/backends/`** — the eight adapters. `rfb`, `ironrdp`, `rustdesk` and
  `spice` are the twins of each Rust crate's `bindings/mod.rs`; `c.rs` plus
  `c/{libvnc,tigervnc,freerdp,realvnc}.rs` are the four C backends over the
  vtable in `vento/backend.h`.
- **`src/main.rs`** — the `vento` binary: `--list`, and a headless probe.

## Building and running

Host packages (the same the C host build needs, plus a libclang for bindgen):
cmake, ninja, pkg-config, a C/C++ compiler — **clang** for the RealVNC backend
(libc-shim does not build under GCC; without clang that one backend is left
out, with a build note) — and the development files for openssl, gnutls, nettle
(hogweed, gmp), libjpeg-turbo, pixman, zlib, FFmpeg (libavcodec, libavutil,
libswscale), libpulse, alsa-lib, plus libclang.

The submodules must be initialised (`git submodule update --init --recursive`;
RealVNC's four host-only submodules need `--recursive`).

```sh
# clang for RealVNC; the three open backends build under either compiler.
CC=clang CXX=clang++ cargo run -p vento -- --list

# A session, headless: connect, print every event, answer the prompts from the
# flags, write the framebuffer and a thumbnail, print the facts, and leave.
cargo run -p vento -- tigervnc 127.0.0.1:5901 --password vncpass \
    --out /tmp/shot.ppm --click 500,300 --key 0x61 --clipboard hello
cargo run -p vento -- rdp 127.0.0.1:3389 --user proto --password protopass \
    --trust --out /tmp/rdp.ppm
cargo run -p vento -- rustdesk 127.0.0.1 -o ConnectBy=address \
    --password rustdeskpass --out /tmp/rd.ppm
cargo run -p vento -- spice 127.0.0.1:5940 --password spicepass --out /tmp/sp.ppm
```

`build.rs` runs `backends/CMakeLists.txt` with `-DVENTO_SHARED=ON`, which makes
each backend a `libvento_<name>.so` under the build tree carrying its vendored
static dependencies and the stderr logger; the binary links those four by name
with an rpath into the build directory, so `cargo run` works from the target
directory without anything on `LD_LIBRARY_PATH`. The existing static path and
the C probes are unaffected (`cmake -S backends -B … ` without the option).

The probe's exit status is zero only if it connected and everything asked for
happened. `vento --list` prints each backend and its `-o` options.

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
cargo run -p vento -- realvnc 127.0.0.1::5901 --trust --password vncpass \
    --out /tmp/rv.ppm
```

`--library-dir` defaults to `$XDG_DATA_HOME/vento/realvnc` and `--state-dir`
(where the core keeps its key, identities and logs — as plain files where the
phone has encrypted ones, and where the other backends keep their own per-host
state) to `$XDG_STATE_HOME/vento`.
