// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! Generates Rust bindings to `vento/libvnc.h`, this backend's header
//! (`cpp/include`), for the `ffi` module in `src/lib.rs`.
//!
//! Only the `vento_libvnc_` items are generated. The shared types they use
//! (`vento_session`, `vento_callbacks`) come from the `api` crate's bindings,
//! which the `ffi` module brings into scope, so the session created here has
//! the type the adapter expects.

use std::env;
use std::path::PathBuf;

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let own = manifest.join("cpp/include");
    let shared = manifest.join("../../api/include");
    println!("cargo:rerun-if-changed={}", own.display());
    println!("cargo:rerun-if-changed={}", shared.display());
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    let builder = bindgen::Builder::default()
        .header_contents("libvnc.h", "#include <vento/libvnc.h>\n")
        .clang_arg(format!("-I{}", own.display()))
        .clang_arg(format!("-I{}", shared.display()))
        .allowlist_item("vento_libvnc_.*")
        .allowlist_item("VENTO_LIBVNC_.*")
        .allowlist_recursively(false)
        .derive_default(true)
        .layout_tests(false);
    // On the Linux client the library is opened at run time
    // (`api::c::library`), so its functions are a table of symbols looked up
    // in it, all of them required, rather than extern declarations resolved
    // at link time. On the phone the C is linked into the same library as
    // this crate.
    let builder = if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
        builder
            .dynamic_library_name("Library")
            .dynamic_link_require_all(true)
            // Edition 2024 requires `unsafe` blocks inside an `unsafe fn`.
            .wrap_unsafe_ops(true)
    } else {
        builder
    };
    builder
        .generate()
        .expect("bindgen over vento/libvnc.h")
        .write_to_file(out.join("bindings.rs"))
        .expect("writing bindings.rs");
}
