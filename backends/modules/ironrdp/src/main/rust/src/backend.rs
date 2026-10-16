// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The IronRDP client as a [`Backend`](session::backend::Backend), with this
//! backend's option handling and its rows: what the Linux client holds, and
//! what the phone reaches through `platform_android`'s JNI layer.

use crate::{Client, Config, Nla, Platform};
use common::{Bridge, Session};
use session::backend::{
    Backend, ConnectionFact, Credentials, Facts, Field, Monitor, Picture, Traffic,
};
use session::credentials::rdp_identity;
use session::events::{Feed, Mailbox};
use session::kind::{self, CONNECT_TIMEOUT_MS, Connect, Kind};
use session::options::Options;
use session::rdp::{KEYBOARD_LAYOUT, desktop_size, monitor_count};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub static KIND: Kind = Kind {
    about: &session::catalog::IRONRDP,
    version: || crate::PROTOCOL_VERSION.to_string(),
    create,
};

struct IronRdp {
    session: Session<Client>,
    mailbox: Arc<Mailbox>,
    options: Mutex<Options>,
    user: String,
    domain: String,
}

fn create(c: &Connect) -> Result<Arc<dyn Backend>, String> {
    let o = c.options;
    let (domain, user) = c.domain_and_user();
    let (width, height) = desktop_size(o.get("DesktopSize"), c.screen, c.last_size);
    let config = Config {
        address: c.address.to_string(),
        user_name: Some(user.clone()),
        domain: Some(domain.clone()),
        password: c.password.map(String::from),
        desktop_size: (
            (width as i32).clamp(640, 8192) as u16,
            (height as i32).clamp(480, 8192) as u16,
        ),
        nla: match o.get("Nla") {
            "require" => Nla::Require,
            "off" => Nla::Off,
            _ => Nla::Prefer,
        },
        compression: crate::compression(if o.bool("Compression") {
            Some("rdp61")
        } else {
            None
        }),
        remote_fx: o.get("Graphics") != "bitmap",
        experience: crate::experience(Some(o.get("Experience"))),
        monitors: monitor_count(o),
        client_name: c.client_name.to_string(),
        // `UNIX` from Linux, as IronRDP's own viewer says, and `ANDROID` from
        // the phone.
        platform: match c.platform {
            kind::Platform::Android => Platform::Android,
            kind::Platform::Linux => Platform::Linux,
        },
        keyboard_layout: KEYBOARD_LAYOUT,
        connect_timeout: Duration::from_millis(CONNECT_TIMEOUT_MS as u64),
    };
    log::info!(
        "connecting to {} as {:?} ({}x{} × {}, nla {:?}, compression {:?}, remotefx {}, experience {:?}, platform {:?})",
        config.address,
        config.user_name,
        config.desktop_size.0,
        config.desktop_size.1,
        config.monitors,
        config.nla,
        config.compression,
        config.remote_fx,
        config.experience,
        config.platform
    );
    let session = Session::start(
        Arc::new(Client::new()),
        config,
        Bridge::new(Feed(Arc::clone(&c.mailbox))),
    );
    session.client.set_view_only(o.bool("ViewOnly"));
    Ok(Arc::new(IronRdp {
        session,
        mailbox: Arc::clone(&c.mailbox),
        options: Mutex::new(o.clone()),
        user,
        domain,
    }))
}

impl Backend for IronRdp {
    fn pointer(&self, x: i32, y: i32, button_mask: i32) {
        self.session.client.pointer(
            x.clamp(0, 65535) as u16,
            y.clamp(0, 65535) as u16,
            // Nine bits: back and forward are buttons 8 and 9.
            (button_mask & 0x1ff) as u16,
        );
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

    /// Unlike RFB's "stop asking", this is a Suppress Output PDU — RDP's
    /// server sends what it likes until told not to.
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
            .request_desktop_size(width.min(65535) as u16, height.min(65535) as u16);
    }

    /// RDP's ids are the monitors' places in the list, which is the order a
    /// layout is sent in: the first is the primary. The desktop's size is the
    /// layout's bounding box.
    fn request_layout(&self, _width: u32, _height: u32, screens: &[Monitor]) {
        let screens: Vec<(i32, i32, u32, u32)> = screens
            .iter()
            .map(|m| (m.x, m.y, m.width.max(0) as u32, m.height.max(0) as u32))
            .collect();
        self.session.client.request_layout(&screens);
    }

    /// Nothing but view-only and the monitor count is live: RDP negotiates
    /// its codecs once. The count is remembered rather than acted on — it is
    /// what the *next* layout asks for.
    fn set_option(&self, key: &str, value: &str) {
        let mut options = self.options.lock().unwrap();
        options.set(key, value);
        match key {
            "ViewOnly" => self.session.client.set_view_only(options.bool(key)),
            "Monitors" => self.session.client.set_monitor_count(monitor_count(&options)),
            _ => {}
        }
    }

    fn facts(&self) -> Facts {
        let (width, height) = self.mailbox.size();
        let client = &self.session.client;
        let monitors = Monitor::from_flat(&client.monitors());
        // A single size over several monitors would be divided evenly
        // between them whatever their shapes, so it is offered over one; a
        // layout says each.
        let takes = self.mailbox.connected() && client.can_resize();
        Facts {
            width,
            height,
            can_resize: takes && monitors.len() <= 1,
            can_layout: takes,
            monitors,
            displays: Vec::new(),
            display: -1,
            view_only: self.options.lock().unwrap().bool("ViewOnly"),
            pointer_relative: false,
        }
    }

    /// Two of the panel's rows are simply not here — RDP has no desktop name
    /// and no encoding anybody can name from outside the codec negotiation —
    /// and they are omitted always, not when they happen to be empty.
    fn info(&self) -> Vec<ConnectionFact> {
        if !self.mailbox.connected() {
            return Vec::new();
        }
        let i = self.session.client.info();
        let mut facts = vec![
            ConnectionFact::of(Field::Protocol, "Protocol", &i.protocol),
            ConnectionFact::of(Field::Connection, "Connection", &i.connection),
            ConnectionFact::of(Field::Security, "Security", &i.security),
            ConnectionFact::of(Field::LineSpeed, "Line speed", &i.line_speed),
        ];
        if let Some(traffic) = self.traffic() {
            facts.push(ConnectionFact::data(traffic));
        }
        facts.push(ConnectionFact::diagnostic(
            Field::ServerPixels,
            "Server pixels",
            &i.server_pixels,
        ));
        facts.push(ConnectionFact::diagnostic(
            Field::ViewerPixels,
            "Viewer pixels",
            &i.viewer_pixels,
        ));
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

    /// A typed `DOMAIN\user` is split here, for every client's prompt;
    /// an empty name is the one the session was created with, and a bare one
    /// keeps the session's domain (see [`rdp_identity`]).
    fn answer_credentials(&self, credentials: Option<Credentials>) {
        self.session.answer_credentials(credentials.map(|c| {
            let (domain, user) = rdp_identity(&c, &self.domain, &self.user);
            Credentials {
                user,
                domain,
                password: c.password,
            }
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
