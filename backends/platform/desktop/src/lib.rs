// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The parts of a backend that are about a desktop rather than about a
//! protocol: [`ffmpeg`], which is libavcodec speaking
//! `common::video::Decoders`, [`alsa`], which is ALSA's default device
//! speaking `common::audio::Outputs`, and [`ffmpeg_audio`], libavcodec
//! again, for the sound that arrives as AAC or Opus.
//!
//! It exists so that the picture can be had where the code is written. A
//! protocol whose picture is video was host-testable everywhere but there,
//! since the decoder was the phone's; with this beside it the probe writes out
//! what the far end's screen looked like, and the tests decode real VP9, VP8,
//! AV1, H.264 and H.265 through the same diff and conversion the phone uses.
//!
//! There is no desktop app, and this is not one: there is no `Events`
//! implementation here, no window and no input. Those arrive with a desktop
//! client if one ever does, and this crate is where they would go.

// Empty on Android, so that the workspace builds for a phone with this crate in
// it and nothing of FFmpeg behind it.
#![cfg(not(target_os = "android"))]

pub mod alsa;
pub mod ffmpeg;
pub mod ffmpeg_audio;
