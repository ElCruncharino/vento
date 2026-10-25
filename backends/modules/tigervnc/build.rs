// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! `vento/tigervnc.h`, this backend's own header (`cpp/include`), as
//! Rust, for the `ffi` module in `src/lib.rs`.
//!
//! Only its own items, the `vento_tigervnc_` ones. The types they use
//! (`vento_session`, `vento_callbacks`) come from the `api` crate, which
//! generates the shared headers, and the module brings those into scope, so
//! that the session this creates is the type the adapter drives.

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
        .header_contents("tigervnc.h", "#include <vento/tigervnc.h>\n")
        .clang_arg(format!("-I{}", own.display()))
        .clang_arg(format!("-I{}", shared.display()))
        .allowlist_item("vento_tigervnc_.*")
        .allowlist_item("VENTO_TIGERVNC_.*")
        .allowlist_recursively(false)
        .derive_default(true)
        .layout_tests(false);
    // On the Linux client the library is opened at run time
    // (`api::c::library`), so its functions are a table looked up in it, all
    // of them required, rather than extern declarations a link has to
    // satisfy. On Android the C is linked into the same library as this
    // crate.
    let builder = if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
        builder
            .dynamic_library_name("Library")
            .dynamic_link_require_all(true)
            // The table's calls in `unsafe` blocks of their own, as edition
            // 2024 requires in an `unsafe fn`'s body.
            .wrap_unsafe_ops(true)
    } else {
        builder
    };
    builder
        .generate()
        .expect("bindgen over vento/tigervnc.h")
        .write_to_file(out.join("bindings.rs"))
        .expect("writing bindings.rs");
}
