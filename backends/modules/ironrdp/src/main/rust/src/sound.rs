// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The server's sound: IronRDP's rdpsnd channel, with a handler that plays
//! what it is sent on the session's output (`common::audio`).
//!
//! Their channel does the protocol — the format exchange, the training, a
//! confirmation for every wave — and asks its handler for the formats to
//! offer before it has seen the server's, keeping only those the server
//! also listed, field for field. What this end can play depends on the
//! server's list instead (AAC and Opus are offered only where the platform
//! decodes them, and only as the server describes them), so [`Sound`] stands
//! in front of the channel and starts a fresh one, with a handler offering
//! exactly the playable part of the server's list, each time a list arrives.
//! The channel's state is reset by a new list anyway, which is why that
//! costs nothing.
//!
//! Waves are played as they come: PCM as it is, AAC and Opus through the
//! platform's decoders (MediaCodec on the phone), in a [`Player`] the
//! session shares with this handler. Windows picks AAC when it is offered,
//! as it does from FreeRDP's device. The server's Close says nothing about
//! when the sound will have been heard, so the stream is closed instead two
//! seconds after the last wave was ([`QUIET`]), from the session's loop.

use common::audio::decode::Codec;
use common::audio::{Format, Outputs, Player};
use ironrdp_core::{Decode as _, ReadCursor, impl_as_any};
use ironrdp_pdu::PduResult;
use ironrdp_pdu::gcc::{ChannelName, ChannelOptions};
use ironrdp_rdpsnd::client::{Rdpsnd, RdpsndClientHandler};
use ironrdp_rdpsnd::pdu::{AudioFormat, PitchPdu, ServerAudioOutputPdu, VolumePdu, WaveFormat};
use ironrdp_svc::{CompressionCondition, SvcClientProcessor, SvcMessage, SvcProcessor};
use std::borrow::Cow;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// How long a stream stays open after its last wave was heard: the gap
/// between one sound and the next it rides over, as FreeRDP's device here
/// waits.
pub const QUIET: Duration = Duration::from_secs(2);

/// `SNDC_FORMATS` and `SNDC_WAVE`, the first byte of the PDUs this has to
/// tell apart before the channel sees them.
const SNDC_FORMATS: u8 = 0x07;
const SNDC_WAVE: u8 = 0x02;

/// The rdpsnd channel as the connector holds it: theirs, made again for each
/// format list the server sends.
pub struct Sound {
    channel: Rdpsnd,
    player: Arc<Mutex<Player>>,
    /// The session's output, whose switch this offers.
    outputs: Arc<dyn Outputs>,
    /// A pre-version-8 WaveInfo has been passed on, and the next payload is
    /// its wave, which has no header to read: it must not be mistaken for a
    /// PDU of its own here.
    wave_follows: bool,
}

impl_as_any!(Sound);

// The channel traits ask for `Debug`, and a player has nothing worth
// printing.
impl std::fmt::Debug for Sound {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Sound").field("channel", &self.channel).finish_non_exhaustive()
    }
}

impl Sound {
    /// The channel, playing on `outputs`; the player is the session's to
    /// close when it goes quiet ([`Player::close_if_quiet`]).
    pub fn new(outputs: Arc<dyn Outputs>) -> (Sound, Arc<Mutex<Player>>) {
        let player = Arc::new(Mutex::new(Player::new(Arc::clone(&outputs))));
        let sound = Sound {
            channel: Rdpsnd::new(Box::new(Waves::new(Vec::new(), Arc::clone(&player)))),
            player: Arc::clone(&player),
            outputs,
            wave_follows: false,
        };
        (sound, player)
    }
}

impl SvcProcessor for Sound {
    fn channel_name(&self) -> ChannelName {
        Rdpsnd::NAME
    }

    fn compression_condition(&self) -> CompressionCondition {
        self.channel.compression_condition()
    }

    fn channel_options(&self) -> ChannelOptions {
        self.channel.channel_options()
    }

    fn process(&mut self, payload: &[u8]) -> PduResult<Vec<SvcMessage>> {
        if std::mem::take(&mut self.wave_follows) {
            return self.channel.process(payload);
        }
        match payload.first() {
            Some(&SNDC_FORMATS) => {
                if let Ok(ServerAudioOutputPdu::AudioFormat(list)) =
                    ServerAudioOutputPdu::decode(&mut ReadCursor::new(payload))
                {
                    let player = self.player.lock().unwrap();
                    let offered = playable(&list.formats, &player);
                    drop(player);
                    // A server that lists a format this end plays may send
                    // sound from now on, and nothing in rdpsnd asks it to
                    // stop during a session, so the session's switch is a
                    // mute, there from this list rather than from the first
                    // wave.
                    if !offered.is_empty() {
                        self.outputs.controls().offer_mute();
                    }
                    self.channel = Rdpsnd::new(Box::new(Waves::new(offered, Arc::clone(&self.player))));
                }
            }
            // The WaveInfo alone, with its wave still to come; a WaveInfo
            // with its wave appended is finished in the one payload.
            Some(&SNDC_WAVE) => self.wave_follows = payload.len() <= WAVE_INFO,
            _ => {}
        }
        self.channel.process(payload)
    }
}

