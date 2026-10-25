// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The `log` crate, pointed at logcat.

/// Route everything the protocol logs to logcat under `tag`, at `info` and
/// above. Once per process: `android_logger` keeps the first logger it is
/// given, so the second backend to load into the same app logs under the
/// first one's tag — which is why each `create` calls this rather than
/// a `JNI_OnLoad`, and why it is harmless to call again.
pub fn init(tag: &str) {
    android_logger::init_once(
        android_logger::Config::default()
            .with_max_level(::log::LevelFilter::Info)
            .with_tag(tag),
    );
}
