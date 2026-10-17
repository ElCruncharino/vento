// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! FreeRDP's `vento_freerdp` as a [`Backend`](session::backend::Backend):
//! its config from the options, its live options and its rows — what the
//! Linux client holds, and on the phone what the one JNI layer stands in front
//! of, linked into the library CMake makes of the C. The library's config path is a directory of ours, which
//! also becomes the process's HOME; its certificate store there is never read
//! or written, since every certificate is the trust prompt's to decide. Its
//! H.264 is decoded by the platform's decoders and its sound (rdpsnd, with
//! Sound = local) played on the platform's output, handed to the C as
//! `vento/video.h`'s and `vento/audio.h`'s tables as TigerVNC's are.

use session::c::{Behaviour, CSession, ffi, start};
use session::backend::{Backend, ConnectionFact, Field, Traffic};
use session::rdp::{desktop_size, monitor_count};
use session::kind::{CONNECT_TIMEOUT_MS, Connect, Kind, Platform};
use session::options::Options;
use std::ffi::CString;

pub static KIND: Kind = Kind {
    about: &session::catalog::FREERDP,
    version: || unsafe { session::c::string(ffi::vento_freerdp_version()) },
    create,
};

// The phone's library: this backend, registered on the module's generated
// natives class. Android only — the host links the C as a library of its own.
// The session is handed AAudio as its sound output, which the server's sound
// plays on through the module's rdpsnd device.
//
// WinPR has a `JNI_OnLoad` of its own, which keeps the `JavaVM` it reads the
// time zone through, and a library has room for one: the module's CMake
// compiles WinPR's under another name (`winpr_JNI_OnLoad`), and this one calls
// it before registering anything.
#[cfg(target_os = "android")]
platform_android::natives!(
    class = c"net/pgaskin/remotedesktop/backend/freerdp/FreeRdpNatives",
    kind = KIND,
    decoders = || Some(std::sync::Arc::new(platform_android::mediacodec::MediaCodec)),
    audio = || Some(std::sync::Arc::new(platform_android::aaudio::AAudio::new())),
    on_load = winpr_on_load,
);

#[cfg(target_os = "android")]
unsafe extern "C" {
    fn winpr_JNI_OnLoad(vm: *mut platform_android::backend::RawVm, reserved: *mut std::ffi::c_void) -> i32;
}

/// # Safety
///
/// `vm` is the runtime's own, handed to the library's `JNI_OnLoad`.
#[cfg(target_os = "android")]
unsafe fn winpr_on_load(vm: *mut platform_android::backend::RawVm) {
    unsafe { winpr_JNI_OnLoad(vm, std::ptr::null_mut()) };
}

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
    let (width, height) = desktop_size(o.get("DesktopSize"), c.screen, c.last_size);
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
    // The C sends ANDROID or UNIX as the general capability set's major type
    // from these, as the other RDP backend does.
    let platform = match c.platform {
        Platform::Android => c"android",
        Platform::Linux => c"linux",
    };
    let config_path = CString::new(store.to_string_lossy().into_owned()).unwrap_or_default();
    let mut config = ffi::vento_freerdp_config {
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
        platform: platform.as_ptr(),
        config_path: config_path.as_ptr(),
        connect_timeout_ms: CONNECT_TIMEOUT_MS,
        video: std::ptr::null(),
        audio: std::ptr::null(),
    };
    log::info!("connecting to {} ({}x{})", c.address, width, height);
    let backend = start(
        c,
        &BEHAVIOUR,
        |callbacks| {
            // The library's H.264 is decoded by the platform's decoders, as
            // TigerVNC's is: the session owns them from the call on, and
            // without them offers no H.264.
            let video = c.decoders.clone().map(session::c::video::table);
            if let Some(video) = &video {
                config.video = video;
            }
            // And its sound plays on the platform's output, as TigerVNC's
            // does: the session owns the table from the call on, and without
            // one does not ask the server for sound.
            let audio = c.audio.clone().map(session::c::audio::table);
            if let Some(audio) = &audio {
                config.audio = audio;
            }
            unsafe { ffi::vento_freerdp_create(&config, callbacks) }
        },
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
