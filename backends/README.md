# backends/

The eight remote desktop backends, what they share, and how they are built
for the phone and for the Linux client.

**The rule:** a thing that only one backend needs lives under that backend's
module, `modules/<name>/`; `common/` and `session/` hold only what two or
more backends, or both clients, share; and what one platform needs of
all of them lives under that platform, `platform/<os>/` — its crate, its C,
its CMake half and, for the phone, its Java and its ports. Inside a module,
each language has one place: the Rust crate at its root, the C in `cpp/`,
the Java in `java/`, the options in `catalog.rs`. What is left at the top is
what builds them all: the Gradle parent, its tasks, the host CMake root and
the CMake both builds include.

    backends/
      README.md, UPGRADING.md
      build.gradle      the :backends Gradle module, parent of the modules
      buildSrc/         the Gradle tasks its helpers use
      CMakeLists.txt    the host build of the C backends
      cmake/            warnings.cmake, reproducible.cmake
      common/  session/  codegen/  vectors/
      modules/<name>/   one per backend (eight)
      platform/
        android/        crate, java/, javatest/, cpp/, cmake/, ports/
        linux/          crate, cpp/, cmake/, probe/, tests/
      build/            the :backends module's Gradle output, untracked

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
  backend's header into Rust with bindgen. RealVNC has no crate; its Linux
  adapter is `linux/src/backends/realvnc.rs`, and `linux/build.rs` reads its
  header.
- `cpp/` (the four C backends) — the C/C++ session over the upstream
  library, its CMake for both builds, the host probe's `main`
  (`-DVENTO_PROBES=ON`), and `include/vento/<name>.h`, the backend's own C
  API: its config, its constructor and its live calls. RealVNC's also holds
  `host/`, the hosting layer that runs their Android viewer core in a Linux
  process (built only on the host, only with clang).
- `java/` — the backend's provider and whatever else of it the phone needs
  in Java: the `.java` files themselves, with no package directories over
  them (javac goes by each file's `package` line). `resources/` — its
  `META-INF/services` registration.
- `build/`, and `.cxx/` for a C backend — Gradle's and the NDK's CMake's
  outputs, untracked.

## What they share

- `common/` (`remotedesktop-common`) — what a protocol is written against:
  the session thread, handler and events, video and audio, the rustls
  pinning and shared stream of the Rust backends that speak TLS (`tls`), and
  the neutral C API in `common/include/vento/` (`backend.h`, `video.h`,
  `audio.h`, `log.h`, `text.h`; each C backend's own header is in its
  module).
- `session/` (`remotedesktop-session`) — what a client holds a session by:
  the `Backend` trait, events, the catalog registry and the shape of an
  option, `Kind`/`Connect`, and the rules both clients keep alike
  (credentials, trust), held to `vectors/`. With the `c` feature, the one
  adapter over the C vtable and the shared headers' declarations
  (`session::c::ffi`), which each C backend's own declarations are written
  over.
- `codegen/` (`vento-codegen`) — the phone's Java catalogs and natives
  classes, generated from `session` at build time on the host.
- `vectors/` — test cases both the Rust and the Java tests read.

## The platforms: `platform/<os>/`

- `platform/android/` (`remotedesktop-platform-android`) — the phone's
  platform: the one JNI layer, MediaCodec and AAudio (the crate), and, in
  `java/`, the `:backends` module's Java — the seam every backend's provider
  is written against, the files with no package directories over them — with
  its tests in `javatest/`; `cpp/`, the logcat logger and the export map a C
  backend's library is linked with; `cmake/vento_android.cmake`, a C
  backend's Android half (`vento_android_library`); and `ports/`, the
  generated Android builds of openssl, nettle and gnutls (`gen.sh` in each
  regenerates its `gen/`).
- `platform/linux/` (`remotedesktop-platform-linux`) — the Linux client's
  platform: FFmpeg, built statically from `third_party/` by `build.rs`, and
  the decoders and the PipeWire/ALSA outputs over it (the crate); `cpp/`, the
  C those decoders call and the stderr logger a C backend links;
  `cmake/vento_linux.cmake`, a C backend's host half (`vento_backend_library`,
  `vento_backend_probe`, `VENTO_SHARED`, `VENTO_PROBES`,
  `VENTO_FFMPEG_PREFIX`); `probe/`, the development tools — the C probes'
  shared body and the two sound rigs (`--features probe`); and `tests/`, the
  decoders' fixture tests.

## Building them

- `build.gradle` — the `:backends` Gradle module, whose sources it points
  at `platform/android/java/` and `javatest/`, and the helpers every backend
  module calls (`generateCatalog`, `generateNatives`, `rustStaticLibrary`,
  `codegenSources`).
- `buildSrc/` — the Gradle tasks those helpers use (cargo, codegen, tarballs,
  patch check), pulled in by the root `buildSrc`.
- `CMakeLists.txt` — the host CMake build of the C backends, which
  `linux/build.rs` runs (`VENTO_SHARED=ON`) and which builds the C probes
  with `-DVENTO_PROBES=ON`. Each platform's half of a C backend's CMake is
  that platform's (`platform/<os>/cmake/`); `cmake/` holds what both builds
  include, `warnings.cmake` and `reproducible.cmake`.
- `UPGRADING.md` — how the upstreams behind the backends are moved, and what
  adding a backend touches.

The crates are members of the root `Cargo.toml`'s workspace; `linux/` is a
workspace of its own that reaches them by path.
