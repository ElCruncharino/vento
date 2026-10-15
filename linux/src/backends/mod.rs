// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The eight backends, each as a [`Kind`]: what the picker and `--list` show,
//! the options it takes, and how to start a session on it.
//!
//! Four are the Rust protocol crates, each adapted in a file of its own that
//! is the twin of that crate's `bindings/mod.rs` — the same config built from
//! the same option values, over a `common::Session` fed into a mailbox
//! instead of into Java. Four are C or C++ libraries behind the vtable in
//! `vento/backend.h`, and `c.rs` is the one adapter over all of them, with
//! the per-backend part being the config and the live options. The order is
//! `BackendProvider.order()`'s, which is the picker's.

pub mod c;
pub mod ironrdp;
pub mod rfb;
pub mod rustdesk;
pub mod spice;

use crate::backend::{Backend, Credentials};
use crate::events::Mailbox;
use crate::options::{Options, Spec};
use std::path::PathBuf;
use std::sync::Arc;

/// Where this client keeps things on disk, with the XDG defaults.
#[derive(Clone, Debug)]
pub struct Dirs {
    /// Where this client's own decisions are kept: the [known
    /// hosts](crate::known_hosts) and RealVNC's identities, both of which
    /// outlive every session.
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

impl Default for Dirs {
    fn default() -> Dirs {
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
}

impl Connect<'_> {
    /// `DOMAIN\user`, `user@domain` or a bare name, as `(domain, user)`,
    /// with an explicit domain winning over one in the name.
    pub fn domain_and_user(&self) -> (String, String) {
        if !self.domain.is_empty() {
            return (self.domain.to_string(), self.user.to_string());
        }
        split_domain(self.user)
    }
}

/// `DOMAIN\user`, `user@domain` or a bare name, as `(domain, user)`. Both
/// forms, because both are typed: the backslash is what Windows shows and
/// the `@` is what a UPN looks like.
pub fn split_domain(value: &str) -> (String, String) {
    let name = trim(value);
    if let Some((domain, user)) = name.split_once('\\') {
        return (domain.to_string(), user.to_string());
    }
    if let Some((user, domain)) = name.split_once('@')
        && !user.is_empty()
    {
        return (domain.to_string(), user.to_string());
    }
    (String::new(), name.to_string())
}

/// A typed name without what Java's `trim()` takes off it — ASCII controls
/// and the space — so that the phone and this client read one answer alike.
fn trim(typed: &str) -> &str {
    typed.trim_matches(|c: char| c <= ' ')
}

/// Credentials as the Java prompts deliver them: an empty user name means
/// the one the session was created with. A name is read trimmed, so one that
/// is only spaces is an empty one (`backends/vectors/credentials.json`).
pub fn fill_user(mut credentials: Credentials, user: &str) -> Credentials {
    let typed = trim(&credentials.user);
    credentials.user = if typed.is_empty() { user } else { typed }.to_string();
    credentials
}

/// An RDP prompt's answer as the `(domain, user)` to send, against the
/// session's own `domain` and `user` (already split). An empty name is the
/// session's, domain and all; a name typed with a domain, or a domain given
/// beside it, is that one; and a bare name keeps the session's domain, since
/// the prompt has no box for one and `--domain CORP` answered with `alice`
/// means `CORP\alice`. A local account on a machine with a domain is
/// `.\alice`, as Windows writes it.
pub fn rdp_identity(answer: &Credentials, domain: &str, user: &str) -> (String, String) {
    let typed = trim(&answer.user);
    if typed.is_empty() {
        return (domain.to_string(), user.to_string());
    }
    if !answer.domain.is_empty() {
        return (answer.domain.clone(), typed.to_string());
    }
    match split_domain(typed) {
        (typed, name) if typed.is_empty() => (domain.to_string(), name),
        split => split,
    }
}

/// What a backend's certificates are pinned against in the [known
/// hosts](crate::known_hosts), and how a first one is taken.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pinned {
    /// The address as written for all but RustDesk, whose key says which
    /// network the id is on.
    pub key: String,
    /// Whether a fingerprint never seen is pinned without asking: RustDesk's
    /// alone, since its key arrives signed by the rendezvous server for the
    /// id that was dialled and a question would have nothing to check it by.
    pub first_sight_silent: bool,
}

pub fn pinned_as(kind: &Kind, address: &str, options: &Options) -> Pinned {
    if kind.id == rustdesk::KIND.id {
        return Pinned {
            key: rustdesk::pinned_as(address, options),
            first_sight_silent: true,
        };
    }
    Pinned {
        key: address.to_string(),
        first_sight_silent: false,
    }
}

/// What the far end lists this machine as, where a phone says its model.
pub fn client_name() -> String {
    std::fs::read_to_string("/etc/hostname")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "vento".to_string())
}

