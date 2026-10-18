# Vento VNC/RDP Client

Fast, fluid VNC/RDP remote desktop client for Android.

It has the best touchpad-style controls of any remote desktop app I am currently aware of, and it supports multiple client backends.

### Features

- Intuitive touchpad-style controls (see [here](./control/README.md) for more info).
- Accuracy assists (improved precision, axis locking, adaptive acceleration).
- Material Design 3.
- Multi-window support.
- Physical keyboard/mouse support.
- Workarounds for various Android IME bugs.
- Hardware-accelerated video where supported (H.264 with FreeRDP and TigerVNC, H.264/H.265/VP8/VP9/AV1 with RustDesk).
- Multiple client backends for VNC (TigerVNC, LibVNC, Rust) and RDP (IronRDP, FreeRDP), plus SPICE.
- Optional RealVNC backend (separate APK, fetches the library at runtime).
- Optional RustDesk backend (separate APK), reaching a machine by ID through a rendezvous server or by address.

[**`Download`**](https://github.com/pgaskin/vento/releases/latest) [**`Website`**](https://pgaskin.net/vento/)

### Vibe-coding

This is mostly vibe-coded, but I've tested it extensively and worked alongside it, reading most of the thinking output (and intervening where required, especially where it started making incorrect assumptions or writing unmaintainable code), and I had a clear vision of what I wanted the final result to look like.

I wouldn't have made this app otherwise (even though I've been wanting to for years) since it's just too much work, especially for all the native bindings and the gesture handling.

Even with Claude, the initial version took ~60 hours of work over ~10 days, and that's not including all the stuff I reused from my existing projects (almost all written without LLMs), for example:

