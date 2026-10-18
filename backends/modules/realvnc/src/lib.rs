// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! RealVNC's viewer core as a [`Backend`](api::backend::Backend):
//! `vento_realvnc_config` and `RealVncBackend`'s option handling and rows —
//! what the Linux client holds. The phone's RealVNC backend is the module's
//! Java over the viewer's own JNI, so on any other target than Linux this
//! crate is empty.
//!
//! The library, `libvento_realvnc.so`, is opened when it is first needed
//! rather than linked: the client's build makes it only where clang is on
//! the path, and decides that after this crate is built, so the backend is
//! always listed and says why it cannot start where the library is not
//! there ([`library`]). It is found by name, as the other C backends'
//! libraries are linked by name — through the binary's rpath, which points
//! at the client's build tree, or `LD_LIBRARY_PATH` — and once open it stays
//! open, since the core it hosts never lets go of a session.
//!
//! Nothing of RealVNC's is in the tree: the two libraries the session loads
//! come from the viewer's APK under `library_dir`, hashes verified by the
//! session, and what the core persists goes under `state_dir` — both from
//! [`Dirs`](api::kind::Dirs), with XDG defaults, and overridable from the
//! client's command line. The identity the core asks to have remembered is
//! the client's to keep, handed out through
//! [`Backend::saved_identity`](api::backend::Backend::saved_identity) and
//! back in as [`Connect::identity`].

#![cfg(target_os = "linux")]

use api::backend::{Backend, ConnectionFact, Field, Traffic};
use api::c::{Behaviour, CSession, Security, start};
use api::kind::{Connect, Kind};
use api::options::Options;
use std::ffi::CString;
use std::os::raw::c_void;
use std::sync::OnceLock;

/// `vento/realvnc.h`, as `build.rs` generated it, over the shared
/// declarations it is written against: its types, and its functions as the
/// fields of a [`Library`](ffi::Library) that looked each of them up.
mod ffi {
    #![allow(non_camel_case_types, dead_code)]
    pub use api::c::ffi::*;
    include!(concat!(env!("OUT_DIR"), "/bindings.rs"));
}

/// The library's name, as the client's CMake build writes it.
const LIBRARY: &str = "libvento_realvnc.so";

/// The library, opened the first time anything asks: the error, kept as
/// well, is what every later ask is told. With `RTLD_GLOBAL`, as a library
/// the binary was linked against would be, so that the hosting layer's
/// lookups in the process's global scope see what they saw when it was.
fn library() -> Result<&'static ffi::Library, &'static str> {
    static LOADED: OnceLock<Result<ffi::Library, String>> = OnceLock::new();
    LOADED.get_or_init(|| open(LIBRARY)).as_ref().map_err(String::as_str)
}

fn open(name: &str) -> Result<ffi::Library, String> {
    use libloading::os::unix::{Library, RTLD_GLOBAL, RTLD_NOW};
    // Opening it runs its constructors, which are the hosting layer's and
    // the C++ runtime's, and looking up the header's functions reads them:
    // the library is one the client's own build made.
    unsafe {
        let library = Library::open(Some(name), RTLD_NOW | RTLD_GLOBAL)
            .map_err(|e| format!("{e} (the Linux client's build makes it only with clang on the path)"))?;
        ffi::Library::from_library(library)
            .map_err(|e| format!("{name} is not the library vento/realvnc.h describes: {e}"))
    }
}

/// The library a session was started over. A [`CSession`] of this backend
/// exists only once [`create`] has opened it, and it is never closed.
fn loaded() -> &'static ffi::Library {
    library().expect("a RealVNC session is started only over the opened library")
}

pub static KIND: Kind = Kind {
    about: &api::catalog::REALVNC,
    version: || match library() {
        Ok(library) => unsafe { api::c::string(library.vento_realvnc_version()) },
        Err(why) => format!("not available: {why}"),
    },
    create,
};

static BEHAVIOUR: Behaviour = Behaviour {
    info,
    // Received only: the core counts one direction and has no count of the
    // other anywhere, and a zero would be a claim.
    sent_counted: false,
    set_option,
    rdp_domain: false,
};

