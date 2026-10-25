// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! A backend as a client holds it — what it is ([`About`], from the
//! [`catalog`](crate::catalog)) beside how to start a session on it — and
//! [`Connect`], what a session is started with.

use crate::backend::Backend;
use crate::catalog::About;
use crate::events::Mailbox;
use crate::options::Options;
use runtime::media::audio::Outputs;
use runtime::media::video::Decoders;
use std::path::PathBuf;
use std::sync::Arc;

/// Where a client keeps things on disk.
#[derive(Clone, Debug)]
pub struct Dirs {
    /// Where the client's own decisions are kept: the known hosts and
    /// RealVNC's identities, both of which outlive every session.
    pub config: PathBuf,
    /// What a session leaves behind that is not a decision: FreeRDP's config
    /// directory (which keeps nothing) and RealVNC's key, own identities and
    /// logs, each under a directory of its own backend.
    pub state: PathBuf,
    /// Where `libvncviewer.so` and `libc++_shared.so` out of the RealVNC
    /// viewer's APK are, for the one backend whose library is not in any
    /// build of this.
    pub realvnc_libs: PathBuf,
}

impl Dirs {
    /// The XDG defaults: `vento` under `$XDG_CONFIG_HOME`, `$XDG_STATE_HOME`
    /// and `$XDG_DATA_HOME`, each falling back to its place under `$HOME`.
    pub fn xdg() -> Dirs {
        let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
        let xdg = |var: &str, under: &str| {
            std::env::var_os(var)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join(under))
                .join("vento")
        };
        Dirs {
            config: xdg("XDG_CONFIG_HOME", ".config"),
            state: xdg("XDG_STATE_HOME", ".local/state"),
            realvnc_libs: xdg("XDG_DATA_HOME", ".local/share").join("realvnc"),
        }
    }
}

/// Which client a session is in, for the protocols whose far end is told:
/// RustDesk's login names the platform, and RDP's general capability set
/// carries a major type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    Android,
    Linux,
}

/// What a session is started with.
pub struct Connect<'a> {
    pub address: &'a str,
    /// Empty for none. For the RDP backends, `DOMAIN\user` and `user@domain`
    /// are split here as the Java backends split them, unless `domain` says
    /// otherwise.
    pub user: &'a str,
    pub domain: &'a str,
    /// `None` prompts; an empty password is not a password, for the reason
    /// the RealVNC backend found first — something that looks like a stored
    /// secret and is not one fails the connection instead of asking for the
    /// real one — and callers pass `None` for it.
    pub password: Option<&'a str>,
    pub options: &'a Options,
    pub mailbox: Arc<Mailbox>,
    pub dirs: &'a Dirs,
    /// The size of the screen this session will be drawn on, where there is
    /// one, which is what the RDP backends' `DesktopSize=device` asks for.
    pub screen: Option<(u32, u32)>,
    /// The size this address's desktop ended up at last time, where the
    /// client keeps that note — the phone does, per address, for both RDP
    /// backends — which is what the RDP backends' `DesktopSize=remote` asks
    /// for.
    pub last_size: Option<(u32, u32)>,
    /// What this address's server was last accepted as, where the client
    /// keeps that note: the identity a backend handed over through
    /// [`Backend::saved_identity`] after a session at this address, handed
    /// back so that it does not ask again about a server it recognises.
    /// Only for a backend whose [`About::saves_identity`] says so — RealVNC's
    /// core is the one library with identities of its own — and `None` for
    /// the others, whose far ends the client pins.
    pub identity: Option<&'a str>,
    /// What the far end lists this machine as: a phone's model, a desktop's
    /// host name.
    pub client_name: &'a str,
    pub platform: Platform,
    /// What decodes a protocol's video, for the one whose picture is video
    /// (RustDesk) and the two that have a video encoding (TigerVNC's and
    /// FreeRDP's H.264, through `vento/video.h`): the phone's MediaCodec, or
    /// libavcodec on a desktop. `None` is a RustDesk session with no
    /// picture, reported as such, and a TigerVNC or FreeRDP one that never
    /// offers H.264.
    pub decoders: Option<Arc<dyn Decoders>>,
    /// Where the far end's sound goes, for the backends that have sound
    /// (TigerVNC's QEMU audio and FreeRDP's rdpsnd through `vento/audio.h`,
    /// SPICE's, IronRDP's and RustDesk's in Rust): the phone's AAudio, or on
    /// a desktop PipeWire (ALSA where no daemon answers), one per session so
    /// that its controls — the switch and the mute a window sets — are this
    /// session's alone. `None` is a session whose sound is dropped.
    pub audio: Option<Arc<dyn Outputs>>,
}

impl Connect<'_> {
    /// `DOMAIN\user`, `user@domain` or a bare name, as `(domain, user)`,
    /// with an explicit domain winning over one in the name.
    pub fn domain_and_user(&self) -> (String, String) {
        if !self.domain.is_empty() {
            return (self.domain.to_string(), self.user.to_string());
        }
        crate::credentials::split_domain(self.user)
    }
}