impl SvcClientProcessor for Sound {}

/// A WaveInfo PDU's length: the four-byte header, then twelve of body.
const WAVE_INFO: usize = 16;

/// What of the server's list this end plays, in the server's order, each
/// as the server wrote it so that the channel's match keeps it — and logged,
/// since which one the server then picks is the thing worth knowing.
fn playable(formats: &[AudioFormat], player: &Player) -> Vec<AudioFormat> {
    let mut offered = Vec::new();
    for f in formats {
        let ours = kind_of(f, player).is_some();
        log::info!(
            "sound: the server offers {} {} Hz {} ch {}-bit ({})",
            name_of(f.format),
            f.n_samples_per_sec,
            f.n_channels,
            f.bits_per_sample,
            if ours { "played here" } else { "not played here" }
        );
        if ours {
            offered.push(f.clone());
        }
    }
    offered
}

/// How a format is played: as PCM, or through a decoder, with its setup.
#[derive(Debug, PartialEq, Eq)]
enum Kind {
    Pcm(Format),
    Coded(Codec, Format, Vec<u8>),
}

fn kind_of(f: &AudioFormat, player: &Player) -> Option<Kind> {
    let format = Format::new(f.n_samples_per_sec, usize::from(f.n_channels))?;
    let extra = f.data.as_deref().unwrap_or_default();
    match f.format {
        WaveFormat::PCM if f.bits_per_sample == 16 => Some(Kind::Pcm(format)),
        // Raw AAC LC frames, which is what Windows sends for the format it
        // describes with no extra bytes; extra bytes would be a
        // HEAACWAVEINFO, a profile nobody here has been sent.
        WaveFormat::AAC_MS if extra.is_empty() && player.decodable(Codec::Aac) => {
            common::audio::decode::aac_config(format).map(|_| Kind::Coded(Codec::Aac, format, Vec::new()))
        }
        // Opus packets, set up by an identification header where the server
        // gives one and by the format alone where not.
        WaveFormat::OPUS
            if format.channels <= 2
                && (extra.is_empty() || extra.starts_with(b"OpusHead"))
                && player.decodable(Codec::Opus) =>
        {
            Some(Kind::Coded(Codec::Opus, format, extra.to_vec()))
        }
        _ => None,
    }
}

fn name_of(format: WaveFormat) -> String {
    match format {
        WaveFormat::PCM => "PCM".into(),
        WaveFormat::AAC_MS => "AAC_MS".into(),
        WaveFormat::OPUS => "Opus".into(),
        WaveFormat::ADPCM => "ADPCM".into(),
        WaveFormat::DVI_ADPCM => "DVI_ADPCM".into(),
        WaveFormat::ALAW => "A-law".into(),
        WaveFormat::GSM610 => "GSM610".into(),
        other => format!("format {other}"),
    }
}

/// The channel's handler: the formats offered, and the waves played.
struct Waves {
    formats: Vec<AudioFormat>,
    player: Arc<Mutex<Player>>,
    /// The format the last wave was in, so that the server's choice is said
    /// once rather than per wave.
    said: Option<AudioFormat>,
}

impl Waves {
    fn new(formats: Vec<AudioFormat>, player: Arc<Mutex<Player>>) -> Waves {
        Waves {
            formats,
            player,
            said: None,
        }
    }
}

impl std::fmt::Debug for Waves {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Waves").field("formats", &self.formats).finish_non_exhaustive()
    }
}

impl RdpsndClientHandler for Waves {
    fn get_formats(&self) -> &[AudioFormat] {
        &self.formats
    }

    fn wave(&mut self, format: &AudioFormat, _ts: u32, data: Cow<'_, [u8]>) {
        let mut player = self.player.lock().unwrap();
        let Some(kind) = kind_of(format, &player) else {
            return;
        };
        if self.said.as_ref() != Some(format) {
            log::info!(
                "sound: the server sends {} {} Hz {} ch",
                name_of(format.format),
                format.n_samples_per_sec,
                format.n_channels
            );
            self.said = Some(format.clone());
        }
        match kind {
            Kind::Pcm(f) => player.pcm(f, &data),
            Kind::Coded(codec, f, config) => player.packet(codec, f, &config, &data),
        }
    }

    /// Linear, a `u16` a channel: the louder is the stream's gain, as
    /// FreeRDP's device here takes it.
    fn set_volume(&mut self, volume: VolumePdu) {
        let loudest = volume.volume_left.max(volume.volume_right);
        log::info!("sound: the server sets its volume to {loudest} of 65535");
        self.player.lock().unwrap().set_gain(f32::from(loudest) / f32::from(u16::MAX));
    }

