// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The far end's sound as a Rust protocol receives it — PCM in some format,
//! or packets of a codec — played on the session's [`Outputs`].
//!
//! What a C backend does with `vento/audio.h`'s table, opening a stream for
//! each format the far end changes to and a decoder for each codec, three
//! Rust protocols (SPICE's playback channel, RustDesk's audio frames,
//! IronRDP's rdpsnd) would otherwise each do again: so it is here once, with
//! the two things every one of them needs beyond the table — the codec said
//! to the session's [`Controls`] while a decoder is open, for the facts row,
//! and a stream closed once its sound has been heard and nothing more has
//! come, since none of those protocols says reliably that its sound has
//! ended and an open stream keeps the device running on silence (and the
//! window's Sound button showing).

use std::sync::Arc;
use std::time::{Duration, Instant};

use super::decode::{Codec, Decoding};
use super::{Format, Outputs, Stream};

/// One session's sound, as its protocol thread plays it. Every call returns
/// at once, as [`Stream`]'s do.
pub struct Player {
    outputs: Arc<dyn Outputs>,
    stream: Option<Stream>,
    decoder: Option<Open>,
    /// The far end's own volume (RDP's Volume PDU, SPICE's), kept across the
    /// streams it outlives.
    gain: f32,
    /// What a decoder's packets came out as, reused from packet to packet.
    samples: Vec<i16>,
    /// When the last sound written will have been heard: the write, and the
    /// stream's latency at that moment.
    heard_by: Option<Instant>,
    /// A codec and format no decoder would take, so that a stream of packets
    /// nobody can decode is one warning rather than one a packet.
    refused: Option<(Codec, Format)>,
}

/// An open decoder, and the format and setup it was opened for (its codec
/// it knows).
struct Open {
    format: Format,
    config: Vec<u8>,
    decoding: Decoding,
}

impl Player {
    pub fn new(outputs: Arc<dyn Outputs>) -> Player {
        Player {
            outputs,
            stream: None,
            decoder: None,
            gain: 1.0,
            samples: Vec::new(),
            heard_by: None,
            refused: None,
        }
    }

    /// Whether this platform decodes `codec`: what a protocol offers the far
    /// end, which then sends only what it was offered.
    pub fn decodable(&self, codec: Codec) -> bool {
        self.outputs.decoders().is_some_and(|d| d.decodable(codec))
    }

    /// Signed 16-bit samples, little-endian and interleaved, in `format`, cut
    /// anywhere. A format other than the open stream's is a new stream.
    pub fn pcm(&mut self, format: Format, bytes: &[u8]) {
        // Little-endian on the wire, and every platform this builds for is
        // too: the stream takes native-endian bytes.
        #[cfg(target_endian = "big")]
        compile_error!("the wire's samples are little-endian and are written as they arrive");
        self.decoder = None;
        if let Some(stream) = self.stream_in(format) {
            stream.write_bytes(bytes);
            self.wrote();
        }
    }

    /// One packet of `codec`, from a far end that said its stream is
    /// `format` and set up by `config` (empty where the protocol sends none:
    /// see [`Decoders::decoder`](super::decode::Decoders::decoder)). What it
    /// decodes to is played in the decoder's format, which is not always the
    /// one said.
    pub fn packet(&mut self, codec: Codec, format: Format, config: &[u8], packet: &[u8]) {
        let same = self
            .decoder
            .as_ref()
            .is_some_and(|d| d.decoding.codec() == codec && d.format == format && d.config == config);
        if !same {
            self.decoder = None;
            if self.refused == Some((codec, format)) {
                return;
            }
            let Some(decoders) = self.outputs.decoders() else {
                self.refuse(codec, format, "this platform decodes no sound");
                return;
            };
            match Decoding::open(decoders, self.outputs.controls(), codec, format, config) {
                Ok(decoding) => {
                    self.refused = None;
                    self.decoder = Some(Open {
                        format,
                        config: config.to_vec(),
                        decoding,
                    });
                }
                Err(why) => {
                    self.refuse(codec, format, &why);
                    return;
                }
            }
        }
        let Some(open) = self.decoder.as_mut() else {
            return;
        };
        self.samples.clear();
        match open.decoding.decode(packet, &mut self.samples) {
            Ok(Some(decoded)) => {
                let samples = std::mem::take(&mut self.samples);
                if let Some(stream) = self.stream_in(decoded) {
                    stream.write(&samples);
                    self.wrote();
                }
                self.samples = samples;
            }
            Ok(None) => {}
            Err(why) => log::debug!("audio: a {} packet would not decode: {why}", codec.name()),
        }
    }

