# backends/

The eight remote desktop backends, what they share, and how they are built
for the phone and for the Linux client. This is a map of where things are
now; the layout is being reorganised in stages, and each stage updates it.

**The rule:** a thing that only one backend needs lives under that backend's
module, `modules/<name>/`; `common/`, `session/` and `platform/` hold only
what two or more backends, or both clients, share.

## The backends: `modules/<name>/`

One directory per backend, each a Gradle module (`:backends:<name>`, remapped
in `settings.gradle`): `rfb`, `libvnc`, `tigervnc`, `freerdp`, `ironrdp`,
`realvnc`, `spice`, `rustdesk`. In each:

- `catalog.rs` — the backend's entry in the catalog: its `About` (name,
  description, place in a picker, how its identities are pinned, whether its
  user name carries a domain) and its options table, with their tests.
  Compiled by `session` by path (`session/src/catalog.rs`), so that
  `vento-codegen` builds from `session` alone; Gradle's `codegenSources`
  lists every one.
- `build.gradle` — the Android module.
- `src/main/rust/` — the backend's crate: the protocol itself for the four in
  Rust (and its `probe` example, behind the `probe` feature), or the small
  adapter over the C library for libvnc, tigervnc and freerdp, whose
  `build.rs` reads the backend's header into Rust with bindgen. RealVNC has
  no crate; its Linux adapter is `linux/src/backends/realvnc.rs`, and
  `linux/build.rs` reads its header.
- `src/main/cpp/` — the C/C++ session over the upstream library, its CMake
  for both builds, the host probe's `main` (`-DVENTO_PROBES=ON`), and
  `include/vento/<name>.h`, the backend's own C API: its config, its
  constructor and its live calls. RealVNC's also holds `host/`, the hosting
  layer that runs their Android viewer core in a Linux process (built only
  on the host, only with clang).
- `src/main/java/`, `src/main/resources/` — the backend's provider and
  whatever else of it the phone needs in Java.

## What they share

- `common/` (`remotedesktop-common`) — what a protocol is written against:
  the session thread, handler and events, video and audio, and the neutral C
  API in `common/include/vento/` (`backend.h`, `video.h`, `audio.h`, `log.h`,
  `text.h`; each C backend's own header is in its module).
- `session/` (`remotedesktop-session`) — what a client holds a session by:
  the `Backend` trait, events, the catalog registry and the shape of an
  option, `Kind`/`Connect`, and the rules both clients keep alike
  (credentials, trust), held to `vectors/`. With the `c` feature, the one
  adapter over the C vtable and the shared headers' declarations
  (`session::c::ffi`), which each C backend's own declarations are written
  over.
- `tls/` — rustls pinning, for the Rust backends that speak TLS.
- `codegen/` (`vento-codegen`) — the phone's Java catalogs and natives
  classes, generated from `session` at build time on the host.
- `platform/android/` — the one JNI layer, MediaCodec and AAudio.
- `platform/desktop/` — FFmpeg (built statically from `third_party/`), the
  ALSA/PipeWire outputs and the desktop C platform (`c/`, with the C probes'
  shared body).
- `src/main/java/` — the `:backends` module's Java: the seam every backend's
  provider is written against.
- `vectors/` — test cases both the Rust and the Java tests read.

## Building them

- `build.gradle` — the `:backends` Gradle module and the helpers every
  backend module calls (`generateCatalog`, `generateNatives`,
  `rustStaticLibrary`, `codegenSources`).
- `buildSrc/` — the Gradle tasks those helpers use (cargo, codegen, tarballs,
  patch check), pulled in by the root `buildSrc`.
- `CMakeLists.txt`, `cmake/` — the host CMake build of the C backends, which
  `linux/build.rs` runs (`VENTO_SHARED=ON`) and which builds the C probes
  with `-DVENTO_PROBES=ON`; `cmake/vento_android.cmake` is the phone's half.
- `ports/` — the generated Android builds of openssl, nettle and gnutls.
- `UPGRADING.md` — how the upstreams behind the backends are moved, and what
  adding a backend touches.

The crates are members of the root `Cargo.toml`'s workspace; `linux/` is a
workspace of its own that reaches them by path.
