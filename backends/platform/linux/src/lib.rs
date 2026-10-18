// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The parts of a backend that are about Linux rather than about a
//! protocol: [`ffmpeg`], which is libavcodec speaking
//! `common::video::Decoders`, [`audio`], which is PipeWire's streams
//! ([`pipewire`]) or ALSA's default device ([`alsa`]) speaking
//! `common::audio::Outputs`, and [`ffmpeg_audio`], libavcodec again, for
//! the sound that arrives as AAC or Opus.
//!
//! It is the Linux client's platform, as `platform_android` is the phone's:
//! the client (`linux/`) hands these decoders and outputs to every session
//! it opens. The same ones serve where the code is written, so a protocol
//! whose picture is video is no longer host-testable everywhere but there:
//! the probes write out what the far end's screen looked like, and the tests
//! decode real VP9, VP8, AV1, H.264 and H.265 through the same diff and
//! conversion the phone uses.
//!
//! What is the client's alone — the window, the input, what is done with a
//! session's events — is in `linux/`, not here: this crate is what a backend
//! needs of the platform, and is usable without a window.

// Empty on Android, so that the workspace builds for a phone with this crate in
// it and nothing of FFmpeg behind it.
#![cfg(not(target_os = "android"))]

pub mod alsa;
pub mod audio;
pub mod ffmpeg;
pub mod ffmpeg_audio;
pub mod pipewire;
