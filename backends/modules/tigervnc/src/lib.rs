// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! TigerVNC's `vento_tigervnc` as a [`Backend`](api::backend::Backend):
//! its config from the options, its live options and its rows — what the
//! phone's one JNI layer stands in front of, linked into the library CMake
//! makes of the C. Its certificates are the client's to judge: the session
//! hands them to `trust_needed`, so they meet the known hosts as every other
//! backend's do, and TigerVNC's own known-hosts file is never read or written —
//! which is why no state directory is given it. Its H.264 is decoded by the
//! platform's decoders, handed to the C as `vento/video.h`'s table
//! (`api::c::video`): the phone's MediaCodec. Its sound — QEMU's audio
//! extension — is played on the platform's output the same way, handed to the C
//! as `vento/audio.h`'s table (`api::c::audio`): AAudio on the phone.

use api::c::{Behaviour, CSession, start};
use api::backend::{Backend, ConnectionFact, Field, Traffic};
use api::kind::{CONNECT_TIMEOUT_MS, Connect, Kind};
use api::options::Options;
use std::ffi::CString;

/// `vento/tigervnc.h`, as `build.rs` generated it, over the shared declarations
/// it is written against.
mod ffi {
    #![allow(non_camel_case_types, dead_code)]
    pub use api::c::ffi::*;
    include!(concat!(env!("OUT_DIR"), "/bindings.rs"));
}

pub static KIND: Kind = Kind {
    about: &api::catalog::TIGERVNC,
    version: || unsafe { api::c::string(ffi::vento_tigervnc_version()) },
    create,
    forget: None,
};

// The phone's library: this backend, registered on the module's generated
// natives class. Android only — the host links the C as a library of its own.
// MediaCodec is what decodes its H.264 there (see `create`), and AAudio is what
// plays its sound.
#[cfg(target_os = "android")]
platform_android::natives!(
    class = c"net/pgaskin/remotedesktop/backend/tigervnc/TigerVncNatives",
    kind = KIND,
    decoders = || Some(std::sync::Arc::new(platform_android::mediacodec::MediaCodec)),
    audio = || Some(std::sync::Arc::new(platform_android::aaudio::AAudio::new())),
);

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
    let mut config = ffi::vento_tigervnc_config {
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
        video: std::ptr::null(),
        audio: o.bool("Audio"),
        audio_output: std::ptr::null(),
        connect_timeout_ms: CONNECT_TIMEOUT_MS,
    };
    log::info!("connecting to {}", c.address);
    let backend = start(
        c,
        &BEHAVIOUR,
        |callbacks| {
            // H.264 is decoded by the platform's decoders, which the session
            // is handed here and owns from the call on, released when it
            // ends (or at once, where it does not start). Made here rather
            // than above so that it is handed over on every path that makes
            // it. No decoders, and the session never offers H.264.
            let video = c.decoders.clone().map(api::c::video::table);
            if let Some(video) = &video {
                config.video = video;
            }
            // The sound output, the same way: the session's from here, and
            // without one the server is never asked for its sound.
            let audio = c.audio.clone().map(api::c::audio::table);
            if let Some(audio) = &audio {
                config.audio_output = audio;
            }
            unsafe { ffi::vento_tigervnc_create(&config, callbacks) }
        },
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
