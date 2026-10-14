// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! Where the Rust backends' log lines go: stderr, one line each, in the same
//! `I tag: text` shape `vento_log` in the C libraries writes, so that a session
//! on either half reads the same in a terminal.
//!
//! A logger of twenty lines rather than a crate, because what it has to do is
//! the twenty lines and nothing a crate would add to them is wanted here.

use log::{Level, LevelFilter, Log, Metadata, Record};
use std::io::Write;

struct Stderr;

impl Log for Stderr {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= log::max_level()
    }

    fn log(&self, record: &Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let level = match record.level() {
            Level::Error | Level::Warn => "W",
            Level::Info => "I",
            Level::Debug | Level::Trace => "D",
        };
        // The crate's name is the tag, as the C side's is the backend's; the
        // module path after it says nothing a person reading a session wants.
        let target = record.target();
        let tag = target.split("::").next().unwrap_or(target);
        // One write per line, so that two protocol threads do not interleave
        // halfway through one.
        let line = format!("{level} {tag}: {}\n", record.args());
        let _ = std::io::stderr().lock().write_all(line.as_bytes());
    }

    fn flush(&self) {}
}

/// Installs the logger. `verbose` lets debug lines through; without it, what
/// the backends say at `info` and above.
pub fn init(verbose: bool) {
    let _ = log::set_logger(&Stderr);
    log::set_max_level(if verbose {
        LevelFilter::Debug
    } else {
        LevelFilter::Info
    });
}
