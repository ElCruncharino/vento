// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! Generates Rust bindings for `vento/realvnc.h`, this backend's header
//! (`cpp/include`), for the `ffi` module in `src/lib.rs`.
//!
//! Only the header's own `vento_realvnc_` items are generated, on top of the
//! `api` crate's shared declarations, as in each C backend's crate. The
//! functions are generated as a loader rather than as `extern` declarations:
//! a `Library` that opens the shared library and looks up each function, so
//! nothing here links against a library the client's build may not have
//! made. Every function is required: a library missing one of them is not
//! the one this header describes.
//!
//! Does nothing on targets other than Linux, where the crate is empty.

use std::env;
use std::path::PathBuf;

fn main() {
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("linux") {
        return;
    }
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let own = manifest.join("cpp/include");
    let shared = manifest.join("../../api/include");
    println!("cargo:rerun-if-changed={}", own.display());
    println!("cargo:rerun-if-changed={}", shared.display());
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    bindgen::Builder::default()
        .header_contents("realvnc.h", "#include <vento/realvnc.h>\n")
        .clang_arg(format!("-I{}", own.display()))
        .clang_arg(format!("-I{}", shared.display()))
        .allowlist_item("vento_realvnc_.*")
        .allowlist_item("VENTO_REALVNC_.*")
        .allowlist_recursively(false)
        .derive_default(true)
        .layout_tests(false)
        .dynamic_library_name("Library")
        .dynamic_link_require_all(true)
        // Wraps the loader's calls in their own `unsafe` blocks, as edition
        // 2024 expects in the body of an `unsafe fn`.
        .wrap_unsafe_ops(true)
        .generate()
        .expect("bindgen over vento/realvnc.h")
        .write_to_file(out.join("bindings.rs"))
        .expect("writing bindings.rs");
}
