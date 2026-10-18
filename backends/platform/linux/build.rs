// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The desktop's FFmpeg: dav1d and FFmpeg built from their submodules
//! (`third_party/dav1d`, `third_party/ffmpeg`) into `OUT_DIR` as static
//! libraries, and `cpp/vento_video.c` compiled over them, so that the Linux
//! client links no FFmpeg of the system's and needs none of its development
//! files.
//!
//! **One FFmpeg for every desktop consumer.** The Rust decoders here are one
//! (the video's, and the sound's AAC and Opus); TigerVNC's libavcodec H.264
//! context and FreeRDP's scaling are the others, built by `linux/build.rs`
//! through CMake. That build is told
//! where this one put its libraries through the `links` metadata
//! (`DEP_VENTO_VIDEO_PREFIX`), so all of them link the same configuration of
//! the same source. FFmpeg is configured with `--disable-everything` and then
//! exactly what those consumers use (`DECODERS`, `HWACCELS`, the two scaling
//! libraries), with `--disable-autodetect` so that nothing the build machine
//! happens to have installed is linked in, and without `--enable-gpl`: what is
//! built is LGPL 2.1 or later, and dav1d is BSD-2-Clause.
//!
//! **Built once.** The two builds take about a minute and a half together
//! (dav1d with Meson and Ninja, FFmpeg with its configure and make), and are
//! made again only when what they are made from changes — any of the three
//! submodules' commits, the configure line, the compiler or whether NASM is
//! there — which a stamp beside them records. Any other reason this script runs (a change to
//! the wrapper, a new `PATH`) finds the stamp the same and only recompiles the
//! wrapper. A debug and a release build each have an `OUT_DIR` and so a copy
//! of their own; FFmpeg is optimised in both, as its configure does by
//! default.
//!
//! **Hardware decoders, from headers alone.** FFmpeg's VAAPI decoders
//! (`HWACCELS`) are built against `third_party/libva`'s headers, installed
//! into the prefix with the one libva generates, through pkg-config files of
//! ours that say so. Nothing of libva is linked: its functions are
//! `cpp/va_shim.c`'s, which loads the real ones when hardware decoding is first
//! asked for. So the binary runs where libva is not installed, and does not
//! load it while hardware decoding is off.
//!
//! **Assembly** is NASM's on x86: without it dav1d and libavcodec fall back to
//! their C, which works and decodes a fraction as fast, so its absence is a
//! warning rather than an error. Installing it later builds them again with it
//! at the next build, without a `cargo clean`.
//!
//! Empty on Android, as the crate is: the phone's decoder is MediaCodec.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The decoders, and who decodes with each. Each is checked by name in
/// libavcodec's configure, so a misspelling is an error rather than a decoder
/// quietly missing.
const DECODERS: &[&str] = &[
    // RustDesk (the Rust decoder), TigerVNC's H.264 encoding, FreeRDP's
    // graphics pipeline (AVC420 and AVC444).
    "h264",
    // RustDesk.
    "hevc",
    "vp8",
    "vp9",
    // RustDesk's AV1, through dav1d in software; libavcodec's own `av1`
    // decodes only with hardware acceleration, and is there for that alone.
    "libdav1d",
    "av1",
    // The sound (`cpp/vento_ffaudio.c`): the AAC an RDP server sends, which
    // Windows does whenever it is offered, and the Opus RDP and RustDesk can
    // send. libavcodec's Opus decoder resamples its SILK half with
    // libswresample, which is why that is built.
    "aac",
    "opus",
];

/// The hardware decoders of the codecs above, for the Rust decoder when
/// hardware decoding is asked for (`cpp/vento_video.c`), which TigerVNC's and
/// FreeRDP's H.264 go through as well: VAAPI's for each. libva is no library at build time: it is reached
/// through `cpp/va_shim.c`, so it is headers here (`third_party/libva`).
const HWACCELS: &[&str] = &[
    "h264_vaapi",
    "hevc_vaapi",
    "vp8_vaapi",
    "vp9_vaapi",
    "av1_vaapi",
];

