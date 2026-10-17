// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The desktop's sound output, played and recorded: a 440 Hz tone and a
//! script of what a session does to it — a gain, a mute from another thread,
//! clicks to time, a burst far faster than real time, a stall — through
//! [`Alsa`] into whatever ALSA's default device is routed to, recorded off a
//! PulseAudio monitor with `parec`, with a timeline of what was done when.
//! `scripts/testaudio/analyse.py rig` reads the two back.
//!
//! **Run it through `scripts/testaudio/rig.sh`, not by hand**: it plays
//! sound. The script loads a null sink, routes this process's ALSA stream
//! there (`PIPEWIRE_NODE`), and checks with `pactl` that it is there before
//! it says `go`. Until `go` arrives on stdin this writes silence and nothing
//! else, so a stream that went to the speakers by mistake is a silent one;
//! without `go` within thirty seconds it exits having played nothing.
//!
//! ```text
//! audio_rig --out DIR [--monitor vento_test.monitor]
//! ```
//!
//! Desktop only, like the crate: on Android this is an empty program, so that
//! the workspace's checks for the phone build every target.

#[cfg(not(target_os = "android"))]
mod rig;

fn main() {
    #[cfg(not(target_os = "android"))]
    rig::main();
}
