// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The `log` crate, pointed at logcat.

/// Route everything the protocol logs to logcat under `tag`, at `info` and
/// above. Called from `create` rather than from `JNI_OnLoad` because the tag
/// is an argument of `create`: Java passes each module's name, and the load
/// hook has nothing but the VM. `android_logger` keeps the first logger it is
/// given, so the later calls are no-ops; and each backend is a library of its
/// own with its own copy of this crate, so one backend's tag never applies to
/// another.
pub fn init(tag: &str) {
    android_logger::init_once(
        android_logger::Config::default()
            .with_max_level(::log::LevelFilter::Info)
            .with_tag(tag),
    );
}