    /// The far end's own volume, as a factor over the session's: 1 is as
    /// sent. Kept for the streams opened after it.
    pub fn set_gain(&mut self, gain: f32) {
        self.gain = gain;
        if let Some(stream) = &self.stream {
            stream.set_gain(gain);
        }
    }

    /// How long until a sample written now is heard, or zero with no stream.
    pub fn latency(&self) -> Duration {
        self.stream.as_ref().map_or(Duration::ZERO, Stream::latency)
    }

    pub fn is_open(&self) -> bool {
        self.stream.is_some()
    }

    /// The stream and the decoder closed now, with whatever is still queued:
    /// a far end that has stopped its sound, or a session ending.
    pub fn close(&mut self) {
        self.stream = None;
        self.decoder = None;
        self.heard_by = None;
    }

    /// The stream closed if everything written to it has been heard and
    /// nothing more has come for `after` beyond that; true if it closed.
    /// Called now and then by a protocol whose far end does not say when its
    /// sound ends — `after` long enough to ride over the gaps between one
    /// sound and the next, which the backends with sound before these took
    /// as two seconds.
    pub fn close_if_quiet(&mut self, after: Duration) -> bool {
        self.close_if_quiet_at(Instant::now(), after)
    }

    fn close_if_quiet_at(&mut self, now: Instant, after: Duration) -> bool {
        match self.heard_by {
            Some(heard_by) if self.stream.is_some() && now >= heard_by + after => {
                log::info!("audio: the far end's sound has stopped; its stream is closed");
                self.close();
                true
            }
            _ => false,
        }
    }

    /// The stream for `format`, opened (and the last one closed) where it is
    /// not the open one; `None` where the platform has no stream for it.
    fn stream_in(&mut self, format: Format) -> Option<&mut Stream> {
        if self.stream.as_ref().is_some_and(|s| s.format() != format) {
            self.stream = None;
        }
        if self.stream.is_none() {
            let stream = self.outputs.open(format)?;
            log::info!("audio: playing {} Hz in {} channels", format.rate, format.channels);
            stream.set_gain(self.gain);
            self.stream = Some(stream);
        }
        self.stream.as_mut()
    }

    fn wrote(&mut self) {
        self.heard_by = Some(Instant::now() + self.latency());
    }

