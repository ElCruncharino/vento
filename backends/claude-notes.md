# backends/

The eight remote desktop backends, what they share, and how they are built
for the phone. This is a map of where things are now; the layout is being
reorganised in stages, and each stage updates it.

**The rule:** a thing that only one backend needs lives under that backend's
module, `modules/<name>/`; `common/`, `session/` and `platform/` hold only
what two or more backends share. Inside a module, each language has one
place: the Rust crate at its root, the C in `cpp/`, the Java in `java/`, the
options in `catalog.rs`.

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
- `build.gradle` — the Android module, its source sets and its CMake
  pointed at the directories below rather than at AGP's `src/main/`.
- `Cargo.toml`, `src/`, `build.rs`, `examples/` — the backend's crate, at
  the module's root: the protocol itself for the four in Rust (and its
  `probe` example, behind the `probe` feature), or the small adapter over
  the C library for libvnc, tigervnc and freerdp, whose `build.rs` reads the
  backend's header into Rust with bindgen. RealVNC has no crate.
- `cpp/` (the four C backends) — the C/C++ session over the upstream
  library, its CMake, and `include/vento/<name>.h`, the backend's own C API:
  its config, its constructor and its live calls.
- `java/` — the backend's provider and whatever else of it the phone needs
  in Java: the `.java` files themselves, with no package directories over
  them (javac goes by each file's `package` line). `resources/` — its
  `META-INF/services` registration.
- `build/`, and `.cxx/` for a C backend — Gradle's and the NDK's CMake's
  outputs, untracked.

## What they share

- `common/` (`remotedesktop-common`) — what a protocol is written against:
  the session thread, handler and events, video and audio, and the neutral C
  API in `common/include/vento/` (`backend.h`, `video.h`, `audio.h`, `log.h`,
  `text.h`; each C backend's own header is in its module).
- `session/` (`remotedesktop-session`) — what a client holds a session by:
  the `Backend` trait, events, the catalog registry and the shape of an
  option, `Kind`/`Connect`, and the rules every client keeps alike
  (credentials, trust), held to `vectors/`. With the `c` feature, the one
  adapter over the C vtable and the shared headers' declarations
  (`session::c::ffi`), which each C backend's own declarations are written
  over.
- `tls/` — rustls pinning, for the Rust backends that speak TLS.
- `codegen/` (`vento-codegen`) — the phone's Java catalogs and natives
  classes, generated from `session` at build time on the host.
- `platform/android/` — the phone's platform: the one JNI layer, MediaCodec
  and AAudio (the crate), and, in `java/`, the `:backends` module's Java —
  the seam every backend's provider is written against, the files with no
  package directories over them — with its tests in `javatest/`; `c/`, the
  logcat logger and the export map a C backend's library is linked with;
  `cmake/vento_android.cmake`, a C backend's Android half
  (`vento_android_library`); and `ports/`, the generated Android builds of
  openssl, nettle and gnutls (`gen.sh` in each regenerates its `gen/`).
- `vectors/` — test cases both the Rust and the Java tests read.

## Building them

- `build.gradle` — the `:backends` Gradle module, whose sources it points
  at `platform/android/java/` and `javatest/`, and the helpers every backend
  module calls (`generateCatalog`, `generateNatives`, `rustStaticLibrary`,
  `codegenSources`).
- `buildSrc/` — the Gradle tasks those helpers use (cargo, codegen, tarballs,
  patch check), pulled in by the root `buildSrc`.
- `cmake/` — what a C backend's CMake includes, `warnings.cmake` and
  `reproducible.cmake`; the phone's half of a C backend is in
  `platform/android/cmake/`.
- `UPGRADING.md` — how the upstreams behind the backends are moved, and what
  adding a backend touches.

The crates are members of the root `Cargo.toml`'s workspace.
