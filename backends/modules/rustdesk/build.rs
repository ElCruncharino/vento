// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! Compiles RustDesk's two `.proto` files into this crate.
//!
//! The protocol is defined only by those files: there is no specification and
//! no second implementation to check against, so a hand-typed copy would be a
//! derivative work with worse provenance and over a thousand lines of message
//! definitions to keep in sync by hand.
//!
//! They come from two submodules. `message.proto`, what two peers say to each
//! other, is in RustDesk's own repository (`third_party/rustdesk`, at a release
//! tag) as `libs/base/protos/message.proto`, where it moved from hbb_common on
//! 2026-09-07. `rendezvous.proto`, what a peer says to the rendezvous server and
//! relay, stayed in hbb_common (`third_party/hbb_common`), which RustDesk's
//! client and server share, and so did `IdPk`, the signed identity. hbb_common
//! is pinned at the commit that release names as its own `libs/hbb_common`, so
//! the two files are the pair it was built with: both are `package hbb`, and a
//! message defined in both would fail here as a duplicate.

use std::path::{Path, PathBuf};

fn main() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../third_party");
    let message = dir(&root, "rustdesk", "libs/base/protos");
    let rendezvous = dir(&root, "hbb_common", "protos");
    let inputs = [message.join("message.proto"), rendezvous.join("rendezvous.proto")];
    for f in &inputs {
        println!("cargo:rerun-if-changed={}", f.display());
    }

    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("protos");
    std::fs::create_dir_all(&out).unwrap();
    protobuf_codegen::Codegen::new()
        .pure()
        .out_dir(&out)
        .inputs(&inputs)
        .include(&message)
        .include(&rendezvous)
        .run()
        .expect("generating the protobuf modules");

    // The generated files start with inner attributes, which `include!` cannot
    // carry into a module. They are dropped here, and the same set is applied
    // as outer attributes on the modules in `protos.rs`.
    for f in ["message.rs", "rendezvous.rs"] {
        let path = out.join(f);
        let src = std::fs::read_to_string(&path).unwrap();
        let kept: String = src
            .lines()
            .filter(|l| !l.starts_with("#![") && !l.starts_with("//!"))
            .map(|l| format!("{l}\n"))
            .collect();
        std::fs::write(&path, kept).unwrap();
    }
}

/// A directory of `.proto` files in a submodule, or a panic naming the
/// submodule to initialise.
fn dir(root: &Path, submodule: &str, protos: &str) -> PathBuf {
    let path = root.join(submodule).join(protos);
    path.canonicalize().unwrap_or_else(|e| {
        panic!(
            "{}: {e}; run `git submodule update --init third_party/{submodule}`",
            path.display()
        )
    })
}
