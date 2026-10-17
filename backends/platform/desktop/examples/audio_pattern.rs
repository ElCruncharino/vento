// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! A far end's rhythm, written into the desktop's output: how a protocol's
//! sound arrives — QEMU's steady 10 ms, RDP's 23 ms waves and their
//! lateness, Windows' PCM in pairs of 186 ms waves, Opus frames bunched by a
//! network — with a half-second 440 Hz burst every two seconds, so that
//! `scripts/testaudio/pattern.py` can time each burst from the moment it was
//! written to the moment the null sink captured it (the client's share of a
//! session's latency, with nothing of a far end's) and count what went
//! missing.
//!
//! **Run it through `scripts/testaudio/pattern.sh`, not by hand**: it plays
//! sound. Like `audio_rig`, it writes silence in the pattern's rhythm until
//! a line `go` arrives on stdin, which the script sends only once it has
//! seen the stream on the null sink; without `go` within thirty seconds it
//! exits having played nothing.
//!
//! ```text
//! audio_pattern PATTERN SECONDS [--stall-every S --stall MS] [--seed N]
//! ```
//!
//! PATTERN is one of:
//!
//! - `qemu`: 10 ms of 48 kHz stereo every 10 ms, up to 2 ms late.
//! - `opus`: 20 ms of 48 kHz stereo every 20 ms, up to 3 ms late (SPICE's
//!   and RustDesk's Opus on a quiet network).
//! - `rdp`: Windows' AAC as decoded, 1024 frames of 44.1 kHz stereo a wave,
//!   each due every 23.2 ms and sent up to 18 ms late: the 18-41 ms spacing
//!   the RDP rig saw from Windows 10.
//! - `rdp-pairs`: the same waves two at a time every 46.4 ms, 10 ms early to
//!   10 ms late.
//! - `winpcm`: Windows' PCM, 8192 frames of 44.1 kHz stereo (186 ms) a wave,
//!   two at a time every 372 ms, up to 10 ms late.
//! - `bursty`: 20 ms Opus frames held by the network and let go in bunches,
//!   whatever is due at moments 20 to 100 ms apart.
//!
//! `--stall-every S --stall MS` holds the writer back MS milliseconds every
//! S seconds, after which everything due is written at once: a protocol
//! thread that lost the processor, or a network that paused. The lateness
//! is pseudo-random from `--seed` (12345), the same every run.
//!
//! On stdout: `READY pid=N`, then an `ONSET wall=<unix seconds> idx=<frame in
//! the write> frames=<write's frames> rate=<rate>` line for every burst as it
//! is written, a `STAT t=<seconds> <the overlay's sound line>` line every
//! second, and `DONE <stats>`.
//!
//! Desktop only, like the crate: on Android this is an empty program.

#[cfg(target_os = "android")]
fn main() {}

#[cfg(not(target_os = "android"))]
fn main() {
    rig::main();
}

#[cfg(not(target_os = "android"))]
mod rig {
    use std::io::{BufRead, Write};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    use common::audio::{Format, Outputs};
    use platform_desktop::audio::{Audio, Output};

    /// Half of full scale, as `audio_rig`'s tone.
    const AMPLITUDE: f64 = 16384.0;

    struct Logger;

    impl log::Log for Logger {
        fn enabled(&self, m: &log::Metadata) -> bool {
            m.level() <= log::Level::Info
        }

        fn log(&self, r: &log::Record) {
            if self.enabled(r.metadata()) {
                eprintln!("{} {}", r.level(), r.args());
            }
        }

        fn flush(&self) {}
    }

    /// How a pattern's writes are made: `frames` a write, `group` writes at
    /// once, a group due every `every` and sent between `early` before and
    /// `late` after that; or, for `bursty`, let go at moments `hold` apart.
    struct Pattern {
        rate: u32,
        frames: usize,
        group: usize,
        early: f64,
        late: f64,
        bursty: bool,
    }

    fn pattern(name: &str) -> Pattern {
        let p = |rate, frames, group, early, late| Pattern {
            rate,
            frames,
            group,
            early,
            late,
            bursty: false,
        };
        match name {
            "qemu" => p(48_000, 480, 1, 0.0, 0.002),
            "opus" => p(48_000, 960, 1, 0.0, 0.003),
            "rdp" => p(44_100, 1024, 1, 0.0, 0.018),
            "rdp-pairs" => p(44_100, 1024, 2, 0.010, 0.010),
            "winpcm" => p(44_100, 8192, 2, 0.0, 0.010),
            "bursty" => Pattern {
                bursty: true,
                ..p(48_000, 960, 1, 0.0, 0.0)
            },
            _ => panic!("audio_pattern qemu|opus|rdp|rdp-pairs|winpcm|bursty SECONDS [--stall-every S --stall MS] [--seed N]"),
        }
    }

