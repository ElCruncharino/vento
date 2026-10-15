// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The C backends, for the Rust binary: `backends/CMakeLists.txt` run with
//! `VENTO_SHARED` on. (The declarations of `vento/backend.h` are read into
//! Rust by `remotedesktop-session`'s own build script, under its `c`
//! feature; the backends' crates link nothing, so the libraries are linked
//! here, into the binary.)
//!
//! The host build already existed — it is what the C probes are built on —
//! and this runs the same file through the `cmake` crate rather than
//! describing it again, with the one switch that makes each `vento_<name>` a
//! shared library carrying its vendored static dependencies and the stderr
//! `vento_log`. The four `.so` files land under `lib/` in the build tree, the
//! binary links them by name and nothing else of theirs, and an rpath to that
//! directory is baked in so that `cargo run` finds them from the target
//! directory without being told.
//!
//! RealVNC's hosting layer builds only under clang, so where `clang` is on the
//! path both compilers are set to it for the whole tree — the three open
//! backends build under either — and where it is not, the CMake root skips
//! that module with a message rather than an error. Whether the library then
//! exists is what the `realvnc` cfg says, so the Rust side can leave the
//! backend out of its list rather than fail to link.
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
    // Known to rustc whether or not it is set, so that `cfg(realvnc)` in the
    // sources is never an unexpected-cfg warning.
    println!("cargo:rustc-check-cfg=cfg(realvnc)");

    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let backends = manifest
        .join("../backends")
        .canonicalize()
        .expect("backends/ beside linux/");
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());

    // What the C build is made of, as far as cargo is asked to watch: the
    // build files, the neutral headers, each module's host half and the
    // desktop platform sources. The vendored trees are not watched — they
    // are submodules, and a change there is a rebuild somebody asks for.
    for path in [
        "CMakeLists.txt",
        "cmake",
        "common/include",
        "platform/desktop/c",
        "platform/desktop/realvnc",
        "platform/desktop/tigervnc",
        "modules/libvnc/src/main/cpp",
        "modules/tigervnc/src/main/cpp",
        "modules/freerdp/src/main/cpp",
        "modules/realvnc/src/main/cpp",
    ] {
        println!("cargo:rerun-if-changed={}", backends.join(path).display());
    }
    // What decides the compilers (see `compilers`). Printing any rerun line
    // means cargo watches only what is printed, so these have to be said.
    for var in ["PATH", "CC", "CXX"] {
        println!("cargo:rerun-if-env-changed={var}");
    }

    let lib = build_c_backends(&backends, &out);
    println!("cargo:rustc-link-search=native={}", lib.display());
    // The rpath, so that the binary and the tests run from where cargo puts
    // them. Absolute, which is right for a development tree; a packaged
    // binary would carry the libraries beside it and `$ORIGIN` instead.
    println!("cargo:rustc-link-arg=-Wl,-rpath,{}", lib.display());
    for name in ["libvnc", "tigervnc", "freerdp"] {
        let path = lib.join(format!("libvento_{name}.so"));
        assert!(path.is_file(), "the C build did not produce {}", path.display());
        println!("cargo:rustc-link-lib=dylib=vento_{name}");
    }
    if lib.join("libvento_realvnc.so").is_file() {
        println!("cargo:rustc-link-lib=dylib=vento_realvnc");
        println!("cargo:rustc-cfg=realvnc");
        println!("cargo:warning=RealVNC backend: in (built with clang)");
    } else {
        println!("cargo:warning=RealVNC backend: left out (needs clang on the path)");
    }
}

/// Runs the host CMake build with the shared switch on and returns the
/// directory the libraries were written to.
fn build_c_backends(backends: &Path, out: &Path) -> PathBuf {
    let clang = compilers();
    reconfigure_if_changed(out, &clang);
    let mut cfg = cmake::Config::new(backends);
    cfg.define("VENTO_SHARED", "ON")
        // The CMake root's own default: a probe at -O0 is not what anybody
        // wants to watch, and neither is a decoder.
        .profile("RelWithDebInfo")
        // The `cmake` crate would otherwise hand over the `cc` crate's idea
        // of flags; the tree has its own, and FreeRDP bakes CMAKE_C_FLAGS
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
    // it, and `lib/` is where vento_desktop.cmake sends the libraries.
    dst.join("build").join("lib")
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

/// The CMake tree under `out` deleted when it was configured for other
/// compilers than these, so that the `cmake` crate, which only configures a
/// tree that has no cache, configures it again. Every object in it would be
/// rebuilt anyway, and a stale `libvento_realvnc.so` from a clang tree must
/// not outlive a switch away from clang.
///
/// What the tree was configured for is a line written beside it. A tree from
/// before that line existed is judged by its cache's C compiler instead —
/// clang or not, which is the decision that matters.
fn reconfigure_if_changed(out: &Path, clang: &Option<(PathBuf, PathBuf)>) {
    let var = |name: &str| env::var(name).unwrap_or_default();
    let want = match clang {
        Some((cc, cxx)) => format!("clang {} {}", cc.display(), cxx.display()),
        None => format!(
            "default CC={} CXX={} cc={} c++={}",
            var("CC"),
            var("CXX"),
            which("cc").unwrap_or_default().display(),
            which("c++").unwrap_or_default().display()
        ),
    };
    let tree = out.join("build");
    let stamp = out.join("vento-compilers");
    let cache = fs::read_to_string(tree.join("CMakeCache.txt")).ok();
    let stale = match (fs::read_to_string(&stamp).ok(), &cache) {
        (Some(had), Some(_)) => had.trim() != want,
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
        println!("cargo:warning=C backends: the compilers changed, configuring afresh for {want}");
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
