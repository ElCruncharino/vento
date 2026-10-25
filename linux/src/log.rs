// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! Where the Rust backends' log lines go: stderr, one line each, in the same
//! `I tag: text` shape `vento_log` in the C libraries writes, so that a session
//! on either half reads the same in a terminal.
//!
//! A short logger rather than a crate, since nothing a logging crate adds is
//! needed here.

use log::{Level, LevelFilter, Log, Metadata, Record};
use std::io::Write;

struct Stderr;

/// The crates under the window whose debug output is about the window
/// rather than a session: tens of thousands of lines a minute from winit's
/// tracing spans and naga's shader passes, which `--verbose` is not for.
const CHATTER: &[&str] = &["tracing", "winit", "wgpu", "naga", "sctk", "calloop", "egui", "smithay"];

impl Log for Stderr {
    fn enabled(&self, metadata: &Metadata) -> bool {
        if metadata.level() > log::max_level() {
            return false;
        }
        let tag = metadata.target().split("::").next().unwrap_or("");
        metadata.level() < Level::Debug || !CHATTER.iter().any(|c| tag.starts_with(c))
    }

    fn log(&self, record: &Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let level = match record.level() {
            Level::Error | Level::Warn => "W",
            Level::Info => "I",
            Level::Debug => "D",
            Level::Trace => "T",
        };
        // The crate's name is the tag, as the C side's is the backend's; the
        // module path after it is noise in a session's log.
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
/// the backends say at `info` and above. Trace lines, which name what is
/// typed key by key, come through only where the environment asks for them
/// by name ([`trace_asked`]), never with `--verbose` alone: a log that is
/// sent with a report must not hold a password typed into the far end.
pub fn init(verbose: bool) {
    let _ = log::set_logger(&Stderr);
    log::set_max_level(if trace_asked(std::env::var("RUST_LOG").ok().as_deref()) {
        LevelFilter::Trace
    } else if verbose {
        LevelFilter::Debug
    } else {
        LevelFilter::Info
    });
}

/// Whether `RUST_LOG` is `trace`, which lets every line through, the trace
/// ones too (and implies `--verbose`).
fn trace_asked(rust_log: Option<&str>) -> bool {
    rust_log.is_some_and(|v| v.trim().eq_ignore_ascii_case("trace"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trace_lines_are_only_for_rust_log_trace() {
        assert!(trace_asked(Some("trace")));
        assert!(trace_asked(Some(" TRACE ")));
        assert!(!trace_asked(None));
        assert!(!trace_asked(Some("debug")));
        assert!(!trace_asked(Some("vento=trace")), "nothing finer than the whole");
    }
}