fn main() {
    println!("cargo:rerun-if-changed=cpp/vento_video.c");
    println!("cargo:rerun-if-changed=cpp/vento_video.h");
    println!("cargo:rerun-if-changed=cpp/vento_ffaudio.c");
    println!("cargo:rerun-if-changed=cpp/vento_ffaudio.h");
    println!("cargo:rerun-if-changed=cpp/va_shim.c");
    println!("cargo:rerun-if-changed=cpp/va_shim.h");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("android") {
        return;
    }
    for var in ["CC", "PATH"] {
        println!("cargo:rerun-if-env-changed={var}");
    }

    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let third_party = manifest
        .join("../../../third_party")
        .canonicalize()
        .expect("third_party/ at the repository root");
    let dav1d = third_party.join("dav1d");
    let ffmpeg = third_party.join("ffmpeg");
    let libva = third_party.join("libva");
    for (source, marker) in [
        (&dav1d, "meson.build"),
        (&ffmpeg, "configure"),
        (&libva, "va/va.h"),
    ] {
        assert!(
            source.join(marker).is_file(),
            "{} is empty; run: git submodule update --init third_party/dav1d third_party/ffmpeg \
             third_party/libva",
            source.display()
        );
        if let Some(head) = git_head(source) {
            println!("cargo:rerun-if-changed={}", head.display());
        }
    }

    let compiler = cc::Build::new().get_compiler();
    let cc = compiler.path().to_path_buf();
    let x86 = matches!(
        env::var("CARGO_CFG_TARGET_ARCH").as_deref(),
        Ok("x86" | "x86_64")
    );
    let asm = !x86 || runs("nasm", &["-v"]);
    if !asm {
        println!(
            "cargo:warning=FFmpeg: no nasm on the path, so dav1d and libavcodec are built without \
             their x86 assembly and decode several times slower"
        );
        // Installing NASM changes no variable: it puts a file into a
        // directory already on the path, which `rerun-if-env-changed=PATH`
        // does not see, and the build would keep its slow FFmpeg until a
        // `cargo clean`. So while it is missing, each directory on the path
        // is watched as well. Its arrival changes one of them, this script
        // runs again, finds NASM and so the stamp's `asm false` stale, and
        // builds FFmpeg again with the assembly. Anything else installed
        // there in the meantime runs it too, which costs only the wrapper's
        // recompile; with NASM present nothing is watched.
        if let Some(path) = env::var_os("PATH") {
            for dir in env::split_paths(&path).filter(|dir| dir.is_dir()) {
                println!("cargo:rerun-if-changed={}", dir.display());
            }
        }
    }

    let root = PathBuf::from(env::var("OUT_DIR").unwrap()).join("ffmpeg");
    let prefix = root.join("prefix");
    let configure = configure_args(&prefix, &cc, asm);
    let stamp = format!(
        "dav1d {}\nffmpeg {}\nlibva {}\ncc {} {}\nasm {asm}\n{}\n",
        revision(&dav1d),
        revision(&ffmpeg),
        revision(&libva),
        cc.display(),
        version(&cc),
        configure.join(" ")
    );
    let stamp_path = root.join("stamp");
    if fs::read_to_string(&stamp_path).ok().as_deref() != Some(stamp.as_str()) {
        println!("cargo:warning=FFmpeg: building dav1d and FFmpeg from third_party/ (once)");
        if root.exists() {
            fs::remove_dir_all(&root).expect("deleting the stale FFmpeg build");
        }
        fs::create_dir_all(&root).unwrap();
        build_dav1d(&dav1d, &root, &prefix, &cc, asm);
        install_libva_headers(&libva, &prefix);
        write_pkg_config(&prefix);
        build_va_shim(&manifest, &root, &prefix);
        build_ffmpeg(&ffmpeg, &root, &prefix, &configure);
        fs::write(&stamp_path, &stamp).unwrap();
    } else {
        // The shim is not part of the stamp: a change to it is a recompile of
        // it alone, which FFmpeg's libraries only link against.
        build_va_shim(&manifest, &root, &prefix);
    }

    cc::Build::new()
        .file(manifest.join("cpp/vento_video.c"))
        .file(manifest.join("cpp/vento_ffaudio.c"))
        .include(prefix.join("include"))
        .warnings(true)
        .compile("vento_video");
    // libavcodec and what it needs, in the order a static link wants them,
    // from its own pkg-config file: dav1d and libavutil after it, the libva
    // shim after that, and the system's libm, libdl and threads. (Not libatomic, which FFmpeg's configure
    // named wherever it was installed until patches/ffmpeg/0001.)
    for lib in static_libs(&prefix, "libavcodec") {
        if prefix.join("lib").join(format!("lib{lib}.a")).is_file() {
            println!("cargo:rustc-link-lib=static={lib}");
        } else {
            println!("cargo:rustc-link-lib={lib}");
        }
    }
    println!(
        "cargo:rustc-link-search=native={}",
        prefix.join("lib").display()
    );
    // For `linux/build.rs`, which hands it to the C backends' CMake build.
    println!("cargo:prefix={}", prefix.display());
}

