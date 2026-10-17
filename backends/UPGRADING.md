---
name: upgrade-backends
description: Upgrade the third-party upstreams behind the backends (submodules under third_party/, the nettle and gnutls tarballs, and the generated ports), one at a time, shared libraries first, each as its own reviewed commit. Use when asked to upgrade, bump, or update the backends, a submodule, or a port.
---

# Upgrading the backends

Every backend is a shim of ours over somebody else's client, pinned as a
submodule under `third_party/` (or, for nettle and gnutls, as a release
tarball named in `backends/build.gradle`). Upgrading one is moving that pin,
carrying our patches across, regenerating whatever is generated, adapting the
shim, and proving the build. This file is the procedure that came out of the
first full round (September 2026), including the things that went wrong.

Read `patch.sh` and `.gitmodules` first: the gitlink names the *pristine*
upstream commit, our changes live in `patches/<name>/`, and the `base` tag in
each submodule remembers which is which. `git add -A` at the root records the
patched commit as the pin and doubles the patches on the next clone; never do
it.

## Scope

- In scope: the submodules, the two tarballs, the ports' `gen/` output, our
  shims, and any new option an upstream change makes worth offering.
- Out of scope unless asked: the toolchain pins (NDK, cmake, rust channel,
  cargo-ndk, Gradle/AGP, JDK) and the app's own Java/Gradle dependencies.
  Rust crate bumps beyond what a moved submodule's manifests force are also
  out; `cargo update` is its own job.

## Order

Shared libraries first, each separately, then the backends, one at a time.
Nothing runs in parallel: every step needs a clean tree to commit from.

1. libjpeg-turbo (libvnc, tigervnc)
2. pixman (tigervnc)
3. openssl (freerdp), via `backends/ports/openssl/gen.sh`
4. nettle, then gnutls (libvnc, tigervnc), via the tarball pins and
   `backends/ports/{nettle,gnutls}/gen.sh`
5. libvncserver, tigervnc, freerdp, ironrdp, hbb_common, ryll

`third_party/winit` is not a backend's and not in this round: it is the
Linux client's winit, pinned at the tag of the release `linux/Cargo.lock`
names and taken in through `linux/Cargo.toml`'s `[patch.crates-io]`. It moves
when that crate's winit does (a `cargo update` there, its own job): the pin to
the new release's tag, `patches/winit/` rebased onto it, and any patch
upstream has taken dropped.

`third_party/ffmpeg` and `third_party/dav1d` are the Linux client's too, and
not in this round either: the static FFmpeg `backends/platform/desktop/build.rs`
builds for every desktop decoder (the Rust one, which TigerVNC's and
FreeRDP's H.264 go through too, and FreeRDP's sound and scaling), each pinned at a
release tag. FFmpeg moves
to a newer release's tag with `patches/ffmpeg/` rebased onto it; the decoders it
is configured with are `DECODERS` and `PARSERS` in that build script, so a
consumer that starts using another codec adds it there. Proven by the desktop
crate's fixture tests, the RustDesk probe with each codec, and the TigerVNC
H.264 and FreeRDP AVC probes. Android builds neither.

`third_party/libva` goes with FFmpeg: headers only, for its VAAPI decoders
(`HWACCELS` in the same build script). Nothing of it is compiled or linked;
libva is reached through `backends/platform/desktop/c/va_shim.c`, which
defines each libva function FFmpeg calls and loads the real one on first
use. A newer FFmpeg may call a libva function the shim does not have yet,
which is a link error naming it (`undefined symbol: va...`): add it to
`VA_FUNCTIONS` there. `vento_video.c` calls two of them itself for a held
picture (`vaSyncSurface`, `vaExportSurfaceHandle`) and reads FFmpeg's
`AVVAAPIDeviceContext` for the display, which a new FFmpeg could rename.
libva moves to a newer release's tag when FFmpeg wants newer headers (its
configure says which `VA_CHECK_VERSION` a feature needs). Proven by the
desktop crate's `hardware` and `hardware_auto` tests with
`VENTO_TEST_HW=require` on a machine with a VAAPI device, and
`hardware_off`.

