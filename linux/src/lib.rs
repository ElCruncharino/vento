// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The Linux client's library half: the [`Backend`](backend::Backend) seam,
//! the [`Event`](events::Event) channel, the option registry, the eight
//! adapters that fit the protocols to it, and the window.

pub mod backend;
pub mod backends;
pub mod events;
mod ffi;
pub mod gui;
pub mod options;
pub mod ppm;

pub mod log;
