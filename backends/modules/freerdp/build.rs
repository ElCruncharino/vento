// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! Generates Rust bindings for `vento/freerdp.h`, this backend's header
//! (`cpp/include`), for the `ffi` module in `src/lib.rs`.
//!
//! Only the `vento_freerdp_` items are generated. The types they use
//! (`vento_session`, `vento_callbacks`) are the `api` crate's, from its
//! bindings for the shared headers, and the module brings those into scope,
//! so the session created here has the type the adapter drives.

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
    // On Linux the library is opened at run time (`api::c::library`), so its
    // functions are a table of symbols looked up in it, all required, rather
    // than extern declarations resolved at link time. On Android the C is
    // linked into the same library as this crate.
    let builder = if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
        builder
            .dynamic_library_name("Library")
            .dynamic_link_require_all(true)
            // Wraps the table's calls in `unsafe` blocks, which edition 2024
            // requires inside an `unsafe fn`.
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
