// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! TigerVNC's `vento_tigervnc` as a [`Backend`](crate::backend::Backend):
//! `TigerVncNative.nativeCreate`'s parameters and `TigerVncBackend`'s option
//! handling and rows. Unlike the phone's, its certificates are this client's
//! to judge: `caller_pins` has the session hand them to `trust_needed`, so
//! they meet the [known hosts](crate::known_hosts) as every other backend's
//! do, and TigerVNC's own known-hosts file is never read or written — which is
//! why no state directory is given it.

use super::{Behaviour, CSession, start};
use crate::backend::{Backend, ConnectionFact, Field, Traffic};
use crate::backends::{CONNECT_TIMEOUT_MS, Connect, Kind};
use crate::ffi;
use crate::options::{self, Options};
use std::ffi::CString;

pub static KIND: Kind = Kind {
    id: "tigervnc",
    name: "VNC (TigerVNC)",
    short_name: "TigerVNC",
    description: "Fastest implementation, dropping the fewest frames, but using 2-5x the CPU usage. This the only VNC backend which supports H.264 encoding, which is the most bandwidth-efficient for large screen updates, where supported by the server. It also supports QEMU audio.",
    options: options::TIGERVNC,
    version: || unsafe { super::string(ffi::vento_tigervnc_version()) },
    create,
};

static BEHAVIOUR: Behaviour = Behaviour {
    info,
    sent_counted: true,
    set_option,
    rdp_domain: false,
};

/// Automatic, for a quality or a colour depth: the client's own rule over
/// its bandwidth estimate.
const LEVEL_AUTO: i32 = -2;

const ENCODING_RAW: i32 = 0;
const ENCODING_RRE: i32 = 2;
const ENCODING_HEXTILE: i32 = 5;
const ENCODING_TIGHT: i32 = 7;
const ENCODING_ZRLE: i32 = 16;
const ENCODING_H264: i32 = 50;

fn encoding(value: &str) -> i32 {
    match value {
        "h264" => ENCODING_H264,
        "zrle" => ENCODING_ZRLE,
        "hextile" => ENCODING_HEXTILE,
        "rre" => ENCODING_RRE,
        "raw" => ENCODING_RAW,
        _ => ENCODING_TIGHT,
    }
}

fn level(value: &str) -> i32 {
    value.parse().unwrap_or(-1)
}

/// The quality option, which has one more answer than compression: "auto" is
/// the client's own rule, and "lossless" falls through to the absence of a
/// level.
fn quality(value: &str) -> i32 {
    if value == "auto" { LEVEL_AUTO } else { level(value) }
}

fn colour(value: &str) -> i32 {
    match value {
        "auto" => LEVEL_AUTO,
        "rgb222" => 1,
        "rgb111" => 2,
        _ => 0,
    }
}

fn create(c: &Connect) -> Result<std::sync::Arc<dyn Backend>, String> {
    let o = c.options;
    let address = CString::new(c.address).map_err(|_| "address has a NUL")?;
    let user = (!c.user.is_empty()).then(|| CString::new(c.user).unwrap_or_default());
    let password = c.password.and_then(|p| CString::new(p).ok());
    let config = ffi::vento_tigervnc_config {
        address: address.as_ptr(),
        user_name: user.as_ref().map_or(std::ptr::null(), |u| u.as_ptr()),
        password: password.as_ref().map_or(std::ptr::null(), |p| p.as_ptr()),
        shared: o.bool("Shared"),
        anonymous_tls: o.bool("AnonymousTLS"),
        encoding: encoding(o.get("Encoding")),
        compress_level: level(o.get("Compression")),
        quality_level: quality(o.get("Quality")),
        color_level: colour(o.get("ColorLevel")),
        h264: o.bool("H264"),
        audio: o.bool("Audio"),
        connect_timeout_ms: CONNECT_TIMEOUT_MS,
        caller_pins: true,
    };
    log::info!("connecting to {}", c.address);
    let backend = start(
        c,
        &BEHAVIOUR,
        |callbacks| unsafe { ffi::vento_tigervnc_create(&config, callbacks) },
        #[cfg(realvnc)]
        None,
    )?;
    backend.set_view_only(o.bool("ViewOnly"));
    Ok(backend)
}

/// All of the picture is live: `SetEncodings` at any point, and the pixel
/// format is this client's to change safely. Shared is decided at ClientInit,
/// audio at the first SetEncodings, and the bell where the bell arrives.
fn set_option(session: &CSession, o: &Options, key: &str) {
    if matches!(key, "Encoding" | "Compression" | "Quality" | "ColorLevel" | "H264") {
        unsafe {
            ffi::vento_tigervnc_set_picture(
                session.session,
                encoding(o.get("Encoding")),
                level(o.get("Compression")),
                quality(o.get("Quality")),
                colour(o.get("ColorLevel")),
                o.bool("H264"),
            )
        }
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