/// Twenty seconds, as every Java backend has it: long enough for a server
/// to finish a handshake, short enough that a mistyped address is not a
/// spinner that never stops.
pub const CONNECT_TIMEOUT_MS: i32 = 20_000;

/// One backend, described.
pub struct Kind {
    /// `BackendProvider.id()`: what the command line names it by.
    pub id: &'static str,
    pub name: &'static str,
    pub short_name: &'static str,
    pub description: &'static str,
    pub options: &'static [Spec],
    /// The one call that proves the whole path: what the library says it is.
    pub version: fn() -> String,
    /// The backend's own constructor; sessions are started with
    /// [`Kind::start`], which wraps it.
    pub create: fn(&Connect) -> Result<Arc<dyn Backend>, String>,
}

/// Every backend this binary has, in the picker's order. RealVNC is here
/// only when `build.rs` found clang to build its hosting layer with.
impl Kind {
    /// Starts a session on this backend: its `create`, after what is the
    /// same for every backend that has the option. `AcceptBell` is that: the
    /// adapters and the libraries behind the vtable hand a bell over whatever
    /// the option says, so it is dropped at the mailbox instead, once for
    /// all of them. (RealVNC's core is told as well, since its library has a
    /// switch of its own.) The option is not live, as the Providers have it,
    /// so this is once per session.
    pub fn start(&self, connect: &Connect) -> Result<Arc<dyn Backend>, String> {
        if self.options.iter().any(|s| s.key == "AcceptBell") {
            connect.mailbox.accept_bell(connect.options.bool("AcceptBell"));
        }
        (self.create)(connect)
    }
}

pub fn all() -> Vec<&'static Kind> {
    let mut kinds = vec![
        &rfb::KIND,
        &c::libvnc::KIND,
        &c::tigervnc::KIND,
        &c::freerdp::KIND,
        &ironrdp::KIND,
    ];
    #[cfg(realvnc)]
    kinds.push(&c::realvnc::KIND);
    kinds.push(&spice::KIND);
    kinds.push(&rustdesk::KIND);
    kinds
}

pub fn find(id: &str) -> Option<&'static Kind> {
    all().into_iter().find(|k| k.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_domain_is_split_either_way_it_is_typed() {
        assert_eq!(split_domain("CORP\\alice"), ("CORP".into(), "alice".into()));
        assert_eq!(split_domain("alice@corp.example"), ("corp.example".into(), "alice".into()));
        assert_eq!(split_domain(" alice "), ("".into(), "alice".into()));
        assert_eq!(split_domain("@odd"), ("".into(), "@odd".into()));
    }

    #[test]
    fn an_rdp_answer_keeps_the_session_domain_unless_it_names_one() {
        let answer = |user: &str, domain: &str| Credentials {
            user: user.into(),
            domain: domain.into(),
            password: "pw".into(),
        };
        let id = |a: &Credentials| rdp_identity(a, "CORP", "proto");
        assert_eq!(id(&answer("", "")), ("CORP".into(), "proto".into()));
        assert_eq!(id(&answer("proto", "")), ("CORP".into(), "proto".into()));
        assert_eq!(id(&answer("alice", "")), ("CORP".into(), "alice".into()));
        assert_eq!(id(&answer("LAB\\alice", "")), ("LAB".into(), "alice".into()));
        assert_eq!(id(&answer("alice@lab.example", "")), ("lab.example".into(), "alice".into()));
        assert_eq!(id(&answer(".\\alice", "")), (".".into(), "alice".into()));
        assert_eq!(id(&answer("alice", "LAB")), ("LAB".into(), "alice".into()));
        assert_eq!(rdp_identity(&answer("alice", ""), "", "proto"), ("".into(), "alice".into()));
    }

    #[test]
    fn a_pin_is_the_address_as_written_except_for_a_rustdesk_id() {
        let rdp = pinned_as(&ironrdp::KIND, "Host:3389", &Options::defaults(ironrdp::KIND.options));
        assert_eq!(rdp.key, "Host:3389");
        assert!(!rdp.first_sight_silent);
        let mut o = Options::defaults(rustdesk::KIND.options);
        let id = pinned_as(&rustdesk::KIND, "123456789", &o);
        assert_eq!(id.key, "123456789@rs-ny.rustdesk.com");
        assert!(id.first_sight_silent);
        o.set("RendezvousServer", " hbbs.example ");
        assert_eq!(pinned_as(&rustdesk::KIND, "123456789", &o).key, "123456789@hbbs.example");
    }

    #[test]
    fn every_kind_has_a_distinct_id_and_defaults_that_pass_its_own_checks() {
        let ids: Vec<&str> = all().iter().map(|k| k.id).collect();
        let mut unique = ids.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(ids.len(), unique.len());
        for kind in all() {
            let options = Options::defaults(kind.options);
            for spec in kind.options {
                assert!(spec.check(options.get(spec.key)).is_ok(), "{}/{}", kind.id, spec.key);
            }
        }
    }
}
