// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! `vento/freerdp.h`, this backend's own header (`cpp/include`), as
//! Rust, for the `ffi` module in `src/lib.rs`.
//!
//! Only its own items, the `vento_freerdp_` ones. What they are written
//! against — `vento_session`, `vento_callbacks` — is the `api` crate's,
//! which generates the shared headers, and the module brings those into
//! scope, so that the session this creates is the type the adapter drives.

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
        .header_contents("freerdp.h", "#include <vento/freerdp.h>\n")
        .clang_arg(format!("-I{}", own.display()))
        .clang_arg(format!("-I{}", shared.display()))
        .allowlist_item("vento_freerdp_.*")
        .allowlist_item("VENTO_FREERDP_.*")
        .allowlist_recursively(false)
        .derive_default(true)
        .layout_tests(false);
    // On the Linux client the library is opened at run time
    // (`api::c::library`), so its functions are a table looked up in it
    // rather than extern declarations a link has to satisfy, every one of them
    // required; on the phone the C is linked into the same library as this
    // crate.
    let builder = if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
        builder
            .dynamic_library_name("Library")
            .dynamic_link_require_all(true)
            // The table's calls in `unsafe` blocks of their own, which
            // edition 2024 asks of an `unsafe fn`'s body.
            .wrap_unsafe_ops(true)
    } else {
        builder
    };
    builder
        .generate()
        .expect("bindgen over vento/freerdp.h")
        .write_to_file(out.join("bindings.rs"))
        .expect("writing bindings.rs");
}
