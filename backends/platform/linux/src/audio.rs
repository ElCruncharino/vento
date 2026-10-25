// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The desktop's sound output, as this platform's
//! `runtime::media::audio::Outputs`: a PipeWire stream of its own per stream
//! ([`pipewire`](crate::pipewire)), where a PipeWire daemon answers, and ALSA's
//! `default` device ([`alsa`](crate::alsa)) where none does or libpipewire is
//! not there — a PulseAudio desktop, a bare ALSA one, a Flatpak sandbox granted
//! the pulse socket and not PipeWire's. Both libraries are loaded at run time,
//! and where neither is there the far end's sound is dropped, which the log
//! says once.
//!
//! PipeWire's own stream rather than its ALSA plugin, which is where ALSA's
//! `default` leads on a PipeWire desktop: the plugin keeps a ring of the
//! 50 ms asked for in front of the graph, and holds the writer to it, where
//! a stream of its own is pulled a graph cycle at a time and keeps nothing
//! (scratch/pipewire-eval measured RDP's sound 35-40 ms sooner, a mute heard
//! in 5 ms rather than 55-65); and it is the session's node, named for it,
//! in a mixer, rather than "PipeWire ALSA [vento]".

use std::sync::Arc;

use runtime::media::audio::decode::Decoders;
use runtime::media::audio::{Controls, Device, Outputs, Queue};

use crate::ffmpeg_audio::FFmpegAudio;
use crate::pipewire::Title;

/// Which output the streams play on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Output {
    /// PipeWire's where a daemon answers, ALSA's otherwise: the default.
    #[default]
    Auto,
    /// PipeWire's only: with no daemon, the far end's sound is dropped
    /// rather than played through ALSA. For tests of this path.
    PipeWire,
    /// ALSA's `default` device, as before there was a PipeWire output;
    /// libpipewire is not loaded. Without libasound, no sound.
    Alsa,
}

impl Output {
    /// `auto`, `pipewire` or `alsa`: the client's `--audio-output`, and
    /// `VENTO_AUDIO_OUTPUT`.
    pub fn from_name(name: &str) -> Option<Output> {
        match name {
            "auto" => Some(Output::Auto),
            "pipewire" => Some(Output::PipeWire),
            "alsa" => Some(Output::Alsa),
            _ => None,
        }
    }

    /// `VENTO_AUDIO_OUTPUT`, where it names one; [`Output::Auto`] otherwise.
    pub fn from_env() -> Output {
        match std::env::var("VENTO_AUDIO_OUTPUT") {
            Ok(name) => Output::from_name(&name).unwrap_or_else(|| {
                log::warn!("audio: VENTO_AUDIO_OUTPUT={name:?} is not auto, pipewire or alsa; taking auto");
                Output::Auto
            }),
            Err(_) => Output::Auto,
        }
    }
}

/// The desktop's output, as a session is handed it: one a session, for its
/// controls and for the name its streams are shown under.
pub struct Audio {
    controls: Arc<Controls>,
    title: Arc<Title>,
    output: Output,
}

impl Audio {
    /// A session's output, its streams called `title` (the desktop's name,
    /// or its address until the far end says one). [`Output::Auto`] is
    /// `VENTO_AUDIO_OUTPUT`'s choice where that is set.
    pub fn new(title: &str, output: Output) -> Audio {
        let output = if output == Output::Auto { Output::from_env() } else { output };
        Audio {
            controls: Arc::new(Controls::new()),
            title: Title::new(title),
            output,
        }
    }

    /// Renames the session's streams, those playing included: what a mixer
    /// lists them as.
    pub fn set_title(&self, title: &str) {
        self.title.set(title);
    }
}

impl Outputs for Audio {
    fn controls(&self) -> &Arc<Controls> {
        &self.controls
    }

    /// libavcodec's, the client's own FFmpeg.
    fn decoders(&self) -> Option<&dyn Decoders> {
        Some(&FFmpegAudio)
    }

    fn device(&self, queue: Arc<Queue>) -> Result<Box<dyn Device>, String> {
        match self.output {
            Output::Alsa => crate::alsa::device(queue),
            Output::Auto => Ok(crate::pipewire::device(queue, Arc::clone(&self.title), true)),
            Output::PipeWire => Ok(crate::pipewire::device(queue, Arc::clone(&self.title), false)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outputs_go_by_their_names() {
        assert_eq!(Output::from_name("auto"), Some(Output::Auto));
        assert_eq!(Output::from_name("pipewire"), Some(Output::PipeWire));
        assert_eq!(Output::from_name("alsa"), Some(Output::Alsa));
        assert_eq!(Output::from_name("PipeWire"), None);
        assert_eq!(Output::from_name("pulse"), None);
        assert_eq!(Output::default(), Output::Auto);
    }
}
