// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The media of a session: what a protocol hands its picture to when the far
//! end sends video, and its sound to when the far end sends any, with
//! neither the protocol nor the platform knowing the other.
//!
//! A protocol whose picture is video gets a [`video::Decoders`] from
//! whoever started it, and does not know whose: the phone's hardware. What is
//! done with a decoder's planes afterwards — finding what moved, and
//! converting only that — is [`video`]'s. A protocol with sound gets an
//! [`audio::Outputs`] the same way, and writes into the [`audio::Queue`] a
//! platform's device drains.
//!
//! Nothing here is about a session's conversation — the handler, the
//! events, the thread a protocol runs on — which is the rest of this
//! crate's, and nothing here uses it: the two meet in the protocols that have
//! a picture or a sound, and in `remotedesktop-api`.

pub mod audio;
pub mod video;
