// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The eight backends, each as a [`Kind`]: what the picker and `--list` show,
//! the options it takes, and how to start a session on it.
//!
//! None of them is this client's own any more. The four Rust protocols each
//! carry their adapter in their own crate (`backend.rs` beside the
//! protocol), the three open C libraries in a small crate beside their C
//! sources, over the one adapter for `vento/backend.h` in
//! `remotedesktop-session`; what is left here is RealVNC's, whose library is
//! the viewer's own and is hosted for this client alone. The order is the
//! catalog's, which is the picker's on the phone too.

#[cfg(realvnc)]
pub mod realvnc;

pub use session::kind::{CONNECT_TIMEOUT_MS, Connect, Dirs, Kind, Platform};
pub use session::trust::{Pinned, pinned_as};

pub use platform_desktop::ffmpeg::{FFmpeg, Hardware};
use std::sync::Arc;

/// What the far end lists this machine as, where a phone says its model.
pub fn client_name() -> String {
    std::fs::read_to_string("/etc/hostname")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "vento".to_string())
}

/// What decodes video here: libavcodec, for RustDesk's picture, where the
/// phone hands over MediaCodec — decoding as `hardware` says, which is off
/// unless the person asked.
///
/// One a session, and held by it as well as handed to its backend in
/// [`Connect::decoders`]: switching the way of decoding under a running
/// session is [`FFmpeg::set_hardware`] on this, which a backend decoding
/// through it takes at its next picture (RustDesk), and which every backend
/// is handed at connect through [`Connect::hardware`], the same answer.
///
/// [`FFmpeg::set_hardware`]: platform_desktop::ffmpeg::FFmpeg::set_hardware
pub fn decoders(hardware: Hardware) -> Arc<FFmpeg> {
    Arc::new(FFmpeg::new(hardware))
}

/// Every backend this binary has, in the picker's order. RealVNC is here
/// only when `build.rs` found clang to build its hosting layer with.
pub fn all() -> Vec<&'static Kind> {
    let mut kinds = vec![
        &remotedesktop_rfb::backend::KIND,
        &remotedesktop_libvnc::KIND,
        &remotedesktop_tigervnc::KIND,
        &remotedesktop_freerdp::KIND,
        &remotedesktop_ironrdp::backend::KIND,
    ];
    #[cfg(realvnc)]
    kinds.push(&realvnc::KIND);
    kinds.push(&remotedesktop_spice::backend::KIND);
    kinds.push(&remotedesktop_rustdesk::backend::KIND);
    kinds
}

pub fn find(id: &str) -> Option<&'static Kind> {
    all().into_iter().find(|k| k.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The catalog's order, which `all` has to keep by hand: the kinds are
    /// in four crates and the list is written out here.
    #[test]
    fn the_kinds_are_in_the_catalogs_order() {
        let orders: Vec<i32> = all().iter().map(|k| k.order).collect();
        assert!(orders.windows(2).all(|w| w[0] < w[1]), "{orders:?}");
        for kind in all() {
            assert!(session::catalog::ALL.iter().any(|a| std::ptr::eq(*a, kind.about)));
        }
    }
}
