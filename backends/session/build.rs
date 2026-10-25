// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! With the `c` feature: `vento/backend.h` and the three backend headers, as
//! Rust, for `src/c/ffi.rs`.
//!
//! Generated rather than written: the vtable's member order is the ABI, and a
//! `#[repr(C)]` copy by hand would be a second place it is kept, drifting the
//! first time a header changed. The headers are ours, small and C, and every
//! item wanted is named `vento_` or `VENTO_`.

fn main() {
    #[cfg(feature = "c")]
    generate_bindings();
}

#[cfg(feature = "c")]
fn generate_bindings() {
    use std::env;
    use std::path::PathBuf;

    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let include = manifest.join("../common/include");
    println!("cargo:rerun-if-changed={}", include.display());
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    let bindings = bindgen::Builder::default()
        .header_contents(
            "vento.h",
            "#include <vento/backend.h>\n\
             #include <vento/libvnc.h>\n\
             #include <vento/tigervnc.h>\n\
             #include <vento/freerdp.h>\n",
        )
        .clang_arg(format!("-I{}", include.display()))
        .allowlist_item("vento_.*")
        .allowlist_item("VENTO_.*")
        // The inline wrappers call through the vtable, which the Rust side
        // does itself; `vento_strings_free` is `free` on each item, done
        // there too, with libc's `free` so the allocator matches.
        .generate_inline_functions(false)
        .derive_default(true)
        .layout_tests(false)
        .generate()
        .expect("bindgen over vento/backend.h");
    bindings
        .write_to_file(out.join("bindings.rs"))
        .expect("writing bindings.rs");
}