    fn set_pitch(&mut self, _pitch: PitchPdu) {}

    /// The server's Close, a new format list, or the channel going: none of
    /// them says the sound has been heard, so the stream is left to close
    /// when it has gone quiet, or with the session.
    fn close(&mut self) {
        self.said = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::audio::decode::{Decoder, Decoders};
    use common::audio::{Controls, Device, Queue};

    struct Quiet;
    impl Device for Quiet {}

    /// An output that decodes AAC and nothing else.
    struct Fake(Arc<Controls>);

    impl Outputs for Fake {
        fn controls(&self) -> &Arc<Controls> {
            &self.0
        }

        fn device(&self, _queue: Arc<Queue>) -> Result<Box<dyn Device>, String> {
            Ok(Box::new(Quiet))
        }

        fn decoders(&self) -> Option<&dyn Decoders> {
            Some(self)
        }
    }

    impl Decoders for Fake {
        fn decodable(&self, codec: Codec) -> bool {
            codec == Codec::Aac
        }

        fn decoder(&self, _codec: Codec, _format: Format, _config: &[u8]) -> Result<Box<dyn Decoder>, String> {
            Err("not in a test".into())
        }
    }

    fn format(format: WaveFormat, rate: u32, channels: u16, bits: u16, data: Option<Vec<u8>>) -> AudioFormat {
        AudioFormat {
            format,
            n_channels: channels,
            n_samples_per_sec: rate,
            n_avg_bytes_per_sec: rate * u32::from(channels) * u32::from(bits) / 8,
            n_block_align: channels * bits / 8,
            bits_per_sample: bits,
            data,
        }
    }

    #[test]
    fn what_is_offered_is_the_playable_part_of_the_servers_list() {
        let player = Player::new(Arc::new(Fake(Arc::new(Controls::new()))));
        // Windows 10's list, in short.
        let windows = [
            format(WaveFormat::AAC_MS, 44_100, 2, 16, None),
            format(WaveFormat::AAC_MS, 44_100, 2, 16, Some(vec![1, 2])),
            format(WaveFormat::PCM, 44_100, 2, 16, None),
            format(WaveFormat::ADPCM, 44_100, 2, 4, Some(vec![0; 32])),
            format(WaveFormat::PCM, 22_050, 1, 8, None),
            format(WaveFormat::OPUS, 48_000, 2, 16, None),
            format(WaveFormat::GSM610, 8_000, 1, 0, None),
        ];
        let offered = playable(&windows, &player);
        assert_eq!(offered, [windows[0].clone(), windows[2].clone()]);
        assert_eq!(
            kind_of(&windows[0], &player),
            Some(Kind::Coded(Codec::Aac, Format::new(44_100, 2).unwrap(), Vec::new()))
        );
        assert_eq!(kind_of(&windows[2], &player), Some(Kind::Pcm(Format::new(44_100, 2).unwrap())));
        // Without decoders, PCM alone.
        let plain = Player::new(Arc::new(common::audio::NoOutputs::new()));
        assert_eq!(playable(&windows, &plain), [windows[2].clone()]);
    }

    #[test]
    fn a_wave_after_its_info_is_not_read_as_a_pdu() {
        let (mut sound, _player) = Sound::new(Arc::new(Fake(Arc::new(Controls::new()))));
        // A WaveInfo alone: the next payload is its wave, whatever its first
        // byte says.
        let mut info = vec![SNDC_WAVE, 0, 12, 0];
        info.extend([0; 12]);
        let _ = sound.process(&info);
        assert!(sound.wave_follows);
        let _ = sound.process(&[SNDC_FORMATS, 0, 0, 0, 0, 0, 0, 0]);
        assert!(!sound.wave_follows);
    }

    #[test]
    fn a_list_with_something_playable_offers_a_mute() {
        use ironrdp_rdpsnd::pdu::{ServerAudioFormatPdu, Version};
        let list = |formats| {
            ironrdp_core::encode_vec(&ServerAudioOutputPdu::AudioFormat(ServerAudioFormatPdu {
                version: Version::V8,
                formats,
            }))
            .unwrap()
        };
        let controls = Arc::new(Controls::new());
        let (mut sound, _player) = Sound::new(Arc::new(Fake(Arc::clone(&controls))));
        // Nothing this end plays: nothing will be sent, so no switch.
        let _ = sound.process(&list(vec![format(WaveFormat::GSM610, 8_000, 1, 0, None)]));
        assert_eq!(controls.sound().switch, None);
        let _ = sound.process(&list(vec![format(WaveFormat::PCM, 44_100, 2, 16, None)]));
        let offered = controls.sound();
        assert_eq!((offered.switch, offered.playing), (Some(common::audio::Switch::Mute), false));
    }
}
