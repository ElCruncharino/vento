// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The C backends, for the Rust binary: `backends/CMakeLists.txt` run with
//! `VENTO_SHARED` on. (The declarations of `vento/backend.h` are read into
//! Rust by `remotedesktop-api`'s own build script, under its `c`
//! feature, and each backend's own header by its crate's; the backends'
//! crates link nothing, so the libraries are made here.)
//!
//! This runs the host build the C probes are built on (with
//! `-DVENTO_PROBES=ON`) through the `cmake` crate rather than describing it
//! again, with the one switch that makes each
//! `vento_<name>` a shared library carrying its vendored static dependencies
//! and the stderr `vento_log`. The probes are left at their default, off:
//! nothing here runs them. The `.so` files land under `lib/` in the CMake
//! tree, and the binary links none of them: each backend's crate opens its
//! own by name the first time it is asked for (`api::c::library`), so that a
//! library that cannot be loaded makes that one backend not available
//! rather than stopping the binary from starting.
//!
//! **Where they are found: `$ORIGIN/lib`.** The installed layout is the
//! binary with a `lib/` directory beside it holding every `libvento_*.so`,
//! and the binary's rpath says exactly that, relative to wherever it is, so
//! the directory can be moved, unpacked anywhere, run from a USB stick.
//! The target directory is laid out the same way: the libraries are linked
//! (or copied) into `target/<profile>/lib/` beside the binary, so that `cargo
//! run` is already an installed tree, and `target/release/` is what
//! `linux/Dockerfile` copies one out of. A second entry, `$ORIGIN/../lib`, is
//! for the test harness, which cargo puts one directory down in `deps/`; in an
//! installed tree it is a directory that holds no `libvento_*.so`, and the
//! search goes on to the system's as it would anyway. No absolute path is
//! baked in, so no build machine path is in the binary or searched at run
//! time.
//!
//! RealVNC's hosting layer builds only under clang, so where `clang` is on the
//! path both compilers are set to it for the whole tree (the three open
//! backends build under either), and where it is not, the CMake root skips
//! that module with a message rather than an error. `remotedesktop-realvnc`
//! opens it as the others are opened, through the same rpath (a `dlopen` from
//! the binary searches the binary's), and says the backend is not available
//! where it is missing: whether this script makes the library is not known
//! until it runs, so it cannot be a feature of a crate built before then.
//!
//! That choice is made again whenever `PATH`, `CC` or `CXX` changes, and a
//! tree configured for other compilers is thrown away and configured afresh:
//! CMake keeps the compiler it was first configured with in its cache, and
//! the configure step is skipped while there is one, so installing clang
//! after a gcc build would otherwise never bring RealVNC in.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let backends = manifest
        .join("../backends")
        .canonicalize()
        .expect("backends/ beside linux/");
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());

    // The C build's inputs, as far as cargo watches them: the build files,
    // the GnuTLS and nettle pins (a bump reruns CMake, which fetches the new
    // release), the neutral headers, each module's host half and the Linux
    // platform's C and CMake half (not its `probe/`, which this build leaves
    // off). The vendored trees are not watched: they are submodules, and a
    // change there needs an explicit rebuild.
    for path in [
        "CMakeLists.txt",
        "crypto-sources.txt",
        "cmake",
        "api/include",
        "platform/linux/cpp",
        "platform/linux/cmake",
        "modules/libvnc/cpp",
        "modules/tigervnc/cpp",
        "modules/freerdp/cpp",
        "modules/realvnc/cpp",
    ] {
        println!("cargo:rerun-if-changed={}", backends.join(path).display());
    }
    // The inputs that decide the compilers (see `compilers`). Printing any
    // rerun line means cargo watches only what is printed, so these have to
    // be listed.
    for var in ["PATH", "CC", "CXX"] {
        println!("cargo:rerun-if-env-changed={var}");
    }
    // The static FFmpeg the Linux platform crate built (its build script,
    // through its `links` key), which FreeRDP links for its scaling as the
    // Rust decoder does, so that the client has one FFmpeg and it is not the
    // system's.
    println!("cargo:rerun-if-env-changed=DEP_VENTO_VIDEO_PREFIX");
    let ffmpeg = PathBuf::from(
        env::var("DEP_VENTO_VIDEO_PREFIX")
            .expect("remotedesktop-platform-linux says where its FFmpeg is"),
    );

    let lib = build_c_backends(&backends, &out, &ffmpeg);
    // The rpath (see the module's comment). Passed to the linker as written,
    // with no shell between, so `$ORIGIN` reaches the binary as the loader
    // wants it.
    println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN/lib:$ORIGIN/../lib");
    // The three open backends' libraries are made by every build: the binary
    // runs without one, but a build that lost one is broken.
    for name in ["libvnc", "tigervnc", "freerdp"] {
        let path = lib.join(format!("libvento_{name}.so"));
        assert!(path.is_file(), "the C build did not produce {}", path.display());
    }
    beside_the_binary(&lib, &out);
    // Whether RealVNC's library was made, for the binary (`backends::all`),
    // which then reports it as not built rather than as not found by the
    // loader.
    let realvnc = lib.join("libvento_realvnc.so").is_file();
    println!("cargo:rustc-env=VENTO_REALVNC_BUILT={}", u8::from(realvnc));
    if realvnc {
        println!("cargo:warning=RealVNC backend: in (built with clang)");
    } else {
        println!("cargo:warning=RealVNC backend: not available (its library needs clang on the path)");
    }
}

