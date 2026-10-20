// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The playback channel: the guest's sound, as PCM or Opus, onto the
//! session's output (`runtime::media::audio`).
//!
//! The server says what it sends before it sends it — a `MODE` for the
//! codec, a `START` for the rate and channels — and says when the guest has
//! stopped (`STOP`), so this is a small state machine in front of a
//! [`Player`], which owns the stream and the decoder. Their renderer has a
//! channel of its own for this, with cpal and libopus behind it; this one
//! plays where every other backend here plays, and decodes Opus with the
//! platform's decoder (libavcodec on the Linux client, MediaCodec on the
//! phone), which is why Opus is offered only where that decodes it.

use runtime::media::audio::decode::Codec;
use runtime::media::audio::{Format, Outputs, Player};
use std::sync::Arc;
use std::time::Duration;

/// `SPICE_MSG_PLAYBACK_*`, the server's messages on this channel.
mod message {
    pub const DATA: u16 = 101;
    pub const MODE: u16 = 102;
    pub const START: u16 = 103;
    pub const STOP: u16 = 104;
    pub const VOLUME: u16 = 105;
    pub const MUTE: u16 = 106;
}

/// `SPICE_AUDIO_DATA_MODE_*`: what `DATA` carries. CELT (2) is offered by
/// nothing that still builds.
const MODE_RAW: u16 = 1;
const MODE_OPUS: u16 = 3;

/// `SPICE_AUDIO_FMT_S16`, the one sample format the protocol has.
const FORMAT_S16: u16 = 1;

/// How long a stream stays open after its sound was heard: the gap between
/// one sound and the next it rides over, as TigerVNC's and FreeRDP's
/// streams do.
pub const QUIET: Duration = Duration::from_secs(2);

/// One session's playback channel, past its link.
pub struct Playback {
    player: Player,
    mode: u16,
    /// What `START` said, until `STOP`.
    format: Option<Format>,
    /// The guest's volume and mute, as `VOLUME` and `MUTE` last said: kept
    /// apart and multiplied, so that unmuting is the volume it was.
    volume: f32,
    muted: bool,
}

impl Playback {
    pub fn new(outputs: Arc<dyn Outputs>) -> Playback {
        Playback {
            player: Player::new(outputs),
            mode: MODE_RAW,
            format: None,
            volume: 1.0,
            muted: false,
        }
    }

    /// The channel capabilities to link with: the volume always, and Opus
    /// where the platform decodes it.
    pub fn caps(&self) -> u32 {
        use crate::channel::playback_caps::{OPUS, VOLUME};
        if self.player.decodable(Codec::Opus) { VOLUME | OPUS } else { VOLUME }
    }

    pub fn message(&mut self, message_type: u16, body: &[u8]) {
        match message_type {
            message::MODE => {
                // The multimedia time, then the mode.
                if let Some(mode) = u16_at(body, 4) {
                    log::info!(
                        "playback: the server sends {}",
                        match mode {
                            MODE_RAW => "PCM".to_string(),
                            MODE_OPUS => "Opus".to_string(),
                            other => format!("mode {other}, which is not played"),
                        }
                    );
                    self.mode = mode;
                }
            }
            message::START => {
                // Channels, sample format, rate, multimedia time.
                let (Some(channels), Some(sample), Some(rate)) = (u32_at(body, 0), u16_at(body, 4), u32_at(body, 6))
                else {
                    return;
                };
                self.format = Format::new(rate, channels as usize).filter(|_| sample == FORMAT_S16);
                match self.format {
                    Some(_) => log::info!("playback: started, {rate} Hz in {channels} channels"),
                    None => log::warn!(
                        "playback: started in format {sample} at {rate} Hz in {channels} channels, which is not played"
                    ),
                }
            }
            message::DATA => {
                // The multimedia time, then the samples or the packet. The
                // time is when the server would have the guest heard, which
                // a client that keeps the picture in step with its sound
                // would use; nothing here delays the picture, and the sound
                // is played as soon as the queue has its share of it.
                let (Some(format), Some(data)) = (self.format, body.get(4..)) else {
                    return;
                };
                match self.mode {
                    MODE_RAW => self.player.pcm(format, data),
                    MODE_OPUS => self.player.packet(Codec::Opus, format, &[], data),
                    _ => {}
                }
            }
            message::STOP => {
                // The guest has stopped its stream. What is queued is still
                // played, and the stream closes once it has been heard
                // ([`Playback::tick`]) unless the guest starts again first.
                log::info!("playback: stopped");
                self.format = None;
            }
            message::VOLUME => {
                // A count of channels, then a `u16` each; the loudest is the
                // stream's gain, as RDP's volume is taken here. Linear, with
                // full scale at 65535, as spice-gtk reads it and as QEMU sends
                // it (its guest's 0-255 times 257), and only from a guest
                // whose sound card has a mixer: the rig's has none, and a
                // guest that has one and never set it is muted at 0, which
                // is the guest's silence and is played as such.
                let Some(&count) = body.first() else { return };
                let loudest = (0..count as usize).filter_map(|i| u16_at(body, 1 + i * 2)).max();
                if let Some(loudest) = loudest {
                    self.volume = f32::from(loudest) / f32::from(u16::MAX);
                    log::info!("playback: the guest's volume is {loudest} of 65535");
                    self.apply_gain();
                }
            }
            message::MUTE => {
                if let Some(&muted) = body.first() {
                    self.muted = muted != 0;
                    log::info!("playback: the guest {}", if self.muted { "muted itself" } else { "unmuted itself" });
                    self.apply_gain();
                }
            }
            _ => {}
        }
    }

