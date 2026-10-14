// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The Linux client's library half: the [`Backend`](backend::Backend) seam,
//! the [`Event`](events::Event) channel, the option registry, and the eight
//! adapters that fit the protocols to it.
//!
//! It is empty on Android. This is a workspace member so that
//! `cargo ndk … check --workspace` compiles it for a phone, and what it
//! compiles there is nothing — the binary's `main` is empty under that cfg
//! too — which is what keeps FFmpeg, cmake and bindgen out of the Android
//! dependency graph the licence pages are generated from.

#![cfg(not(target_os = "android"))]

pub mod backend;
pub mod backends;
pub mod events;
mod ffi;
pub mod options;
pub mod ppm;

pub mod log;
