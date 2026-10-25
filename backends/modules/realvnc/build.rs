// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! `vento/realvnc.h`, this backend's own header (`cpp/include`), as Rust,
//! for the `ffi` module in `src/lib.rs`.
//!
//! Only its own items, the `vento_realvnc_` ones, over the `api` crate's
//! shared declarations, as each C backend's crate reads its header — but its
//! functions as a loader rather than as `extern` declarations: a `Library`
//! that opens the shared library and looks every one of them up, so that
//! nothing here links against a library the client's build may not have made.
//! Every function is required: a library without one of them is not one this
//! header describes.
//!
//! Nothing on a target other than Linux, where the crate is empty.

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
        // The loader's calls in `unsafe` blocks of their own, which edition
        // 2024 asks of an `unsafe fn`'s body.
        .wrap_unsafe_ops(true)
        .generate()
        .expect("bindgen over vento/realvnc.h")
        .write_to_file(out.join("bindings.rs"))
        .expect("writing bindings.rs");
}