A shared library can be blocked by a consumer: nettle 4 could not move until
TigerVNC had upstream's nettle-4 compatibility. When an evaluation says "stay"
because of a consumer, note it and come back after that consumer moves.

## Who does what

For each upstream, one orchestrating agent (the strongest model available)
that spawns two subagents in turn and then reviews their work itself:

1. **Evaluate** (strong model). Reads the real diff between `base` and the
   candidate, not the changelog: every API item, setting, callback, CMake
   option and target our shim uses, checked against the target; patches
   dry-run against a scratch worktree (`git apply --check`); release vs
   development-tip judged for a phone app; candidate options proposed with
   key, label, summary, type, choices, default, scope and live. Concrete
   findings with file:line references.
2. **Upgrade** (the implementation model). Moves the pin, rebases the
   patches, adapts the shim, adds the agreed options end to end, runs the
   verification list. Does not commit.
3. **Review** (the orchestrator). Independently verifies pins, reads every
   regenerated patch hunk by hunk against the old one, reviews the whole
   diff, reruns the builds. Fixes small things itself; sends larger ones
   back. Does not commit.

The lead reads the report and the diff, then commits. Give every agent a
written conventions file (this one plus the session's specifics: target
candidates already fetched, what is out of scope, where scratch files go)
rather than a prompt from memory; agents get cut off and resumed, and a
file survives that where a transcript does not. Have them save their reports
to the scratchpad as they go, for the same reason.

