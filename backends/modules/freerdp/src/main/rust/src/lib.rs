// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! FreeRDP's `vento_freerdp` as a [`Backend`](session::backend::Backend):
//! `FreeRdpNative.nativeCreate`'s parameters and `FreeRdpBackend`'s option
//! handling and rows. The library's config path is a directory of ours, which
//! also becomes the process's HOME where it has none (the phone's); its
//! certificate store there is never read or written, since every certificate
//! is the trust prompt's to decide.

use session::c::{Behaviour, CSession, ffi, start};
use session::backend::{Backend, ConnectionFact, Field, Traffic};
use session::rdp::{desktop_size, monitor_count};
use session::kind::{CONNECT_TIMEOUT_MS, Connect, Kind};
use session::options::Options;
use std::ffi::CString;

pub static KIND: Kind = Kind {
    about: &session::catalog::FREERDP,
    version: || unsafe { session::c::string(ffi::vento_freerdp_version()) },
    create,
};

static BEHAVIOUR: Behaviour = Behaviour {
    info,
    sent_counted: true,
    set_option,
    rdp_domain: true,
};

fn scale(o: &Options) -> i32 {
    o.int("Scale", 100) as i32
}

fn create(c: &Connect) -> Result<std::sync::Arc<dyn Backend>, String> {
    let o = c.options;
    let (domain, user) = c.domain_and_user();
    let (width, height) = desktop_size(o.get("DesktopSize"), c.screen);
    // A directory of ours: the library sets a certificate store up there at
    // connect, never consulted, and without a path for one cannot connect.
    let store = c.dirs.state.join("freerdp");
    std::fs::create_dir_all(&store).map_err(|e| format!("FreeRDP's config directory: {e}"))?;

    let address = CString::new(c.address).map_err(|_| "address has a NUL")?;
    let user_c = (!user.is_empty()).then(|| CString::new(user.clone()).unwrap_or_default());
    let domain_c = (!domain.is_empty()).then(|| CString::new(domain.clone()).unwrap_or_default());
    let password = c.password.and_then(|p| CString::new(p).ok());
    let nla = CString::new(o.get("Nla")).unwrap_or_default();
    let graphics = CString::new(o.get("Graphics")).unwrap_or_default();
    let experience = CString::new(o.get("Experience")).unwrap_or_default();
    let sound = CString::new(o.get("Sound")).unwrap_or_default();
    let name = CString::new(c.client_name).unwrap_or_default();
    let config_path = CString::new(store.to_string_lossy().into_owned()).unwrap_or_default();
    let config = ffi::vento_freerdp_config {
        address: address.as_ptr(),
        user_name: user_c.as_ref().map_or(std::ptr::null(), |u| u.as_ptr()),
        domain: domain_c.as_ref().map_or(std::ptr::null(), |d| d.as_ptr()),
        password: password.as_ref().map_or(std::ptr::null(), |p| p.as_ptr()),
        nla: nla.as_ptr(),
        legacy_tls: o.bool("LegacyTls"),
        compression: o.bool("Compression"),
        graphics: graphics.as_ptr(),
        experience: experience.as_ptr(),
        sound: sound.as_ptr(),
        scale: scale(o),
        width: width as i32,
        height: height as i32,
        monitors: monitor_count(o) as i32,
        keyboard_layout: session::rdp::KEYBOARD_LAYOUT as i32,
        client_name: name.as_ptr(),
        config_path: config_path.as_ptr(),
        connect_timeout_ms: CONNECT_TIMEOUT_MS,
    };
    log::info!("connecting to {} ({}x{})", c.address, width, height);
    let backend = start(
        c,
        &BEHAVIOUR,
        |callbacks| unsafe { ffi::vento_freerdp_create(&config, callbacks) },
        None,
    )?;
    backend.set_view_only(o.bool("ViewOnly"));
    Ok(backend)
}

/// Only the monitor count is live, and remembered rather than acted on: it
/// is what the next layout asks for. Everything else is decided in the
/// connection sequence, which has already happened.
fn set_option(session: &CSession, o: &Options, key: &str) {
    if key == "Monitors" {
        unsafe { ffi::vento_freerdp_set_monitor_count(session.session, monitor_count(o) as i32) }
    }
}

/// No desktop name, because RDP has none — but an encoding, unlike the other
/// RDP backend: this library says what the picture is arriving as.
fn info(s: &[String], traffic: Option<Traffic>) -> Vec<ConnectionFact> {
    if s.len() < 7 {
        return Vec::new();
    }
    let mut facts = vec![
        ConnectionFact::of(Field::Protocol, "Protocol", &s[0]),
        ConnectionFact::of(Field::Connection, "Connection", &s[1]),
        ConnectionFact::of(Field::Security, "Security", &s[2]),
        ConnectionFact::of(Field::Encoding, "Encoding", &s[3]),
        ConnectionFact::of(Field::LineSpeed, "Line speed", &s[4]),
    ];
    if let Some(traffic) = traffic {
        facts.push(ConnectionFact::data(traffic));
    }
    facts.push(ConnectionFact::diagnostic(Field::ServerPixels, "Server pixels", &s[5]));
    facts.push(ConnectionFact::diagnostic(Field::ViewerPixels, "Viewer pixels", &s[6]));
    facts
}