    fn refuse(&mut self, codec: Codec, format: Format, why: &str) {
        log::warn!(
            "audio: no {} decoder for {} Hz in {} channels ({why}); that sound is dropped",
            codec.name(),
            format.rate,
            format.channels
        );
        self.refused = Some((codec, format));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::decode::{Decoder, Decoders};
    use crate::audio::{Controls, Device, Queue};
    use std::sync::Mutex;

    struct Quiet;
    impl Device for Quiet {}

    /// Doubles every byte of a packet into a sample, in a format of its own
    /// choosing: enough to tell what was decoded and what it came out as.
    struct Doubling(Format);

    impl Decoder for Doubling {
        fn decode(&mut self, packet: &[u8], out: &mut Vec<i16>) -> Result<Option<Format>, String> {
            if packet.is_empty() {
                return Err("empty".into());
            }
            out.extend(packet.iter().map(|&b| b as i16 * 2));
            Ok(Some(self.0))
        }

        fn name(&self) -> String {
            "doubling".into()
        }
    }

    struct Fake {
        controls: Arc<Controls>,
        queues: Mutex<Vec<Arc<Queue>>>,
        opened: Mutex<Vec<(Codec, Format)>>,
    }

    impl Outputs for Fake {
        fn controls(&self) -> &Arc<Controls> {
            &self.controls
        }

        fn device(&self, queue: Arc<Queue>) -> Result<Box<dyn Device>, String> {
            self.queues.lock().unwrap().push(queue);
            Ok(Box::new(Quiet))
        }

        fn decoders(&self) -> Option<&dyn Decoders> {
            Some(self)
        }
    }

    impl Decoders for Fake {
        fn decodable(&self, codec: Codec) -> bool {
            codec == Codec::Opus
        }

        fn decoder(&self, codec: Codec, format: Format, _config: &[u8]) -> Result<Box<dyn Decoder>, String> {
            if codec != Codec::Opus {
                return Err("not here".into());
            }
            self.opened.lock().unwrap().push((codec, format));
            // Opus comes out at 48 kHz whatever the far end said.
            Ok(Box::new(Doubling(Format::new(48_000, format.channels).unwrap())))
        }
    }

    fn fake() -> Arc<Fake> {
        Arc::new(Fake {
            controls: Arc::new(Controls::new()),
            queues: Mutex::new(Vec::new()),
            opened: Mutex::new(Vec::new()),
        })
    }

    #[test]
    fn pcm_plays_on_a_stream_per_format() {
        let outputs = fake();
        let mut player = Player::new(outputs.clone());
        let stereo = Format::new(44_100, 2).unwrap();
        player.pcm(stereo, &[1, 0, 2, 0, 3]);
        player.pcm(stereo, &[0, 4, 0]);
        assert_eq!(outputs.queues.lock().unwrap().len(), 1);
        assert_eq!(outputs.queues.lock().unwrap()[0].take_all(), [1, 2, 3, 4], "frames finished across writes");
        assert!(outputs.controls.playing());

        player.pcm(Format::new(22_050, 1).unwrap(), &[5, 0]);
        let queues = outputs.queues.lock().unwrap();
        assert_eq!(queues.len(), 2, "a new format is a new stream");
        assert_eq!(queues[1].format(), Format::new(22_050, 1).unwrap());
        assert_eq!(outputs.controls.playback().unwrap().describe(), "PCM 22.05 kHz mono");
    }

    #[test]
    fn packets_are_decoded_once_a_decoder_opens_and_say_their_codec() {
        let outputs = fake();
        let mut player = Player::new(outputs.clone());
        assert!(player.decodable(Codec::Opus));
        assert!(!player.decodable(Codec::Aac));
        let said = Format::new(24_000, 2).unwrap();
        player.packet(Codec::Opus, said, &[], &[1, 2]);
        player.packet(Codec::Opus, said, &[], &[3, 4]);
        // A broken packet is dropped, and the decoder kept for the next.
        player.packet(Codec::Opus, said, &[], &[]);
        player.packet(Codec::Opus, said, &[], &[5, 6]);
        assert_eq!(*outputs.opened.lock().unwrap(), [(Codec::Opus, said)]);
        let queue = Arc::clone(&outputs.queues.lock().unwrap()[0]);
        assert_eq!(queue.format(), Format::new(48_000, 2).unwrap(), "the decoder's format, not the one said");
        assert_eq!(queue.take_all(), [2, 4, 6, 8, 10, 12]);
        assert_eq!(outputs.controls.playback().unwrap().describe(), "Opus 48 kHz stereo");

        // PCM after it closes the decoder, and the facts say PCM again.
        player.pcm(Format::new(48_000, 2).unwrap(), &[1, 0, 1, 0]);
        assert_eq!(outputs.controls.playback().unwrap().describe(), "PCM 48 kHz stereo");
        assert_eq!(outputs.queues.lock().unwrap().len(), 1, "the same format keeps its stream");
    }

    #[test]
    fn a_codec_nobody_decodes_is_dropped_and_asked_for_once() {
        let outputs = fake();
        let mut player = Player::new(outputs.clone());
        let format = Format::new(44_100, 2).unwrap();
        player.packet(Codec::Aac, format, &[], &[1]);
        player.packet(Codec::Aac, format, &[], &[2]);
        assert!(outputs.queues.lock().unwrap().is_empty());
        assert!(!player.is_open());
        assert_eq!(outputs.controls.playback(), None);
    }

    #[test]
    fn the_far_ends_gain_outlives_its_streams() {
        let outputs = fake();
        let mut player = Player::new(outputs.clone());
        player.set_gain(0.5);
        player.pcm(Format::new(48_000, 1).unwrap(), &[0, 64]);
        let queue = Arc::clone(&outputs.queues.lock().unwrap()[0]);
        let mut out = [0i16; 1];
        // Primed first: pulled before its target, a queue plays silence.
        queue.write(&vec![0x4000; 48_000]);
        queue.pull(&mut out, queue.format());
        assert_eq!(out[0], 0x2000, "half of the first sample, 0x4000");
    }

    #[test]
    fn a_quiet_stream_closes_once_heard_and_a_close_says_so() {
        let outputs = fake();
        let mut player = Player::new(outputs.clone());
        let format = Format::new(48_000, 2).unwrap();
        let after = Duration::from_secs(2);
        let start = Instant::now();
        assert!(!player.close_if_quiet_at(start + after * 2, after), "nothing open");
        player.pcm(format, &[0; 4 * 4800]);
        // 100 ms queued, with nothing pulling it.
        assert!(player.latency() >= Duration::from_millis(100));
        let heard_by = player.heard_by.unwrap();
        assert!(!player.close_if_quiet_at(heard_by + after - Duration::from_millis(1), after));
        assert!(outputs.controls.playing());
        assert!(player.close_if_quiet_at(heard_by + after, after));
        assert!(!player.is_open());
        assert!(!outputs.controls.playing());

        player.packet(Codec::Opus, format, &[], &[1, 2, 3, 4]);
        assert!(outputs.controls.playback().is_some());
        player.close();
        assert_eq!(outputs.controls.playback(), None);
        assert!(!outputs.controls.playing());
    }
}