/// Runs the host CMake build with the shared switch on and returns the
/// directory the libraries were written to.
fn build_c_backends(backends: &Path, out: &Path, ffmpeg: &Path) -> PathBuf {
    let clang = compilers();
    reconfigure_if_changed(out, &clang, ffmpeg);
    let mut cfg = cmake::Config::new(backends);
    cfg.define("VENTO_SHARED", "ON")
        .define("VENTO_FFMPEG_PREFIX", ffmpeg)
        // The CMake root's own default: -O0 is too slow for a probe or a
        // decoder.
        .profile("RelWithDebInfo")
        // The `cmake` crate would otherwise pass the `cc` crate's flags; the
        // tree has its own, and FreeRDP bakes CMAKE_C_FLAGS
        // into a build string, so the host build is kept the same as a plain
        // `cmake -S backends`.
        .define("CMAKE_C_FLAGS", "")
        .define("CMAKE_CXX_FLAGS", "")
        // Everything, not an install: the tree neuters `install()` for the
        // vendored projects, and the libraries are found where they are built.
        .build_target("all")
        // The configure step of three vendored trees is slow, and ninja
        // re-runs it on its own when a CMake file changes; a change of
        // compilers, which it does not see, is `reconfigure_if_changed`'s.
        .always_configure(false);
    if on_path("ninja") {
        cfg.generator("Ninja");
    }
    if let Some((cc, cxx)) = &clang {
        cfg.define("CMAKE_C_COMPILER", cc)
            .define("CMAKE_CXX_COMPILER", cxx);
    }
    let dst = cfg.build();
    // `build()` returns the install prefix; the build tree is `build/` under
    // it, and `lib/` is where vento_linux.cmake sends the libraries.
    dst.join("build").join("lib")
}

