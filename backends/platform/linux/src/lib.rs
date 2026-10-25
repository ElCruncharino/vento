// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The Linux parts of a backend that belong to no protocol: [`ffmpeg`],
//! libavcodec implementing `runtime::media::video::Decoders`; [`audio`],
//! PipeWire streams ([`pipewire`]) or ALSA's default device ([`alsa`])
//! implementing `runtime::media::audio::Outputs`; [`ffmpeg_audio`],
//! libavcodec's AAC and Opus decoders for compressed sound; and [`stderr`],
//! the stderr line writer shared by the client's logger, FFmpeg's log and
//! the C backends' `vento_log`.
//!
//! It is the Linux client's platform, as `platform_android` is the phone's:
//! the client (`linux/`) hands these decoders and outputs to every session it
//! opens. The same ones run on the development machine, so a protocol that
//! sends video is testable on the host: the probes write out the far end's
//! screen, and the tests decode real VP9, VP8, AV1, H.264 and H.265 through
//! the same diff and conversion the phone uses.
//!
//! The window, the input and the handling of a session's events belong to the
//! client and are in `linux/`. This crate needs no window.

// Empty on Android, so that the workspace builds for a phone with this crate in
// it and no FFmpeg.
#![cfg(not(target_os = "android"))]

pub mod alsa;
pub mod audio;
mod dl;
pub mod ffmpeg;
pub mod ffmpeg_audio;
pub mod pipewire;
pub mod stderr;

// The build script's cross-compiling arguments for FFmpeg and Meson, compiled
// here only for their tests.
#[cfg(test)]
#[path = "../cross.rs"]
mod cross;