/// FFmpeg's configure line, which is also part of the stamp.
fn configure_args(prefix: &Path, cc: &Path, asm: bool) -> Vec<String> {
    let mut args: Vec<String> = [
        "--disable-everything",
        "--disable-autodetect",
        "--disable-iconv",
        "--disable-programs",
        "--disable-doc",
        "--disable-network",
        "--disable-debug",
        "--disable-avformat",
        "--disable-avfilter",
        "--disable-avdevice",
        "--enable-swscale",
        "--enable-swresample",
        "--enable-static",
        "--disable-shared",
        // Into the C backends' shared libraries as well as the binary.
        "--enable-pic",
        "--enable-pthreads",
        "--enable-libdav1d",
        // Hardware decoding (`HWACCELS`), against the headers of
        // `third_party/` through the pkg-config files `write_pkg_config`
        // puts in the prefix. Named, since `--disable-autodetect` turns off
        // everything that is not; nothing else of the machine's is looked
        // for, so no libdrm and no X11.
        "--enable-vaapi",
        // dav1d's pkg-config file is the only one it reads, and the static
        // half of it is what a static link needs.
        "--pkg-config-flags=--static",
    ]
    .map(String::from)
    .into();
    args.push(format!("--prefix={}", prefix.display()));
    args.push(format!("--cc={}", cc.display()));
    args.push(format!("--enable-decoder={}", DECODERS.join(",")));
    args.push(format!("--enable-hwaccel={}", HWACCELS.join(",")));
    if !asm {
        args.push("--disable-x86asm".into());
    }
    args
}

fn build_dav1d(source: &Path, root: &Path, prefix: &Path, cc: &Path, asm: bool) {
    let build = root.join("dav1d-build");
    let mut setup = Command::new("meson");
    setup
        .arg("setup")
        .arg(&build)
        .arg(source)
        .arg(format!("--prefix={}", prefix.display()))
        .args([
            "--libdir=lib",
            "--buildtype=release",
            "--default-library=static",
            "-Db_staticpic=true",
            "-Denable_tools=false",
            "-Denable_tests=false",
            "-Denable_examples=false",
            // Nothing is fetched: dav1d needs no subproject without its tools.
            "--wrap-mode=nodownload",
        ])
        .arg(format!("-Denable_asm={asm}"))
        .env("CC", cc);
    run(setup, &root.join("dav1d-setup.log"));
    let mut install = Command::new("ninja");
    install.arg("-C").arg(&build).arg("install").arg(jobs());
    run(install, &root.join("dav1d-build.log"));
}