    /// Called after every turn of the session's loop, and every half second
    /// while a stream is open: closes the stream once it has gone quiet.
    pub fn tick(&mut self) {
        self.player.close_if_quiet(QUIET);
    }

    pub fn is_open(&self) -> bool {
        self.player.is_open()
    }

    fn apply_gain(&mut self) {
        self.player.set_gain(if self.muted { 0.0 } else { self.volume });
    }
}

fn u16_at(body: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(body.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(body: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(body.get(at..at + 4)?.try_into().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime::media::audio::{Controls, Device, Queue};
    use std::sync::Mutex;

    struct Quiet;
    impl Device for Quiet {}

    struct Fake {
        controls: Arc<Controls>,
        queues: Mutex<Vec<Arc<Queue>>>,
    }

    impl Outputs for Fake {
        fn controls(&self) -> &Arc<Controls> {
            &self.controls
        }

        fn device(&self, queue: Arc<Queue>) -> Result<Box<dyn Device>, String> {
            self.queues.lock().unwrap().push(queue);
            Ok(Box::new(Quiet))
        }
    }

    fn start(channels: u32, rate: u32) -> Vec<u8> {
        let mut body = channels.to_le_bytes().to_vec();
        body.extend(FORMAT_S16.to_le_bytes());
        body.extend(rate.to_le_bytes());
        body.extend(0u32.to_le_bytes());
        body
    }

    #[test]
    fn pcm_plays_in_the_format_start_said_and_stop_ends_it() {
        let outputs = Arc::new(Fake {
            controls: Arc::new(Controls::new()),
            queues: Mutex::new(Vec::new()),
        });
        let mut playback = Playback::new(outputs.clone());
        assert_eq!(playback.caps(), crate::channel::playback_caps::VOLUME, "no decoders, no Opus");
        // Data before a start is nothing to play.
        playback.message(message::DATA, &[0, 0, 0, 0, 1, 0, 2, 0]);
        assert!(!playback.is_open());

        playback.message(message::MODE, &[0, 0, 0, 0, 1, 0]);
        playback.message(message::START, &start(2, 44_100));
        playback.message(message::DATA, &[9, 9, 9, 9, 1, 0, 2, 0, 3, 0, 4, 0]);
        assert!(playback.is_open());
        assert!(outputs.controls.playing());
        let queues = outputs.queues.lock().unwrap();
        assert_eq!(queues.len(), 1);
        assert_eq!(queues[0].format(), Format::new(44_100, 2).unwrap());
        assert_eq!(outputs.controls.playback().unwrap().describe(), "PCM 44.1 kHz stereo");
        drop(queues);

        // After a stop the stream stays until it has been heard; data with
        // no start in front of it is dropped.
        playback.message(message::STOP, &[]);
        playback.message(message::DATA, &[0, 0, 0, 0, 1, 0, 2, 0]);
        assert!(playback.is_open());
        // A channel that ends is dropped with the stream it had.
        drop(playback);
        assert!(!outputs.controls.playing());
    }

    #[test]
    fn the_guests_volume_and_mute_are_the_streams_gain() {
        let outputs = Arc::new(Fake {
            controls: Arc::new(Controls::new()),
            queues: Mutex::new(Vec::new()),
        });
        let mut playback = Playback::new(outputs.clone());
        playback.message(message::START, &start(1, 48_000));
        playback.message(message::VOLUME, &[2, 0xff, 0x7f, 0x00, 0x10]);
        playback.message(message::MUTE, &[1]);
        playback.message(message::DATA, &[0, 0, 0, 0, 0, 0x40]);
        let queue = Arc::clone(&outputs.queues.lock().unwrap()[0]);
        // Primed first: before its target a queue plays silence.
        queue.write(&vec![0x4000; 48_000]);
        let mut out = [1i16; 1];
        queue.pull(&mut out, queue.format());
        assert_eq!(out[0], 0, "muted");
        playback.message(message::MUTE, &[0]);
        queue.pull(&mut out, queue.format());
        // 0x7fff of 0xffff: the louder channel, about half.
        assert!((0x1ff0..=0x2010).contains(&out[0]), "{:#x}", out[0]);
    }

    #[test]
    fn a_short_message_is_ignored() {
        let outputs = Arc::new(Fake {
            controls: Arc::new(Controls::new()),
            queues: Mutex::new(Vec::new()),
        });
        let mut playback = Playback::new(outputs.clone());
        playback.message(message::START, &[2, 0, 0]);
        playback.message(message::MODE, &[0]);
        playback.message(message::VOLUME, &[4, 0]);
        playback.message(message::DATA, &[0, 0, 0, 0, 1, 0]);
        assert!(!playback.is_open());
    }
}
