// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! RealVNC's viewer core as a [`Backend`](api::backend::Backend):
//! `vento_realvnc_config` and `RealVncBackend`'s option handling and rows.
//! The phone's RealVNC backend is the module's Java over the viewer's own
//! JNI, so on any other target than Linux this crate is empty.
//!
//! The library, `libvento_realvnc.so`, is opened when it is first needed
//! rather than linked: the host build makes it only under clang, and decides
//! that after this crate is built, so the backend is always listed and says
//! why it cannot start where the library is not there ([`library`]). It is
//! found by name, as the other C backends' libraries are linked by name —
//! through the binary's rpath or `LD_LIBRARY_PATH` — and once open it stays
//! open, since the core it hosts never lets go of a session.
//!
//! Nothing of RealVNC's is in the tree: the two libraries the session loads
//! come from the viewer's APK under `library_dir`, hashes verified by the
//! session, and what the core persists goes under `state_dir` — both from
//! [`Dirs`](api::kind::Dirs), with XDG defaults. The identity the core asks
//! to have remembered is the caller's to keep, handed out through
//! [`Backend::saved_identity`](api::backend::Backend::saved_identity) and
//! back in as [`Connect::identity`].

#![cfg(target_os = "linux")]

use api::backend::{Backend, ConnectionFact, Field, Traffic};
use api::c::{Behaviour, CSession, Security, start};
use api::kind::{Connect, Dirs, Kind};
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
            .map_err(|e| format!("{e} (the host build makes it only with clang on the path)"))?;
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
    forget: Some(forget),
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

    let state = state_dir(c.dirs);
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

/// Where the core keeps what it persists: handed to it as its state
/// directory.
fn state_dir(dirs: &Dirs) -> std::path::PathBuf {
    dirs.state.join("realvnc")
}

/// The core's own record of the servers it accepted, which it writes itself
/// under the state directory it was given, in the directory of the
/// application name the session gives it (`appName` in
/// `realvnc_session.cxx`): a line per thing it remembers of a server,
/// `<address>=<identity>` for a server with a key and
/// `<address>/extra=<flags>` for what was accepted of it (an unencrypted
/// connection, say), whether or not *Remember server* was ticked. With a
/// server in it, the core asks nothing of that server whatever it is handed.
fn core_identities(dirs: &Dirs) -> std::path::PathBuf {
    state_dir(dirs).join("net.pgaskin.remotedesktop/.vnc/identities")
}

/// [`Kind::forget`]: the core's lines for `address` out of its record, the
/// rest kept as they were — what the phone's `forgetIdentities` deletes the
/// whole file for, since it forgets every server at once. Nothing of the
/// library is loaded; a session to the same server running meanwhile may
/// write its line back when it ends.
fn forget(dirs: &Dirs, address: &str) -> Result<Vec<String>, String> {
    let path = core_identities(dirs);
    let text = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    let (kept, removed) = forget_lines(&text, address);
    if removed.is_empty() {
        return Ok(Vec::new());
    }
    // Replaced whole, as the client's own stores are, with the mode the
    // core gave it.
    let temp = path.with_file_name(format!(".identities.{}.tmp", std::process::id()));
    let written = (|| {
        let mode = std::fs::metadata(&path)?.permissions();
        std::fs::write(&temp, &kept)?;
        std::fs::set_permissions(&temp, mode)?;
        std::fs::rename(&temp, &path)
    })();
    if let Err(e) = written {
        let _ = std::fs::remove_file(&temp);
        return Err(format!("{}: {e}", path.display()));
    }
    Ok(removed
        .into_iter()
        .map(|key| format!("{}: {key}", path.display()))
        .collect())
}

/// The core's record without the lines whose address (the key before any
/// `/`) names the server `address` does, and the keys of those removed.
/// Every other line is kept byte for byte, one the core may read and this
/// does not included.
fn forget_lines(text: &[u8], address: &str) -> (Vec<u8>, Vec<String>) {
    let mut kept = Vec::with_capacity(text.len());
    let mut removed = Vec::new();
    for line in text.split_inclusive(|&b| b == b'\n') {
        let key = std::str::from_utf8(line)
            .ok()
            .and_then(|l| l.split_once('='))
            .map(|(key, _)| key.trim());
        match key {
            Some(key) if api::trust::same_server(key.split('/').next().unwrap_or(key), address) => {
                removed.push(key.to_string());
            }
            _ => kept.extend_from_slice(line),
        }
    }
    (kept, removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_server_is_forgotten_from_the_cores_record_and_nothing_else() {
        let record = b"127.0.0.1::5901/extra=0201\n10.0.0.2::5977=00000800dec7\n\
                       10.0.0.2::5977/extra=0200\nnot a line\n127.0.0.1::59010=00ff";
        // `:1` is display 1, port 5901: the same server as written by the
        // core with `::`.
        let (kept, removed) = forget_lines(record, "127.0.0.1:1");
        assert_eq!(removed, ["127.0.0.1::5901/extra"]);
        assert_eq!(
            kept,
            b"10.0.0.2::5977=00000800dec7\n10.0.0.2::5977/extra=0200\nnot a line\n127.0.0.1::59010=00ff"
        );
        let (kept, removed) = forget_lines(&kept, "10.0.0.2::5977");
        assert_eq!(removed, ["10.0.0.2::5977", "10.0.0.2::5977/extra"]);
        assert_eq!(kept, b"not a line\n127.0.0.1::59010=00ff");
        assert_eq!(forget_lines(b"", "x").1, Vec::<String>::new());
    }

    #[test]
    fn forgetting_rewrites_the_cores_file_and_a_missing_one_is_nothing() {
        let root = std::env::temp_dir().join(format!("vento-realvnc-forget-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let dirs = Dirs {
            config: root.join("config"),
            state: root.join("state"),
            realvnc_libs: root.join("libs"),
        };
        assert_eq!(forget(&dirs, "127.0.0.1::5901"), Ok(Vec::new()));
        let path = core_identities(&dirs);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "127.0.0.1::5901/extra=0201\nother::5900/extra=0201\n").unwrap();
        let removed = forget(&dirs, "127.0.0.1:5901").unwrap();
        assert_eq!(removed.len(), 1);
        assert!(removed[0].ends_with(": 127.0.0.1::5901/extra"), "{removed:?}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "other::5900/extra=0201\n");
        assert_eq!(forget(&dirs, "127.0.0.1:5901"), Ok(Vec::new()));
        let _ = std::fs::remove_dir_all(&root);
    }

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
