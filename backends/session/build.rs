// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! With the `c` feature: `vento/backend.h`, `vento/video.h` and
//! `vento/audio.h`, this crate's `include/`, as Rust, for `src/c/ffi.rs`.
//!
//! Generated rather than written: the vtable's member order is the ABI, and a
//! `#[repr(C)]` copy by hand would be a second place it is kept, drifting the
//! first time a header changed. The headers are ours, small and C, and every
//! item wanted is named `vento_` or `VENTO_`.
//!
//! Only the shared headers: each C backend's own, `vento/<name>.h` in its
//! module, is read by that backend's crate (RealVNC's by the Linux client,
//! which holds its adapter), over the declarations generated here, so that
//! one `vento_session` is one Rust type whichever crate names it.

fn main() {
    #[cfg(feature = "c")]
    generate_bindings();
}

#[cfg(feature = "c")]
fn generate_bindings() {
    use std::env;
    use std::path::PathBuf;

    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let include = manifest.join("include");
    println!("cargo:rerun-if-changed={}", include.display());
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    let bindings = bindgen::Builder::default()
        .header_contents(
            "vento.h",
            "#include <vento/backend.h>\n\
             #include <vento/video.h>\n\
             #include <vento/audio.h>\n",
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
