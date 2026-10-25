// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The RustDesk client as a [`Backend`](session::backend::Backend), with this
//! backend's option handling and its rows: what the phone reaches through
//! `platform_android`'s JNI layer. The picture is decoded by whatever the
//! client hands over in [`Connect::decoders`]: MediaCodec on the phone.

use crate::{Client, Config, Platform, PreferredCodec, Quality, Reach};
use common::{Bridge, Session};
use session::backend::{
    Backend, ConnectionFact, Credentials, Facts, Field, Monitor, Picture, Traffic,
};
use session::events::{Feed, Mailbox};
use session::kind::{self, CONNECT_TIMEOUT_MS, Connect, Kind};
use session::options::Options;
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub static KIND: Kind = Kind {
    about: &session::catalog::RUSTDESK,
    version: || crate::VERSION.to_string(),
    create,
};

/// The `Codec` option's value, as `RustDeskProvider` writes it.
fn codec_of(name: &str) -> PreferredCodec {
    match name {
        "vp9" => PreferredCodec::Vp9,
        "vp8" => PreferredCodec::Vp8,
        "av1" => PreferredCodec::Av1,
        "h264" => PreferredCodec::H264,
        "h265" => PreferredCodec::H265,
        _ => PreferredCodec::Auto,
    }
}

/// The `Quality` option's value, as `RustDeskProvider` writes it.
fn quality_of(name: &str) -> Quality {
    match name {
        "low" => Quality::Low,
        "best" => Quality::Best,
        _ => Quality::Balanced,
    }
}

/// The frame rate option as the client wants it: 0 for their choice.
fn fps(options: &Options) -> i32 {
    options.int("FrameRate", 0).clamp(0, 120) as i32
}

struct RustDesk {
    session: Session<Client>,
    mailbox: Arc<Mailbox>,
    options: Mutex<Options>,
}

fn create(c: &Connect) -> Result<Arc<dyn Backend>, String> {
    let o = c.options;
    let by_id = o.get("ConnectBy") == "id";
    let config = Config {
        address: c.address.to_string(),
        reach: if by_id {
            Reach::Id {
                server: o.get("RendezvousServer").to_string(),
                key: o.get("RendezvousKey").to_string(),
            }
        } else {
            Reach::Direct
        },
        password: c.password.map(String::from),
        connect_timeout: Duration::from_millis(CONNECT_TIMEOUT_MS as u64),
        // What the far end shows and logs while this session is up.
        my_name: c.client_name.to_string(),
        // What their own client says from the same platform.
        platform: match c.platform {
            kind::Platform::Android => Platform::Android,
            kind::Platform::Linux => Platform::Linux,
        },
        quality: quality_of(o.get("Quality")),
        fps: fps(o),
        codec: codec_of(o.get("Codec")),
        lock_after: o.bool("LockAfterSession"),
    };
    log::info!(
        "connecting to {} {} ({:?}, {} fps)",
        config.address,
        if by_id { "by id" } else { "directly" },
        config.quality,
        config.fps
    );
    let decoders = c.decoders.clone().ok_or("no video decoder to hand the picture to")?;
    let client = Client::new(decoders);
    let session = Session::start(
        Arc::new(client),
        config,
        Bridge::new(Feed(Arc::clone(&c.mailbox))),
    );
    session.client.set_view_only(o.bool("ViewOnly"));
    Ok(Arc::new(RustDesk {
        session,
        mailbox: Arc::clone(&c.mailbox),
        options: Mutex::new(o.clone()),
    }))
}

impl Backend for RustDesk {
    /// Which of these become a move, a press, a release or a notch of the
    /// wheel is the client's business.
    fn pointer(&self, x: i32, y: i32, button_mask: i32) {
        self.session.client.pointer(x, y, button_mask & 0x1ff);
    }

    fn key_down(&self, keysym: u32, key_id: i64) {
        self.session.client.key_down(keysym, key_id);
    }

    fn key_up(&self, key_id: i64) {
        self.session.client.key_up(key_id);
    }

    fn release_all_keys(&self) {
        self.session.client.release_all_keys();
    }

    /// There is no pause message in this protocol, so what this does is ask
    /// the far end for one frame a second.
    fn focus(&self, focused: bool) {
        self.session.client.set_focused(focused);
    }

    fn set_view_only(&self, view_only: bool) {
        self.set_option("ViewOnly", if view_only { "true" } else { "false" });
    }