/// libva's headers into the prefix, where its own install would put them
/// (`include/va/`), with the one it generates: `va_version.h`, from its
/// template and the version `meson.build` declares. Nothing of libva is
/// compiled; its functions are `cpp/va_shim.c`'s.
fn install_libva_headers(source: &Path, prefix: &Path) {
    let into = prefix.join("include/va");
    fs::create_dir_all(&into).unwrap();
    let meson = fs::read_to_string(source.join("meson.build")).expect("libva's meson.build");
    let part = |name: &str| -> String {
        meson
            .lines()
            .find_map(|l| {
                let (key, value) = l.split_once('=')?;
                (key.trim() == name).then(|| value.trim().to_string())
            })
            .unwrap_or_else(|| panic!("libva's meson.build declares no {name}"))
    };
    let (major, minor, micro) = (
        part("va_api_major_version"),
        part("va_api_minor_version"),
        part("va_api_micro_version"),
    );
    let template =
        fs::read_to_string(source.join("va/va_version.h.in")).expect("libva's va_version.h.in");
    let version = template
        .replace("@VA_API_MAJOR_VERSION@", &major)
        .replace("@VA_API_MINOR_VERSION@", &minor)
        .replace("@VA_API_MICRO_VERSION@", &micro)
        .replace("@VA_API_VERSION@", &format!("{major}.{minor}.{micro}"));
    assert!(
        !version.contains("@VA_"),
        "va_version.h.in has a placeholder this does not fill"
    );
    fs::write(into.join("va_version.h"), version).unwrap();
    // The headers libva's install puts there, which `va/meson.build` lists:
    // the core's and the DRM one's, which is in a directory of its own in the
    // tree and beside the rest once installed.
    let list = fs::read_to_string(source.join("va/meson.build")).expect("libva's va/meson.build");
    let listed = |name: &str| -> Vec<String> {
        let start = list
            .find(&format!("{name} = ["))
            .unwrap_or_else(|| panic!("va/meson.build lists no {name}"));
        let body = &list[start..start + list[start..].find(']').unwrap()];
        body.split('\'')
            .skip(1)
            .step_by(2)
            .map(String::from)
            .collect()
    };
    let headers = [listed("libva_headers"), listed("libva_drm_headers")].concat();
    assert!(
        headers.iter().any(|h| h == "va.h") && headers.iter().any(|h| h == "drm/va_drm.h"),
        "va/meson.build's header lists are not what this reads them as"
    );
    for header in headers {
        let from = source.join("va").join(&header);
        fs::copy(&from, into.join(from.file_name().unwrap())).unwrap();
    }
}

/// What FFmpeg's configure is told libva is, through the pkg-config files it
/// reads (the prefix's alone, as for dav1d): libva and libva-drm are the
/// headers just installed and the shim.
fn write_pkg_config(prefix: &Path) {
    let dir = prefix.join("lib/pkgconfig");
    fs::create_dir_all(&dir).unwrap();
    let head = format!(
        "prefix={}\nlibdir=${{prefix}}/lib\nincludedir=${{prefix}}/include\n\n",
        prefix.display()
    );
    // The version libva would say it is: 2.x for the 1.x of its API.
    let api = fs::read_to_string(prefix.join("include/va/va_version.h")).unwrap();
    let api = api
        .lines()
        .find_map(|l| l.strip_prefix("#define VA_VERSION_S"))
        .map(|v| v.trim().trim_matches('"').to_string())
        .expect("va_version.h has VA_VERSION_S");
    let libva_version = match api.split_once('.') {
        Some(("1", rest)) => format!("2.{rest}"),
        _ => api.clone(),
    };
    fs::write(
        dir.join("libva.pc"),
        format!(
            "{head}Name: libva\nDescription: libva's headers over vento's dlopen shim\n\
             Version: {libva_version}\nCflags: -I${{includedir}}\n\
             Libs: -L${{libdir}} -lvento_va -ldl\n"
        ),
    )
    .unwrap();
    fs::write(
        dir.join("libva-drm.pc"),
        format!(
            "{head}Name: libva-drm\nDescription: libva-drm's header over vento's dlopen shim\n\
             Version: {libva_version}\nRequires: libva\nCflags: -I${{includedir}}\n\
             Libs: -L${{libdir}} -lvento_va\n"
        ),
    )
    .unwrap();
}

/// `cpp/va_shim.c` as `libvento_va.a` in the prefix, which the pkg-config files
/// above name and FFmpeg's libraries are linked with from then on. Copied in
/// only when it differs, so that a build of the C backends that links it is
/// not made to link again for nothing.
fn build_va_shim(manifest: &Path, root: &Path, prefix: &Path) {
    let out = root.join("va-shim");
    fs::create_dir_all(&out).unwrap();
    cc::Build::new()
        .file(manifest.join("cpp/va_shim.c"))
        .include(prefix.join("include"))
        .warnings(true)
        .pic(true)
        .out_dir(&out)
        .cargo_metadata(false)
        .compile("vento_va");
    let built = out.join("libvento_va.a");
    let installed = prefix.join("lib/libvento_va.a");
    fs::create_dir_all(installed.parent().unwrap()).unwrap();
    if fs::read(&built).ok() != fs::read(&installed).ok() {
        fs::copy(&built, &installed).unwrap();
    }
}

