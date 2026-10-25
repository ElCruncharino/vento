// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! RustDesk's message definitions, generated at build time from the submodule.
//!
//! Sibling modules rather than nested, because a generated file refers to
//! another by module path where its `.proto` imports one.

#[allow(clippy::all, dead_code, unused_mut, unused_results, trivial_casts)]
#[allow(non_camel_case_types, non_snake_case, non_upper_case_globals)]
pub mod message {
    include!(concat!(env!("OUT_DIR"), "/protos/message.rs"));
}

#[allow(clippy::all, dead_code, unused_mut, unused_results, trivial_casts)]
#[allow(non_camel_case_types, non_snake_case, non_upper_case_globals)]
pub mod rendezvous {
    include!(concat!(env!("OUT_DIR"), "/protos/rendezvous.rs"));
}
