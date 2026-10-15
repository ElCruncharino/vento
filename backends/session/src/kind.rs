// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! A backend as a client holds it — what it is ([`About`], from the
//! [`catalog`](crate::catalog)) beside how to start a session on it — and
//! [`Connect`], what a session is started with.

use crate::backend::Backend;
use crate::catalog::About;
use crate::events::Mailbox;
use crate::options::Options;
use common::video::Decoders;
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
    /// The XDG defaults, as the Linux client has them: `vento` under
    /// `$XDG_CONFIG_HOME`, `$XDG_STATE_HOME` and `$XDG_DATA_HOME`, each
    /// falling back to its place under `$HOME`.
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
    /// What the far end lists this machine as: a phone's model, a desktop's
    /// host name.
    pub client_name: &'a str,
    pub platform: Platform,
    /// What decodes a protocol's video, for the one whose picture is video
    /// (RustDesk): the phone's MediaCodec, or libavcodec on a desktop. `None`
    /// is a session with no picture, reported as such.
    pub decoders: Option<Arc<dyn Decoders>>,
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
}

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
    pub fn start(&self, connect: &Connect) -> Result<Arc<dyn Backend>, String> {
        if self.options.iter().any(|s| s.key == "AcceptBell") {
            connect.mailbox.accept_bell(connect.options.bool("AcceptBell"));
        }
        (self.create)(connect)
    }
}