/// Puts the libraries where the binary's rpath looks: `lib/` in the
/// profile's directory of the target directory (`target/debug/lib/`), which
/// `OUT_DIR` is three levels under (`target/debug/build/vento-…/out`), as it
/// is with `--target` (`target/<triple>/debug/build/…`). Hard links, which
/// cost nothing, where the two are on one file system; copies where not.
/// Made afresh on every run of this script, which is every build of the
/// libraries. A library this build no longer makes (RealVNC's, after a switch
/// away from clang) is removed from there too, or the binary would open what
/// an earlier build left.
fn beside_the_binary(lib: &Path, out: &Path) {
    let profile = out.ancestors().nth(3).expect("OUT_DIR is target/<profile>/build/<crate>/out");
    let dest = profile.join("lib");
    fs::create_dir_all(&dest).expect("creating lib/ beside the binary");
    for name in ["libvnc", "tigervnc", "freerdp", "realvnc"] {
        let file = format!("libvento_{name}.so");
        let (from, to) = (lib.join(&file), dest.join(&file));
        match fs::remove_file(&to) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => panic!("removing the old {}: {e}", to.display()),
        }
        if from.is_file() && fs::hard_link(&from, &to).is_err() {
            fs::copy(&from, &to).unwrap_or_else(|e| panic!("copying {} to {}: {e}", from.display(), to.display()));
        }
    }
}

/// clang and clang++ where both are on the path and run, as the paths `PATH`
/// finds them at; `None` for whatever the `cmake` crate picks otherwise
/// (`CC` and `CXX`, or `cc` and `c++`).
fn compilers() -> Option<(PathBuf, PathBuf)> {
    let found = |program: &str| {
        on_path(program).then(|| which(program).unwrap_or_else(|| PathBuf::from(program)))
    };
    Some((found("clang")?, found("clang++")?))
}

/// Deletes the CMake tree under `out` when it was configured for other
/// compilers than these, or another FFmpeg, so that the `cmake` crate, which
/// only configures a tree that has no cache, configures it again. Every
/// object in it would be rebuilt anyway, and a stale `libvento_realvnc.so`
/// from a clang tree must not outlive a switch away from clang. (The FFmpeg's
/// prefix is the platform crate's `OUT_DIR`, which moves only when that
/// crate's build does.)
///
/// The tree's configuration is recorded in a line written beside it. A tree
/// without that line is judged by its cache's C compiler instead (clang or
/// not, which is the decision that matters).
fn reconfigure_if_changed(out: &Path, clang: &Option<(PathBuf, PathBuf)>, ffmpeg: &Path) {
    let var = |name: &str| env::var(name).unwrap_or_default();
    let compilers = match clang {
        Some((cc, cxx)) => format!("clang {} {}", cc.display(), cxx.display()),
        None => format!(
            "default CC={} CXX={} cc={} c++={}",
            var("CC"),
            var("CXX"),
            which("cc").unwrap_or_default().display(),
            which("c++").unwrap_or_default().display()
        ),
    };
    let want = format!("{compilers}\nffmpeg {}", ffmpeg.display());
    let tree = out.join("build");
    let stamp = out.join("vento-compilers");
    let cache = fs::read_to_string(tree.join("CMakeCache.txt")).ok();
    let stale = match (fs::read_to_string(&stamp).ok(), &cache) {
        (Some(had), Some(_)) => had.trim() != want.trim(),
        (None, Some(cache)) => {
            let was_clang = cache
                .lines()
                .find_map(|l| l.strip_prefix("CMAKE_C_COMPILER:"))
                .and_then(|l| l.split_once('='))
                .is_some_and(|(_, path)| {
                    Path::new(path)
                        .file_name()
                        .is_some_and(|f| f.to_string_lossy().contains("clang"))
                });
            was_clang != clang.is_some()
        }
        (_, None) => false,
    };
    if stale {
        println!(
            "cargo:warning=C backends: the compilers or the FFmpeg changed, configuring afresh for {want}"
        );
        fs::remove_dir_all(&tree).expect("deleting the stale CMake tree");
    }
    fs::write(&stamp, format!("{want}\n")).expect("writing the compilers stamp");
}

fn on_path(program: &str) -> bool {
    Command::new(program)
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Where `PATH` finds `program`, as a shell would.
fn which(program: &str) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    env::split_paths(&env::var_os("PATH")?)
        .map(|dir| dir.join(program))
        .find(|p| {
            p.metadata()
                .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        })
}