/// Twenty seconds, as every Java backend has it: long enough for a server
/// to finish a handshake, short enough that a mistyped address is not a
/// spinner that never stops.
pub const CONNECT_TIMEOUT_MS: i32 = 20_000;

/// One backend: what it is, and how to start a session on it. Derefs to its
/// [`About`], so `kind.id` and `kind.options` read as they always have.
pub struct Kind {
    pub about: &'static About,
    /// The one call that proves the whole path: what the library says it is.
    pub version: fn() -> String,
    /// The backend's own constructor; sessions are started with
    /// [`Kind::start`], which wraps it.
    pub create: fn(&Connect) -> Result<Arc<dyn Backend>, String>,
    /// Forgets what the backend's library keeps of its own about the server
    /// `address` names ([`trust::same_server`](crate::trust::same_server)),
    /// under `dirs`, for a client forgetting it: a line for the person per
    /// thing removed. Never connects anywhere. `None` for a backend whose
    /// library keeps nothing of a server's, which is all but RealVNC's (whose
    /// core remembers the servers it accepted). The phone's counterpart is
    /// `BackendProvider.forgetHosts`.
    pub forget: Option<Forget>,
}

/// [`Kind::forget`]: the client's directories and the address, to a line per
/// thing removed, or why the backend's record could not be changed.
pub type Forget = fn(&Dirs, &str) -> Result<Vec<String>, String>;

impl std::ops::Deref for Kind {
    type Target = About;

    fn deref(&self) -> &About {
        self.about
    }
}

impl Kind {
    /// Starts a session on this backend: its `create`, after what is the
    /// same for every backend that has the option. `AcceptBell` is that: the
    /// adapters and the libraries behind the vtable hand a bell over whatever
    /// the option says, so it is dropped at the mailbox instead, once for
    /// all of them. (RealVNC's core is told as well, since its library has a
    /// switch of its own.) The option is not live, as the catalog has it,
    /// so this is once per session.
    ///
    /// The session's sound is likewise said for every backend at once: the
    /// session's audio controls are told to say it through the mailbox, as
    /// [`Event::Sound`](crate::events::Event::Sound), whichever backend's
    /// streams it is that start and stop and whichever offers the switch.
    pub fn start(&self, connect: &Connect) -> Result<Arc<dyn Backend>, String> {
        if self.options.iter().any(|s| s.key == "AcceptBell") {
            connect.mailbox.accept_bell(connect.options.bool("AcceptBell"));
        }
        if let Some(audio) = &connect.audio {
            // Weak, so that a stream a backend leaks past its session does
            // not keep the mailbox alive with it.
            let mailbox = Arc::downgrade(&connect.mailbox);
            audio.controls().on_sound(Box::new(move |sound| {
                if let Some(mailbox) = mailbox.upgrade() {
                    mailbox.on_sound(sound);
                }
            }));
        }
        (self.create)(connect)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::Event;
    use runtime::media::audio::{Controls, Device, Format, Queue};

    struct Quiet;
    impl Device for Quiet {}

    struct Fake(Arc<Controls>);

    impl Outputs for Fake {
        fn controls(&self) -> &Arc<Controls> {
            &self.0
        }

        fn device(&self, _queue: Arc<Queue>) -> Result<Box<dyn Device>, String> {
            Ok(Box::new(Quiet))
        }
    }

    #[test]
    fn a_session_says_its_sound_and_its_switch() {
        static KIND: Kind = Kind {
            about: &crate::catalog::FIXTURE,
            version: String::new,
            create: |_| Err("not here".into()),
            forget: None,
        };
        let audio = Arc::new(Fake(Arc::new(Controls::new())));
        let mailbox = Mailbox::new(None);
        let options = Options::default();
        let dirs = Dirs {
            config: PathBuf::new(),
            state: PathBuf::new(),
            realvnc_libs: PathBuf::new(),
        };
        let connect = Connect {
            address: "",
            user: "",
            domain: "",
            password: None,
            options: &options,
            mailbox: Arc::clone(&mailbox),
            dirs: &dirs,
            screen: None,
            last_size: None,
            identity: None,
            client_name: "",
            platform: Platform::Linux,
            decoders: None,
            audio: Some(Arc::clone(&audio) as Arc<dyn Outputs>),
        };
        assert!(KIND.start(&connect).is_err());
        let mut stream = audio.open(Format::new(48_000, 2).unwrap()).unwrap();
        stream.write(&[0, 0]);
        let playing = |playing: bool| {
            Some(Event::Sound(runtime::media::audio::Sound {
                switch: playing.then_some(runtime::media::audio::Switch::Mute),
                enabled: true,
                muted: false,
                playing,
            }))
        };
        assert_eq!(mailbox.take(), playing(true));
        drop(stream);
        assert_eq!(mailbox.take(), playing(false));
        audio.controls().offer_enable(false, Box::new(|_| {}));
        assert!(matches!(mailbox.take(), Some(Event::Sound(s)) if s.switch == Some(runtime::media::audio::Switch::Enable) && !s.enabled));
    }
}