    fn clipboard(&self, text: &str) {
        self.session.client.clipboard(text);
    }

    fn request_desktop_size(&self, width: u32, height: u32) {
        self.session
            .client
            .request_desktop_size(width.min(i32::MAX as u32) as i32, height.min(i32::MAX as u32) as i32);
    }

    /// What comes back is a switch message, which may name a different
    /// display than the one asked for.
    fn request_display(&self, index: i32) {
        self.session.client.request_display(index);
    }

    /// The picture-quality controls are live: one `Misc::option` message,
    /// which their server acts on for the next frame.
    fn set_option(&self, key: &str, value: &str) {
        let mut options = self.options.lock().unwrap();
        options.set(key, value);
        match key {
            "ViewOnly" => self.session.client.set_view_only(options.bool(key)),
            "Quality" | "FrameRate" | "Codec" => self.session.client.set_options(
                quality_of(options.get("Quality")),
                fps(&options),
                codec_of(options.get("Codec")),
            ),
            _ => {}
        }
    }

    /// The displays and the current index come out of one call, so the two
    /// cannot disagree about a display that arrived between them.
    fn facts(&self) -> Facts {
        let (width, height) = self.mailbox.size();
        let connected = self.mailbox.connected();
        let client = &self.session.client;
        let (flat, current) = if connected {
            client.displays()
        } else {
            (Vec::new(), -1)
        };
        Facts {
            width,
            height,
            monitors: Vec::new(),
            display: if flat.is_empty() { -1 } else { current },
            displays: Monitor::from_flat(&flat),
            can_resize: connected && client.can_resize(),
            can_layout: false,
            view_only: self.options.lock().unwrap().bool("ViewOnly"),
            pointer_relative: false,
        }
    }

    fn info(&self) -> Vec<ConnectionFact> {
        if !self.mailbox.connected() {
            return Vec::new();
        }
        let i = self.session.client.info();
        let mut facts = vec![
            ConnectionFact::of(Field::DesktopName, "Desktop", &i.desktop_name),
            ConnectionFact::of(Field::Protocol, "Protocol", &i.protocol),
            ConnectionFact::of(Field::Connection, "Connection", &i.connection),
            // The one row this backend exists to be honest in: there is no
            // encryption in direct mode and no option to turn any on.
            ConnectionFact::of(Field::Security, "Security", &i.security),
            ConnectionFact::of(Field::Encoding, "Encoding", &i.encoding),
            // Their timing message is a round trip rather than a rate, so it
            // is not the line speed row.
            ConnectionFact::of(Field::Other, "Round trip", &i.round_trip),
        ];
        if let Some(traffic) = self.traffic() {
            facts.push(ConnectionFact::data(traffic));
        }
        facts.push(ConnectionFact::diagnostic(Field::Other, "Peer", &i.platform));
        if !i.display.is_empty() {
            facts.push(ConnectionFact::diagnostic(Field::Other, "Display", &i.display));
        }
        facts
    }

    fn traffic(&self) -> Option<Traffic> {
        let (received, sent) = self.session.client.traffic();
        Some(Traffic {
            received,
            sent: Some(sent),
        })
    }

    fn read_region(
        &self,
        x: usize,
        y: usize,
        width: usize,
        height: usize,
        dst: &mut [u8],
        dst_stride: usize,
        dst_x: usize,
        dst_y: usize,
    ) -> bool {
        let start = dst_y * dst_stride + dst_x * 4;
        if start > dst.len() {
            return false;
        }
        let framebuffer = self.session.client.framebuffer().read().unwrap();
        framebuffer.read_rgba(x, y, width, height, &mut dst[start..], dst_stride)
    }

    fn thumbnail(&self, step: usize) -> Option<Picture> {
        let framebuffer = self.session.client.framebuffer().read().unwrap();
        let (w, h, words) = framebuffer.thumbnail(step.max(1))?;
        Some(Picture::from_words(w, h, &words))
    }

    /// There is no user name in this protocol: a password is the whole of
    /// what a peer asks for, and the rest of the answer is blank.
    fn answer_credentials(&self, credentials: Option<Credentials>) {
        self.session.answer_credentials(credentials.map(|c| Credentials {
            password: c.password,
            ..Credentials::default()
        }));
    }

    fn answer_trust(&self, accept: bool) {
        self.session.answer_trust(accept);
    }

    fn disconnect(&self) {
        self.session.disconnect();
    }

    fn destroy(&self) {
        self.session.destroy();
    }
}