## Before starting

    for p in third_party/*; do git -C "$p" fetch origin; done
    git -C third_party/<name> ls-remote --tags origin   # releases live here

The submodules are shallow. `git log base..origin/master` is grafted and
lies about counts; `git diff --stat`, `git ls-remote` and
`merge-base --is-ancestor` tell the truth, and `git fetch --unshallow` is
fine when a real log is needed. Tags on release branches (openssl's
`openssl-3.5.*`) are not fetched by default.

Which pin to pick:

- A release tag on the maintained line, if one exists that the consumers can
  use. Check which line is maintained (libjpeg-turbo's `main` became the 3.2
  line while `3.1.x` went to ESR; openssl 3.5 is LTS while 3.6 was a year
  from EOL and 4.0 removes APIs the port still names).
- The development tip only for a concrete reason, and then a cut that
  builds: TigerVNC had no release in six months and master carried the
  nettle-4 compatibility, the leak fix and a features-off configuration
  upstream now builds itself, but only cuts after the gettext work stopped
  needing libintl.
- Stay, honestly, when there is nothing to gain: pixman's master touched
  only SIMD compositing that the region code never reaches; gnutls and
  libvncserver were already at the newest thing that exists.

## Moving a pin

With patches:

    cd third_party/<name>
    git rebase --onto <TARGET> base HEAD
    # re-derive each conflict against the new code; drop a patch upstream
    # made redundant and say which commit did; keep subjects and bodies
    git tag -f base <TARGET>
    cd -
    git update-index --cacheinfo 160000,<TARGET>,third_party/<name>
    ./patch.sh <name> && ./patch.sh check

Without patches: `checkout --detach`, `tag -f base`, `update-index`,
`./patch.sh check`. A submodule with no `base` tag yet gets one at the
target.

A tarball: bump version, sha256 and both URLs in `backends/build.gradle`,
verify the hash against upstream's signature or announcement (not against
the download), `rm -rf third_party/<name>`, `./gradlew
:backends:fetchCryptoSources`, then the port's `gen.sh`. gnutls' gen builds
nettle itself, so a nettle bump reruns gnutls' gen too and may change its
`gen/`.

A port's `gen.sh` rerun at the same pin must produce an empty diff. If it
does not, fix the script first as its own commit, regenerated at the old
pin, so the upgrade's diff is only the upgrade. The three things that have
broken this: a build date baked in (pin it to the commit date via
`SOURCE_DATE_EPOCH`), a path recorded relative to a `mktemp` directory (build
in a fixed directory under the port; `build` is gitignored), and `sort`
under the host locale (`export LC_ALL=C`).

## Adapting the shim

- A C backend's shim is one file under `src/main/cpp`, `<name>_session.c`
  (`.cxx` for TigerVNC), which drives the library against the neutral C API
  in `backends/common/include/vento/` and names no platform. Above it is the
  crate in `src/main/rust`, which fills the backend's `vento_<name>_config`
  from the options and words its rows; it is what the Linux client holds,
  and on the phone it is linked into the module's library as a static
  library (`rustStaticLibrary` in `backends/build.gradle`,
  `backends/cmake/vento_android.cmake`) with the one JNI layer in
  `backends/platform/android`. An upstream change reaches the session file;
  the crate changes only when the config does, and so does `<name>_probe.c`,
  the host probe's `main` beside them. The module's
  CMakeLists is one file for both builds, `if(ANDROID)` and `else()` around
  a shared middle: a vendored tree's new option goes in the middle, a new
  port or NDK library in the Android half, a new system library in the host
  half (see `backends/CMakeLists.txt`).
- A Rust backend's shim is its protocol crate, and its adapter is that
  crate's `src/backend.rs`: the `Kind`, the config read from the options, and
  the rows. An upstream change (IronRDP, hbb_common, ryll) reaches the
  protocol code; `backend.rs` changes only when an option or a row does.
- Next to nothing about a protocol is in Java or JNI any more. The phone
  reaches every backend but RealVNC through one JNI layer,
  `backends/platform/android`, over the same `session::Backend` the Linux
  client holds; what a module's Java keeps is its provider and, for RDP and
  the two borrowed VNC clients, a `NativeBackend.Hooks`. An upgrade that
  keeps the adapter's `Kind` building has nothing to change on that side.
- TigerVNC's H.264 decoder context is the shim's own on both platforms
  (`H264VentoDecoderContext`, selected by `H264_VENTO` in their factory; their
  libav one is not built, `HAVE_LIBAV` unset), and hands each rectangle to
  vento's video decoders through `vento/video.h`'s table. Its QEMU audio is
  played on vento's sound output through `vento/audio.h`'s table
  (`VentoAudio.h`; AAudio on the phone, ALSA on the host, both behind
  `common::audio`'s queue), not their viewer's `AudioOutput`, which is not
  compiled. An upstream change to `H264DecoderContext`
  or `H264Decoder` (the reset flags, the 64-context limit) or to the QEMU
  audio calls in `CConnection` reaches both platforms.
- Every comment of ours that names an upstream version or behaviour ("in
  3.19.1", "the assembly is GAS", "returns Some") is a claim to re-verify
  against the target and correct or remove. Grep for the old version string
  across `backends/` and `README.md`, excluding `gen/`.
- Defaults that upstream flips reach us silently. FreeRDP turned ExtSecurity
  on and raised its TLS floor; a client that "prefers NLA" or "never NLA" has
  to drive every switch upstream added.
- Reproducibility stamps: a new upstream may add a git-describe version, a
  build timestamp or a host path (`USE_VERSION_FROM_GIT_TAG`,
  `BUILD_TIMESTAMP`). Pin them; `strings` the built `.so` for a date, a
  hostname or a path.
- Warnings in a vendored tree are silenced at the target
  (`backends/cmake/warnings.cmake`), never on ours and never globally.
- The README's list of blob directories under `third_party/`, and the
  licence file paths `app/build.gradle` reads, are both things a moved pin
  can invalidate.
- New host tools an upstream starts looking for (JSON parsers, Opus,
  uriparser) are switched off by name, so the configure result does not
  depend on what the build machine has installed.

## Options

A backend's settings are its table in `backends/session/src/options.rs`, one
`Spec` per row, and the one copy: the Linux client builds its dialog and
`-o key=value` from it, and on the phone `vento-codegen` writes it into the
module's generated `<Name>Catalog` at build time, which the provider answers
from. When an upstream change adds something a person would switch (a new
encoding or codec, a security type, a behaviour toggle), add a row in the
shape, wording and scope of its neighbours, read it in the adapter (`o.get`,
`o.bool`, `o.int`) into the config — for a C backend a new field in
`vento_<name>_config`, which `session/build.rs` reads into Rust with
bindgen — and default it to upstream's answer, and add its key to the
test `the_keys_are_the_ones_records_hold`, which is there because a stored
key renamed is a setting forgotten. The value reaches a session
in the options it is created with, every key and value as strings, so
nothing in Java or JNI changes for a row. Where the phone's words are wrong
on a desktop ("this phone"), give the row a `desktop_summary` or the choice
a `desktop_label`. Do not add rows for what the app already decides, what nobody would set, or
what cannot be exercised here (an H.264 path with no decoder behind it).

The user-facing strings in this app are hand-written by its owner. A new
row's label and summary are a draft; name them in the report so they get
rewritten.

## Adding a backend

Not part of an upgrade round, but the same layers, so here once. A backend
is a module under `backends/modules/<name>/`, and each layer of it is one
line or one file somewhere that already has seven:

- **The protocol.** A Rust crate in `src/main/rust` (`crate-type = ["lib",
  "cdylib"]`), or a C shim in `src/main/cpp` over `vento/backend.h` with its
  config in `backends/common/include/vento/<name>.h` (added to
  `session/build.rs`'s bindgen header), a host probe `<name>_probe.c`, and a
  small crate in `src/main/rust` beside it (`remotedesktop-session` with the `c` feature; `session::c::start`
  and a `Behaviour` do the rest).
- **The session crate.** An `About` in `catalog.rs` (and in `ALL`), its
  options table in `options.rs`, and in the backend's own crate a `Kind`:
  the `About`, the version, and `create`, which turns a `Connect` into an
  `Arc<dyn Backend>`. Credentials prompts, the trust decision
  (`session::trust::decide` over each client's `Store`), the bell and the
  events are the session crate's and are not written again.
- **The Linux client.** The crate in `linux/Cargo.toml` and its `KIND` in
  `linux/src/backends/mod.rs`'s `all()`; a C module also goes into
  `backends/CMakeLists.txt`, which `linux/build.rs` builds, and into the
  lists of directories and libraries there that it watches and links.
- **The phone's library.** One `platform_android::natives!` line in the
  crate's `lib.rs` under `cfg(target_os = "android")`, naming the module's
  natives class and the `Kind` (`decoders =` for a backend whose picture is
  video, `on_load =` for a linked library that wanted a `JNI_OnLoad` of its
  own), with `remotedesktop-platform-android` as an Android-only dependency.
  A Rust module's `build.gradle` has cargo-ndk build the cdylib into its
  `jniLibs` (rfb's, copied); a C module's builds the crate as a static library
  (`rustStaticLibrary`), passes CMake `-DVENTO_RUST_LIBRARY_DIR`, and its
  CMakeLists calls `vento_android_library(<target> <crate>)` from
  `backends/cmake/vento_android.cmake` in its Android half.
- **The phone's Java.** The module's `build.gradle` registers
  `generateCatalog` and `generateNatives` from `:backends`; its
  `<Name>Provider` answers from `<Name>Catalog` and creates a
  `NativeBackend` on `new <Name>Natives()`, and is declared in
  `src/main/resources/META-INF/services/`, as a `BackendProvider`.
  What the protocol needs of the phone that the layer does not do goes in
  a `NativeBackend.Hooks` (`RdpHooks` remembers RDP's desktop sizes and words
  the prompt's hint, `VncHooks` words an anonymous TLS question); most
  backends need none. `settings.gradle` includes the module, and the app
  takes it from `backendModules` in `app/build.gradle` — or an add-on under
  `plugins/` does, as RustDesk's.
- **The checks.** A line in `scripts/check-jni-symbols.sh`'s `MODULES`, and
  the probe (below) against a test desktop.

## Verification

Native modules:

    ./gradlew :backends:<module>:assembleRelease
    ./gradlew :backends:<module>:assembleRelease -PstrictWarnings

The C backends again on the host, which is the one build that then runs — a
probe per backend, against the test desktops under `scripts/` (README,
Development). The three open ones build against the system libraries; the
RealVNC one builds under clang only and runs their pinned library out of
the viewer APK, so a change to `realvnc/.../Library.java`'s pins, to
`realvnc_traffic.h`'s walk or to the four host-only submodules is proven
here and nowhere else:

    CC=clang CXX=clang++ cmake -S backends -B build/scratch/host -G Ninja -DSTRICT_WARNINGS=ON
    cmake --build build/scratch/host

Rust modules:

    cargo check --locked --workspace
    cargo test --locked -p <crate>
    cargo clippy --locked --workspace
    ./gradlew :backends:<module>:assembleRelease

Every backend through the seam, on the host: the Linux client (`linux/`,
its own workspace; see its README) builds all eight (RealVNC under clang
only), and its headless probe connects one, writes the picture out and
exits — the run that goes through the same adapters the phone does, RealVNC's
aside:

    (cd linux && cargo build && cargo test && cargo clippy --all-targets)
    linux/target/debug/vento <backend> <address> --password ... --trust \
        --probe 3 --out build/scratch/<backend>.ppm

Always `./patch.sh check`. After an options change, `cargo test --locked -p
remotedesktop-session` and `./gradlew :backends:testDebugUnitTest`: the
first checks every table's defaults against its own rows and the keys
records hold, and both hold `settle`, and its Java copy, to
`backends/vectors/options.json`. After
any change to a native surface — `platform_android`'s `surface.rs`,
`NativeBackend`, or a module's natives class or library —
`scripts/check-jni-symbols.sh`. At the end of the round: `./gradlew :app:assembleDebug
:plugins:realvnc:assembleDebug :plugins:rustdesk:assembleDebug`.

Nothing in the Gradle or cargo steps runs on a device or against a real
server. The probes are the exception: they run against the Docker desktops
under `scripts/` (`testvnc`, `testrdp`, `testspice`, `testrustdesk`;
`testh264` for TigerVNC's H.264, which needs a Wayland session and a GPU to
nest in). Say in the report which were run and which were not, rather than
implying otherwise; the other harnesses under `scripts/` need VMs.

Build in the repository's own `target/` and `.cxx`. `/tmp` is a RAM-backed
tmpfs: no cargo target directories, scratch workspaces, unpacks or clones
there, including in the agent scratchpad. A scratch build goes under
`build/scratch/` (gitignored) and is deleted afterwards.

## Committing

One commit per upstream, after review, by the lead:

- Subject `Update <name> to <version>` for a release, or `Update <name> to
  owner/repo@<full sha>` for a commit; a body that says what upstream changed
  that reached us, what the shim did about it, and what happened to each
  patch.
- Upstream commits as `owner/repo@<full sha>` wherever a GitHub repository
  backs the submodule, in subjects and bodies alike, substituted in place
  without rewrapping the text around them.
- Signed (`git commit -S`). Author is the model that led the work, committer
  is the repository owner, with the implementing model as `Co-Authored-By`.
  Preserve author and commit dates if rewriting.
- `git add` the named paths, never `-A`; the gitlink is already staged by
  `update-index`.

## What each upstream is, as of the September 2026 round

- **libjpeg-turbo** 3.2.0. One patch (add_subdirectory from downstream).
  `main` is the 3.2 line; `3.1.x` is ESR. arm64 SIMD is Neon intrinsics;
  x86 SIMD stays off (needs NASM).
- **pixman** 0.46.4. No patches. Hand-written source list in
  `backends/modules/tigervnc/src/main/cpp/cmake/pixman.cmake`; only the region
  API is reached. The checkout's origin is freedesktop's gitlab while
  `.gitmodules` names the X11Libre mirror; they match.
- **openssl** 3.5.8, LTS to 2030. No patches. Moving to 4.x needs `no-ech`,
  and the engine references in `gen.sh` and the port's CMakeLists go.
- **nettle** 4.0, **gnutls** 3.8.13 (the release that added nettle-4
  support; newest in the 3.8 line). nettle 4.1 warns of further API changes.
  `MINI_GMP_ENABLE_FLOAT=0` is an answer configure puts in CFLAGS, so the
  port sets it itself.
- **libvncserver** at upstream master; five patches. No releases since
  0.9.15.
- **tigervnc** master dd416cbf, reporting 1.16.80; six patches (the PAM one
  dropped, and the libav context's pixel format fix with that context). The certificate check is the shim's now: what cannot be excused is
  refused and the rest goes to `trust_needed`, their viewer's known-hosts store
  not used at all; the RSA-AES key question is ported from
  `vncviewer/CConn.cxx`; audio is behind the `Audio` row and plays on
  vento's sound output through `vento/audio.h`'s table (`VentoAudio.h`).
  Builds `common/` only, with `config.h` written by our
  CMake and, on Android, an empty `pthread` target for the link.
- **freerdp** 3.32.0; one patch (`0001`, codec/h264: `WITH_H264_EXTERNAL`,
  a slot for an application's subsystem, `g_Subsystem_external`, tried
  first, and `WITH_GFX_H264` following it). The module's `h264_vento.c` is
  built against `libfreerdp/codec/h264.h`, which is internal: check that
  `H264_CONTEXT_SUBSYSTEM` and the context's `pYUVData`, `iStride`,
  `YUVWidth`, `YUVHeight` and `pSystemData` still mean the same, that
  contexts are still made, reset and fed inside the GDI's SurfaceCommand
  (the session sets whose they are around it), and that AVC444 still feeds
  both pictures through the surface's one context. No `WITH_VIDEO_FFMPEG`,
  `WITH_FFMPEG_HWACCEL` or `WITH_MEDIACODEC`. `LegacyTls` row for TLS 1.0 at
  level 0; `remotedesktop_vendored_warnings` silences their tree.
- **ironrdp** master 9b151c4c. No patches. A submodule rather than crates.io
  because the bitmap stride fix (80bb81b3) is still unreleased; revisit at
  the next ironrdp-session release. EGFX is deliberately off; the doc
  comment on `session::options::IRONRDP` says why.
- **hbb_common** 3d6fb2c, and terminal there: the next upstream commit moved
  `message.proto` into rustdesk's own repository
  (`libs/base/protos/message.proto`). Moving further means a second source
  for that file (a rustdesk submodule, or vendoring the one file with its
  commit recorded) and `crypto.rs` importing `IdPk` from the rendezvous
  module. The kx-v2 key exchange that followed is not needed: this client
  never answers the exchange, like theirs without a token.
- **ryll** develop 3ba0f24; four patches, none merged upstream. No tag since
  v0.1.7. Only the protocol and compression crates are consumed; `client` is
  deliberately not (bundled root store).
- **logger**, **libc-shim**, **mcpelauncher-linker** (branch `ng`, with its
  nested `bionic` and `core`) and **libjnivm** (October 2026): the host-only
  stack under `backends/platform/desktop/realvnc`, pinned at the commits
  that stack was proven with and in no APK. No patches. Moving any of them
  is proven by the RealVNC probe alone; the libc-shim gaps the hosting
  layer closes from glibc (`Loader.cxx`) are the things to re-check.
