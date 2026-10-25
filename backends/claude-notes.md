# backends/

The eight remote desktop backends, what they share, and how they are built
for the phone.

**The rule:** a thing that only one backend needs lives under that backend's
module, `modules/<name>/`; `runtime/` and `api/` hold only what two or more
backends share — `runtime/` what a protocol client is written against and a
platform is written to, its picture and sound (`runtime::media`) included,
and `api/` what a client holds a session by; and what one platform needs of
all of them lives under that platform, `platform/<os>/` — its crate, its C,
its CMake half and, for the phone, its Java and its ports. Inside a module,
each language has one place: the Rust crate at its root, the C in `cpp/`, the
Java in `java/`, the options in `catalog.rs`. What is left at the top is what
builds them all: the Gradle parent, its tasks and the CMake every C backend
includes.

    backends/
      claude-notes.md, UPGRADING.md
      build.gradle      the :backends Gradle module, parent of the modules
      buildSrc/         the Gradle tasks its helpers use
      cmake/            warnings.cmake, reproducible.cmake
      runtime/          what a protocol and a platform are written to,
                        src/media/ too
      api/              what a client holds a session by
      codegen/  vectors/
      modules/<name>/   one per backend (eight)
      platform/
        android/        crate, java/, javatest/, cpp/, cmake/, ports/
      build/            the :backends module's Gradle output, untracked

## The backends: `modules/<name>/`

One directory per backend, each a Gradle module (`:backends:<name>`, remapped
in `settings.gradle`): `rfb`, `libvnc`, `tigervnc`, `freerdp`, `ironrdp`,
`realvnc`, `spice`, `rustdesk`. In each:

- `catalog.rs` — the backend's entry in the catalog: its `About` (name,
  description, place in a picker, how its identities are pinned, whether its
  user name carries a domain) and its options table, with their tests.
  Compiled by `api` by path (`api/src/catalog.rs`), so that
  `vento-codegen` builds from `api` alone; Gradle's `codegenSources`
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

- `runtime/` (`remotedesktop-runtime`) — what a protocol client is written
  against and a platform is written to: the `Client` it implements, the
  `Handler` its thread calls and the `Callbacks` a platform answers with, the
  `Runner` that is the session's thread and the `Bridge` between them, the
  address parser, the dial a hang-up interrupts, and the rustls pinning and shared stream
  of the Rust backends that speak TLS (`tls`); and `media`, what a protocol
  hands its picture and sound to: video (the decoders a platform supplies, and
  what is done with their planes) and audio (the outputs a platform supplies,
  their decoders, and the queue a device drains). `media` uses nothing of the
  rest of the crate, nor the rest of it `media`; a platform crate depends on
  `runtime/` for the traits it implements.
- `api/` (`remotedesktop-api`) — what a client holds a session by:
  the `Backend` trait, events, the catalog registry and the shape of an
  option, `Kind`/`Connect`, and the rules every client keeps alike
  (credentials, trust), held to their cases (`tests/vectors.rs`). With the
  `c` feature, the one adapter over the C vtable and the shared headers'
  declarations (`api::c::ffi`), which each C backend's own declarations are
  written over. The neutral C API it adapts is in `api/include/vento/`
  (`backend.h`, `video.h`, `audio.h`, `log.h`, `text.h`; each C backend's own
  header is in its module): with the crate that reads them into Rust, as each
  backend's header is with its own.
- `codegen/` (`vento-codegen`) — the phone's Java catalogs and natives
  classes, generated from `api` at build time on the host.
- `vectors/` — `options.json`, the cases of the one rule with a Java copy
  (`settle`), which both the Rust and the Java tests read. Cases only the
  Rust tests read are written in them.

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

## Building them

- `build.gradle` — the `:backends` Gradle module, whose sources it points
  at `platform/android/java/` and `javatest/`, and the helpers every backend
  module calls (`generateCatalog`, `generateNatives`, `rustStaticLibrary`,
  `codegenSources`).
- `buildSrc/` — the Gradle tasks those helpers use (cargo, codegen, tarballs,
  patch check), pulled in by the root `buildSrc`.
- `cmake/` — what a C backend's CMake includes, `warnings.cmake` and
  `reproducible.cmake`. Each platform's half of a C backend's CMake is that
  platform's (`platform/<os>/cmake/`).
- `UPGRADING.md` — how the upstreams behind the backends are moved, and what
  adding a backend touches.

The crates are members of the root `Cargo.toml`'s workspace.
