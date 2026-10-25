// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! Compiles RustDesk's two `.proto` files into this crate.
//!
//! The protocol is defined only by those files: there is no specification and
//! no second implementation to check against, so a hand-typed copy would be a
//! derivative work with worse provenance and 1032 lines of message definitions
//! to keep in sync by hand. They come from the submodule at
//! `third_party/hbb_common`.
//!
//! It is pinned at the last upstream commit that has both. On 2026-09-07
//! `message.proto` moved out of hbb_common, which RustDesk's client and server
//! share, into rustdesk's own tree as `libs/base/protos/message.proto`
//! (rustdesk b50fde6); only what the server uses stayed. So this cannot follow
//! hbb_common's main branch past 3d6fb2c without a second source for that
//! file, and doing so shows up as a missing `message.proto` below.

use std::path::PathBuf;

fn main() {
    let protos = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../third_party/hbb_common/protos");
    let protos = protos.canonicalize().unwrap_or_else(|e| {
        panic!(
            "{}: {e} — run `git submodule update --init third_party/hbb_common`",
            protos.display()
        )
    });
    for f in ["message.proto", "rendezvous.proto"] {
        println!("cargo:rerun-if-changed={}", protos.join(f).display());
    }

    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("protos");
    std::fs::create_dir_all(&out).unwrap();
    protobuf_codegen::Codegen::new()
        .pure()
        .out_dir(&out)
        .inputs([protos.join("message.proto"), protos.join("rendezvous.proto")])
        .include(&protos)
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
