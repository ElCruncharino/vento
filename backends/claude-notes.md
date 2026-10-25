# backends/

The eight remote desktop backends, what they share, and how they are built
for the phone. This is a map of where things are now; the layout is being
reorganised in stages, and each stage updates it.

**The rule:** a thing that only one backend needs lives under that backend's
module, `modules/<name>/`; `common/`, `session/` and `platform/` hold only
what two or more backends share.

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
  adapter over the C library for libvnc, tigervnc and freerdp. RealVNC has no
  crate.
- `src/main/cpp/` — the C/C++ session over the upstream library and its
  CMake.
- `src/main/java/`, `src/main/resources/` — the backend's provider and
  whatever else of it the phone needs in Java.

## What they share

- `common/` (`remotedesktop-common`) — what a protocol is written against:
  the session thread, handler and events, video and audio, and the neutral C
  API in `common/include/vento/`.
- `session/` (`remotedesktop-session`) — what a client holds a session by:
  the `Backend` trait, events, the catalog registry and the shape of an
  option, `Kind`/`Connect`, and the rules every client keeps alike
  (credentials, trust), held to `vectors/`.
- `tls/` — rustls pinning, for the Rust backends that speak TLS.
- `codegen/` (`vento-codegen`) — the phone's Java catalogs and natives
  classes, generated from `session` at build time on the host.
- `platform/android/` — the one JNI layer, MediaCodec and AAudio.
- `src/main/java/` — the `:backends` module's Java: the seam every backend's
  provider is written against.
- `vectors/` — test cases both the Rust and the Java tests read.

## Building them

- `build.gradle` — the `:backends` Gradle module and the helpers every
  backend module calls (`generateCatalog`, `generateNatives`,
  `rustStaticLibrary`, `codegenSources`).
- `buildSrc/` — the Gradle tasks those helpers use (cargo, codegen, tarballs,
  patch check), pulled in by the root `buildSrc`.
- `cmake/` — what a C backend's CMake includes; `cmake/vento_android.cmake`
  is the phone's half.
- `ports/` — the generated Android builds of openssl, nettle and gnutls.
- `UPGRADING.md` — how the upstreams behind the backends are moved, and what
  adding a backend touches.

The crates are members of the root `Cargo.toml`'s workspace.