fn create(c: &Connect) -> Result<std::sync::Arc<dyn Backend>, String> {
    let library = library().map_err(|why| format!("RealVNC is not available: {why}"))?;
    let o = c.options;
    // The provider's defaults, which are not all zero; then the rows over
    // them, as `RealVncBackend.connect` hands the whole options map over.
    let mut config = ffi::vento_realvnc_config::default();
    unsafe { library.vento_realvnc_config_init(&mut config) };

    let state = c.dirs.state.join("realvnc");
    std::fs::create_dir_all(&state).map_err(|e| format!("the state directory: {e}"))?;

    let address = CString::new(c.address).map_err(|_| "address has a NUL")?;
    let user = (!c.user.is_empty()).then(|| CString::new(c.user).unwrap_or_default());
    let password = c.password.and_then(|p| CString::new(p).ok());
    let quality = CString::new(o.get("Quality")).unwrap_or_default();
    let encoding = CString::new(o.get("PreferredEncoding")).unwrap_or_default();
    // NULL follows the quality, as on the phone, unless a depth was chosen.
    let colour = o
        .is_set("ColorLevel")
        .then(|| CString::new(o.get("ColorLevel")).unwrap_or_default());
    let encryption = CString::new(o.get("Encryption")).unwrap_or_default();
    let library_dir = CString::new(c.dirs.realvnc_libs.to_string_lossy().into_owned())
        .map_err(|_| "the library path has a NUL")?;
    let state_dir =
        CString::new(state.to_string_lossy().into_owned()).map_err(|_| "the state path has a NUL")?;
    // What this server was last accepted as, as `RealVncBackend` hands it
    // back: with it the core's security prompt is silent for a server it
    // recognises. The client keeps it, and a new one is its to keep too.
    let identity = c.identity.and_then(|i| CString::new(i).ok());

    config.address = address.as_ptr();
    config.user_name = user.as_ref().map_or(std::ptr::null(), |u| u.as_ptr());
    config.password = password.as_ref().map_or(std::ptr::null(), |p| p.as_ptr());
    config.quality = quality.as_ptr();
    config.preferred_encoding = encoding.as_ptr();
    config.color_level = colour.as_ref().map_or(std::ptr::null(), |c| c.as_ptr());
    config.view_only = o.bool("ViewOnly");
    config.shared = o.bool("Shared");
    config.auto_reconnect = o.bool("AutoReconnect");
    config.enable_udp = o.bool("EnableUdpRfb");
    config.encryption = encryption.as_ptr();
    config.warn_unencrypted = o.bool("WarnUnencrypted");
    config.dot_when_no_cursor = o.bool("DotWhenNoCursor");
    config.accept_bell = o.bool("AcceptBell");
    config.keep_alive_interval_s = o.int("KeepAliveInterval", 30) as i32;
    config.keep_alive_response_timeout_s = o.int("KeepAliveResponseTimeout", 30) as i32;
    config.library_dir = library_dir.as_ptr();
    config.state_dir = state_dir.as_ptr();
    config.identity = identity.as_ref().map_or(std::ptr::null(), |i| i.as_ptr());
    // 0 or less for the backend's own twenty seconds, which is what config_init
    // leaves; a RealVNC connection that fails reports nothing at all.

    log::info!("connecting to {}", c.address);
    start(
        c,
        &BEHAVIOUR,
        |callbacks| unsafe { library.vento_realvnc_create(&config, callbacks) },
        Some(&SECURITY),
    )
}

/// The core's security prompt has a remember, and an identity to keep when
/// it was answered with one.
static SECURITY: Security = Security {
    answer: |s, accept, remember| unsafe {
        loaded().vento_realvnc_answer_security(s.session, accept, remember)
    },
    saved_identity,
};

fn saved_identity(s: &CSession) -> Option<String> {
    let raw = unsafe { loaded().vento_realvnc_identity_saved(s.session) };
    if raw.is_null() {
        return None;
    }
    let identity = unsafe { api::c::string(raw) };
    unsafe { ffi::free(raw as *mut c_void) };
    (!identity.is_empty()).then_some(identity)
}

/// Every key goes through the core's own live set, which carries the quality
/// group together and re-reads the encoder. View-only is already sent through
/// the vtable by [`CBackend`](api::c::CBackend), so it is not sent again here.
fn set_option(session: &CSession, o: &Options, key: &str) {
    if key == "ViewOnly" {
        return;
    }
    let (k, v) = (CString::new(key), CString::new(o.get(key)));
    if let (Ok(k), Ok(v)) = (k, v) {
        unsafe { loaded().vento_realvnc_set_option(session.session, k.as_ptr(), v.as_ptr()) }
    }
}

fn info(s: &[String], traffic: Option<Traffic>) -> Vec<ConnectionFact> {
    if s.len() < 8 {
        return Vec::new();
    }
    let mut facts = vec![
        ConnectionFact::of(Field::DesktopName, "Desktop", &s[0]),
        ConnectionFact::of(Field::Protocol, "Protocol", &s[1]),
        ConnectionFact::of(Field::Connection, "Connection", &s[2]),
        ConnectionFact::of(Field::Security, "Security", &s[3]),
        ConnectionFact::of(Field::Encoding, "Encoding", &s[4]),
        ConnectionFact::of(Field::LineSpeed, "Line speed", &s[5]),
    ];
    if let Some(traffic) = traffic {
        facts.push(ConnectionFact::data(traffic));
    }
    facts.push(ConnectionFact::diagnostic(Field::ServerPixels, "Server pixels", &s[6]));
    facts.push(ConnectionFact::diagnostic(Field::ViewerPixels, "Viewer pixels", &s[7]));
    facts
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A library that is not there is a sentence about it, not a panic: what
    /// `--list` prints and a session's start reports where the client was
    /// built without clang.
    #[test]
    fn a_missing_library_says_so() {
        let why = open("libvento_realvnc_not_here.so").err().expect("no such library");
        assert!(why.contains("libvento_realvnc_not_here.so"), "{why}");
        assert!(why.contains("clang"), "{why}");
    }
}