    pub fn main() {
        log::set_logger(&Logger).unwrap();
        log::set_max_level(log::LevelFilter::Info);
        let mut args = std::env::args().skip(1);
        let name = args.next().expect("PATTERN");
        let seconds: f64 = args.next().expect("SECONDS").parse().unwrap();
        let (mut stall_every, mut stall, mut seed) = (0.0f64, 0.0f64, 12345u32);
        while let Some(a) = args.next() {
            let mut value = || args.next().expect("a value").parse::<f64>().unwrap();
            match a.as_str() {
                "--stall-every" => stall_every = value(),
                "--stall" => stall = value() / 1000.0,
                "--seed" => seed = value() as u32,
                _ => panic!("unknown argument {a}"),
            }
        }
        let p = pattern(&name);
        let format = Format::new(p.rate, 2).unwrap();
        let audio = Arc::new(Audio::new("audio_pattern", Output::Auto));
        let mut stream = audio.open(format).expect("a stream");

        let go = Arc::new(AtomicBool::new(false));
        {
            let go = Arc::clone(&go);
            std::thread::spawn(move || {
                for line in std::io::stdin().lock().lines() {
                    if line.map(|l| l.trim() == "go").unwrap_or(false) {
                        go.store(true, Ordering::SeqCst);
                        return;
                    }
                }
            });
        }
        println!("READY pid={}", std::process::id());
        std::io::stdout().flush().unwrap();

        let mut lcg = seed;
        let mut random = move || {
            lcg = lcg.wrapping_mul(1_103_515_245).wrapping_add(12345);
            (lcg >> 8) as f64 / (1u32 << 24) as f64
        };
        let wave = p.frames as f64 / p.rate as f64;
        let every = wave * p.group as f64;
        let t0 = Instant::now();
        let mut buf = vec![0i16; p.frames * 2];
        // The next write's first frame, and the group it is in.
        let mut next = 0u64;
        let mut go_frame: Option<u64> = None;
        let mut stalled_until = 0.0f64;
        let mut next_stall = if stall_every > 0.0 { stall_every } else { f64::INFINITY };
        let mut next_stat = 1.0;
        let mut release = 0.0f64;
        let end = |go: Option<u64>| go.map(|g| g + ((seconds + 1.0) * p.rate as f64) as u64);
        loop {
            // When this write's group is let go of: its due time and its
            // lateness, or the network's next moment for `bursty`.
            let group = next / (p.frames * p.group) as u64;
            let due = group as f64 * every;
            let at = if p.bursty {
                while release < due {
                    release += 0.020 + 0.080 * random();
                }
                release
            } else if next.is_multiple_of((p.frames * p.group) as u64) {
                (due - p.early + (p.early + p.late) * random()).max(0.0)
            } else {
                // The rest of a group goes with its first.
                t0.elapsed().as_secs_f64()
            };
            let at = at.max(stalled_until);
            let now = t0.elapsed().as_secs_f64();
            if at > now {
                std::thread::sleep(Duration::from_secs_f64(at - now));
            }
            let now = t0.elapsed().as_secs_f64();
            if go_frame.is_some() && now >= next_stall {
                stalled_until = now + stall;
                next_stall += stall_every;
                eprintln!("STALL t={now:.3} for {:.0} ms", stall * 1000.0);
                continue;
            }
            if go_frame.is_none() && go.load(Ordering::SeqCst) {
                // A second of the stream's frames from here, and on a whole
                // write.
                go_frame = Some(next + p.rate as u64);
                next_stall = now + if stall_every > 0.0 { stall_every } else { f64::INFINITY };
            }
            if go_frame.is_none() && now > 30.0 {
                eprintln!("no go within 30 s; nothing was played");
                std::process::exit(2);
            }
            let mut onset = None;
            for (i, f) in buf.chunks_exact_mut(2).enumerate() {
                let n = next + i as u64;
                let v = match go_frame {
                    Some(g) if n >= g => {
                        let t = (n - g) as f64 / p.rate as f64;
                        if t < seconds && t % 2.0 < 0.5 {
                            if (n - g).is_multiple_of(2 * p.rate as u64) {
                                onset = Some(i);
                            }
                            (AMPLITUDE * (2.0 * std::f64::consts::PI * 440.0 * t).sin()) as i16
                        } else {
                            0
                        }
                    }
                    _ => 0,
                };
                f.fill(v);
            }
            if let Some(i) = onset {
                let wall = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs_f64();
                println!("ONSET wall={wall:.6} idx={i} frames={} rate={}", p.frames, p.rate);
            }
            stream.write(&buf);
            next += p.frames as u64;
            if go_frame.is_some() && now >= next_stat {
                next_stat += 1.0;
                let controls = audio.controls();
                let line = controls.sound().line(controls.playback()).unwrap_or_default();
                println!("STAT t={now:.2} {line}");
            }
            std::io::stdout().flush().unwrap();
            if end(go_frame).is_some_and(|e| next > e) {
                break;
            }
        }
        println!("DONE {:?}", stream.stats());
    }
}