fn build_ffmpeg(source: &Path, root: &Path, prefix: &Path, configure: &[String]) {
    let build = root.join("ffmpeg-build");
    fs::create_dir_all(&build).unwrap();
    let mut cmd = Command::new(source.join("configure"));
    cmd.current_dir(&build)
        .args(configure)
        // dav1d's file and no other: the system's are not looked at.
        .env("PKG_CONFIG_LIBDIR", prefix.join("lib/pkgconfig"))
        .env_remove("PKG_CONFIG_PATH");
    run(cmd, &root.join("ffmpeg-configure.log"));
    let mut make = Command::new("make");
    make.current_dir(&build).arg(jobs()).arg("install");
    run(make, &root.join("ffmpeg-build.log"));
}

/// `-lfoo` for each library `pkg-config --static` names for `package` in the
/// built prefix, each once, at its last place, which is where a static link
/// needs it.
fn static_libs(prefix: &Path, package: &str) -> Vec<String> {
    let out = Command::new("pkg-config")
        .args(["--static", "--libs", package])
        .env("PKG_CONFIG_LIBDIR", prefix.join("lib/pkgconfig"))
        .env_remove("PKG_CONFIG_PATH")
        .output()
        .expect("running pkg-config");
    assert!(
        out.status.success(),
        "pkg-config does not know the {package} just built"
    );
    let words: Vec<String> = String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .filter_map(|w| match w {
            "-pthread" => Some("pthread".to_string()),
            w => w.strip_prefix("-l").map(String::from),
        })
        .collect();
    let mut libs = Vec::new();
    for (i, lib) in words.iter().enumerate() {
        if !words[i + 1..].contains(lib) {
            libs.push(lib.clone());
        }
    }
    libs
}

fn jobs() -> String {
    format!("-j{}", env::var("NUM_JOBS").unwrap_or_else(|_| "4".into()))
}

/// Runs `cmd` with its output into `log`, and fails the build with the log's
/// end when it fails.
fn run(mut cmd: Command, log: &Path) {
    let file = fs::File::create(log).unwrap();
    let status = cmd
        .stdout(file.try_clone().unwrap())
        .stderr(file)
        .status()
        .unwrap_or_else(|e| panic!("could not run {:?}: {e}", cmd.get_program()));
    if !status.success() {
        let text = fs::read_to_string(log).unwrap_or_default();
        let tail: Vec<&str> = text.lines().rev().take(30).collect();
        let tail: Vec<&str> = tail.into_iter().rev().collect();
        panic!(
            "{:?} failed ({status}); {}:\n{}",
            cmd.get_program(),
            log.display(),
            tail.join("\n")
        );
    }
}

fn runs(program: &str, args: &[&str]) -> bool {
    Command::new(program)
        .args(args)
        .output()
        .is_ok_and(|o| o.status.success())
}

/// The submodule's checked-out commit, or "unknown" outside a git checkout.
fn revision(source: &Path) -> String {
    Command::new("git")
        .arg("-C")
        .arg(source)
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "unknown".into())
}

/// The file whose change is a checkout of another commit in the submodule:
/// `HEAD` in its git directory, which a submodule's `.git` file points to.
fn git_head(source: &Path) -> Option<PathBuf> {
    let dot_git = source.join(".git");
    let dir = if dot_git.is_dir() {
        dot_git
    } else {
        let text = fs::read_to_string(&dot_git).ok()?;
        source.join(text.trim().strip_prefix("gitdir:")?.trim())
    };
    Some(dir.join("HEAD")).filter(|p| p.is_file())
}

/// The compiler's first line of `--version`, so that an upgrade rebuilds.
fn version(cc: &Path) -> String {
    Command::new(cc)
        .arg("--version")
        .output()
        .ok()
        .and_then(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .next()
                .map(String::from)
        })
        .unwrap_or_default()
}
