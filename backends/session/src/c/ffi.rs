// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The C backends' declarations, as `build.rs` generated them from
//! `vento/backend.h` and the four headers beside it.
//!
//! Nothing here is written by hand: the vtable's member order is the ABI,
//! and a retyped copy would be a second place it is kept, drifting the first
//! time a header changed. The one thing added is `extern "C" free`, which is
//! what `vento_strings` and `vento_realvnc_identity_saved` are freed with —
//! the header frees them that way too, so the allocator matches.
//!
//! The generated items are `pub` and `unsafe` to touch. [`super`] is the one
//! safe wrapper over the vtable; a backend's own crate reaches its config
//! struct, its constructor and its live calls here.

#![allow(non_camel_case_types)]
#![allow(non_snake_case)]
#![allow(dead_code)]

include!(concat!(env!("OUT_DIR"), "/bindings.rs"));

unsafe extern "C" {
    /// `free`, for the strings the C side hands over and documents as the
    /// caller's to release.
    pub fn free(ptr: *mut std::os::raw::c_void);
}