- [vncpatch](https://github.com/pgaskin/vncpatch): fixes, patches, and notes for the realvnc app
- [lithiumpatch](https://github.com/pgaskin/lithiumpatch): apk fetch stuff and workflow
- [xwlrvnc](https://github.com/pgaskin/xwlrvnc): realvnc reverse-engineering work
- [cmus-android](https://github.com/pgaskin/cmus-android): c/c++ native build/patch logic and gradle integration, reproducibility work
- [windy](https://github.com/pgaskin/windy): rust native build logic and gradle integration, build system, reproducibility work, app architecture
- [asslcapture](https://github.com/pgaskin/asslcapture): ndk stuff
- various test harnesses and reversing tooling I've written over the years for myself
- various experimental pieces of ideas for this app I've written over the years

Claude was very useful for playing with the gesture detection code, as it was able to quickly test various ideas I had without me needing to think about the state machines and math too much for the initial prototype. See [here](./control/README.md) for more about this.

Claude was also very helpful for exhaustively testing things like keyboard input, catching quite a few bugs in the process. Without Claude, I wouldn't have bothered to write and run individual test scripts for all combinations of platforms (Windows, Linux, etc), server implementations (RealVNC, Microsoft Remote Desktop, IronRDP, FreeRDP, TightVNC, TigerVNC, NeatVNC, UltraVNC, LibVNC), and client implementations (IronRDP, FreeRDP, LibVNC, TigerVNC, RealVNC, etc). Over a day of unattended running, it wrote more than 90k lines of throwaway test scripts and ran them on multiple devices against multiple VMs I set up and its own containers.

This README (and the ones in the other folders) is entirely hand-written by me. Almost all user-facing text in the app is also hand-written.

I still need to refactor the code and clean up the comments in the app.

### Development

Only Linux x86_64 is supported for building the app due to the native builds and shell scripts.

JDK 21 is required. Use `JAVA_HOME` to point to it if it isn't the default.

The `sdk.dir` option in `local.properties` must point to your Android SDK installation, which must have `ndk;28.2.13676358`, `build-tools;36.0.0`, and `cmake;3.30.5`.

The NDK should be set in `local.properties` and `ANDROID_NDK_HOME`.

```bash
# fetch and patch dependencies
git submodule update --init
./patch.sh -n
./gradlew :backends:fetchCryptoSources

# build apk
./gradlew :app:assembleDebug

# build the non-free realvnc backend plugin
./gradlew :plugins:realvnc:assembleDebug

# build the rustdesk backend plugin
./gradlew :plugins:rustdesk:assembleDebug
```

Backend plugins must be signed with the same key (enforced since the main app loads code from it) and built from the same version (the API isn't stable).

The C backends also build on the Linux host, with a probe per backend (`-DVENTO_PROBES=ON`; off by default, so the Linux client's build of the same tree has none) that connects, prints every callback and writes the framebuffer out as a PPM. The three open ones (libvncclient, TigerVNC, FreeRDP) build against the system libraries instead of the ports and vendored trees the phone needs: cmake, ninja, pkg-config, a C/C++ compiler, and the development packages for openssl, gnutls, nettle (hogweed, gmp), libjpeg-turbo, pixman, zlib and FFmpeg (libswscale and libavutil, for FreeRDP's scaling). `backends/CMakeLists.txt` says what each switch does. The Linux client (`linux/`) needs no FFmpeg of the system's: it builds FFmpeg and dav1d from `third_party/` statically and links the C backends against that (`linux/README.md`).

The fourth, RealVNC, runs their proprietary Android viewer core in the host process (`backends/modules/realvnc/cpp/host`, on mcpelauncher-linker, libc-shim and libjnivm, which are host-only submodules). It needs clang — libc-shim does not build under GCC, and the root skips the module with a message under anything but clang — and the submodules initialised with `--recursive`. The library itself is RealVNC's and is not in this repository or any build of it: take `libvncviewer.so` and `libc++_shared.so` for your machine's ABI out of the viewer APK of the build `realvnc/.../Library.java` pins, and the probe refuses any other build by SHA-256. What the core persists (its RSA key, accepted identities, logs) goes under `--state-dir`, as plain files where the phone has encrypted ones.

```bash
# build the host libraries and probes (-DSTRICT_WARNINGS=ON makes our warnings errors)
git submodule update --init --recursive
CC=clang CXX=clang++ cmake -S backends -B build/scratch/host -G Ninja -DVENTO_PROBES=ON
cmake --build build/scratch/host

# the RealVNC libraries, out of the viewer APK (x86_64 for a PC, arm64-v8a for an arm64 host)
mkdir -p build/scratch/realvnc-libs && unzip -j -o com.realvnc.viewer.android_4.9.3.60175.apk \
    'lib/x86_64/libvncviewer.so' 'lib/x86_64/libc++_shared.so' -d build/scratch/realvnc-libs

# point them at the test desktops under scripts/ (each probe prints its usage without arguments)
scripts/testvnc/run.sh && scripts/testrdp/run.sh
build/scratch/host/libvnc/vento_libvnc_probe 127.0.0.1:1 --password vncpass --out $PWD/build/scratch/libvnc.ppm
build/scratch/host/tigervnc/vento_tigervnc_probe 127.0.0.1:1 --password vncpass --click 500,300 --key 0x61 --clipboard hello --out $PWD/build/scratch/tigervnc.ppm
build/scratch/host/freerdp/vento_freerdp_probe 127.0.0.1 --trust --user proto --password protopass --config-dir $PWD/build/scratch/freerdp-home --out $PWD/build/scratch/freerdp.ppm
build/scratch/host/realvnc/vento_realvnc_probe 127.0.0.1::5901 --libs $PWD/build/scratch/realvnc-libs --state-dir $PWD/build/scratch/realvnc-state --trust --password vncpass --click 500,300 --key 0x61 --clipboard hello --out $PWD/build/scratch/realvnc.ppm
```

When updating the third_party dependencies, you'll need to update the unpatched gitlink, regenerate the patches and generated code, and commit the result.

```bash
# apply/refresh the third_party patches
./patch.sh

# regenerate code (needs autotools, perl, make, host toolchain, ndk)
./backends/ports/nettle/gen.sh
./backends/ports/gnutls/gen.sh
./backends/ports/openssl/gen.sh
```

The builds are designed to be reproducible (this is why I commit generated code). There are some blobs (mostly images plus some testdata) in the deps, but none of these are required for the build:

```gitignore
third_party/freerdp/client/Android/
third_party/freerdp/client/iOS/
third_party/freerdp/client/Mac/
third_party/freerdp/client/SDL/
third_party/freerdp/client/Windows/
third_party/freerdp/docs/
third_party/freerdp/libfreerdp/codec/test/
third_party/freerdp/resources/
third_party/freerdp/server/
third_party/freerdp/winpr/libwinpr/utils/test/
third_party/libjpeg-turbo/doc/
third_party/libjpeg-turbo/testimages/
third_party/openssl/apps/
third_party/openssl/doc/
third_party/openssl/test/
third_party/pixman/demos/
third_party/ryll/shakenfist-spice-compression/tests/
third_party/tigervnc/java/
third_party/tigervnc/media/
third_party/tigervnc/tests/
third_party/tigervnc/vncviewer/
third_party/tigervnc/win/
```

### Naming

I was originally just going to call this "Remote Desktop", but the name doesn't fit nicely, and it wouldn't be searchable. I ended up asking ChatGPT and Claude to come up with a list of name ideas given the list of features, and I liked "Vento" the best. It's Italian for wind, which makes sense since the main reason I created this was the lack of fluidity in the controls for other remote desktop apps. The name also kind of makes me think of VNC, and is unique, which is also nice.
