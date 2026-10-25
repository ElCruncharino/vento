// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The rig itself; see `main.rs`.

use std::io::{BufRead, Read, Write};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use runtime::media::audio::{Format, Outputs, Stream};
use platform_linux::audio::{Audio, Output};

const RATE: u32 = 48_000;
const CHANNELS: usize = 2;
/// Half of full scale: room for a gain above one to be seen not to clip.
const AMPLITUDE: f64 = 16384.0;
/// The writer's chunk, which is how often a session's sound arrives.
const CHUNK: Duration = Duration::from_millis(10);

/// What the writer writes in each stretch of the script.
#[derive(Clone, Copy, PartialEq)]
enum Sound {
    Silence,
    Tone,
    /// Silence with a click at the start of every 500 ms: what latency is
    /// measured by.
    Clicks,
    /// Nothing at all: the session stalled.
    Stall,
}

/// The script, in seconds from `go`. The gain and the mute are not here:
/// they are set from another thread, as a toolbar would (see `toolbar`).
const SCRIPT: &[(f64, f64, Sound)] = &[
    (0.0, 1.0, Sound::Silence),
    (1.0, 11.0, Sound::Tone),
    (11.0, 13.0, Sound::Clicks),
    (13.0, 15.0, Sound::Tone),
    // The burst is written at 15.0, before this stretch's first chunk.
    (15.0, 17.0, Sound::Clicks),
    (17.0, 18.0, Sound::Tone),
    (18.0, 19.0, Sound::Stall),
    (19.0, 21.0, Sound::Tone),
    (21.0, 22.5, Sound::Clicks),
    (22.5, 23.0, Sound::Silence),
];
const BURST_AT: f64 = 15.0;
const BURST: Duration = Duration::from_secs(2);
const BURST_HZ: f64 = 660.0;
const TOOLBAR: &[(f64, &str)] = &[(4.0, "gain 0.5"), (7.0, "gain 1"), (8.0, "mute"), (9.0, "unmute")];

struct Timeline {
    events: Vec<String>,
    latency: Vec<(f64, f64)>,
}

/// The output's own lines (the device it opened, what it fell back to) on
/// stderr, which the script keeps in `rig.log`.
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

pub fn main() {
    log::set_logger(&Logger).unwrap();
    log::set_max_level(log::LevelFilter::Info);
    let mut out = None;
    let mut monitor = "vento_test.monitor".to_string();
    let mut second = false;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--out" => out = args.next(),
            "--monitor" => monitor = args.next().expect("--monitor NAME"),
            "--second" => second = true,
            _ => panic!("audio_rig --out DIR [--monitor NAME] [--second]"),
        }
    }
    let out = std::path::PathBuf::from(out.expect("--out DIR"));
    std::fs::create_dir_all(&out).unwrap();

    let audio = Arc::new(Audio::new("audio_rig", Output::Auto));
    let format = Format::new(RATE, CHANNELS).unwrap();
    let mut stream = audio.open(format).expect("a stream");
    // A second session's output beside the first, playing silence for as
    // long as the rig runs: two sessions' streams, which on PipeWire are two
    // nodes of the process's one connection.
    let done = Arc::new(AtomicBool::new(false));
    let second = second.then(|| {
        let done = Arc::clone(&done);
        std::thread::spawn(move || {
            let audio = Audio::new("audio_rig, second session", Output::Auto);
            let mut stream = audio.open(format).expect("a second stream");
            let silence = vec![0i16; format.frames(CHUNK) * CHANNELS];
            let mut next = Instant::now();
            while !done.load(Ordering::SeqCst) {
                stream.write(&silence);
                next += CHUNK;
                sleep_until(next);
            }
        })
    });

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

    // Silence, in real time, until the script has checked where the stream
    // goes.
    let silence = vec![0i16; format.frames(CHUNK) * CHANNELS];
    let waiting = Instant::now();
    let mut next = waiting;
    while !go.load(Ordering::SeqCst) {
        if waiting.elapsed() > Duration::from_secs(30) {
            eprintln!("no go within 30 s; nothing was played");
            std::process::exit(2);
        }
        stream.write(&silence);
        next += CHUNK;
        sleep_until(next);
    }

    let t0 = Instant::now();
    let recorder = record(&monitor, &out, t0);
    let timeline = Arc::new(Mutex::new(Timeline {
        events: Vec::new(),
        latency: Vec::new(),
    }));
    let toolbar = toolbar(&audio, t0, Arc::clone(&timeline));
    play(&mut stream, t0, &timeline);

    // The recording's tail, then everything stopped and written down.
    std::thread::sleep(Duration::from_millis(500));
    let stats = stream.stats();
    drop(stream);
    done.store(true, Ordering::SeqCst);
    if let Some(second) = second {
        second.join().unwrap();
    }
    toolbar.join().unwrap();
    let chunks = recorder.stop();
    let timeline = timeline.lock().unwrap();
    let mut json = String::new();
    json += &format!("{{\"rate\": {RATE}, \"channels\": {CHANNELS}, \"amplitude\": {AMPLITUDE}, \"tone\": 440, \"burst_hz\": {BURST_HZ},\n");
    json += &format!(
        "\"stats\": {{\"written\": {}, \"dropped\": {}, \"underruns\": {}}},\n",
        stats.written, stats.dropped, stats.underruns
    );
    json += &format!("\"events\": [\n  {}\n],\n", timeline.events.join(",\n  "));
    let pairs = |v: &[(f64, f64)]| v.iter().map(|(a, b)| format!("[{a:.6}, {b:.3}]")).collect::<Vec<_>>().join(", ");
    json += &format!("\"latency\": [{}],\n", pairs(&timeline.latency));
    json += &format!("\"chunks\": [{}]\n}}\n", pairs(&chunks));
    std::fs::write(out.join("timeline.json"), json).unwrap();
    println!("DONE {:?}", stats);
}

fn sleep_until(t: Instant) {
    let now = Instant::now();
    if t > now {
        std::thread::sleep(t - now);
    }
}

fn event(timeline: &Mutex<Timeline>, t0: Instant, what: &str, extra: &str) {
    let t = t0.elapsed().as_secs_f64();
    timeline.lock().unwrap().events.push(format!("{{\"t\": {t:.6}, \"what\": \"{what}\"{extra}}}"));
}

/// The gain and the mute, set from a thread of their own while the writer
/// writes: what a window's toolbar does to a playing session.
fn toolbar(audio: &Arc<Audio>, t0: Instant, timeline: Arc<Mutex<Timeline>>) -> std::thread::JoinHandle<()> {
    let audio = Arc::clone(audio);
    std::thread::spawn(move || {
        for &(at, what) in TOOLBAR {
            sleep_until(t0 + Duration::from_secs_f64(at));
            let controls = audio.controls();
            match what {
                "mute" => controls.set_muted(true),
                "unmute" => controls.set_muted(false),
                gain => controls.set_gain(gain[5..].parse().unwrap()),
            }
            event(&timeline, t0, what, "");
        }
    })
}

/// The script, chunk by chunk, each written at its time.
fn play(stream: &mut Stream, t0: Instant, timeline: &Mutex<Timeline>) {
    let frames = (RATE as f64 * CHUNK.as_secs_f64()) as usize;
    let end = SCRIPT.last().unwrap().1;
    let mut chunk = vec![0i16; frames * CHANNELS];
    let mut k = 0u64;
    let mut burst_done = false;
    loop {
        let at = k as f64 * CHUNK.as_secs_f64();
        if at >= end {
            break;
        }
        sleep_until(t0 + CHUNK * k as u32);
        if !burst_done && at >= BURST_AT {
            burst_done = true;
            let n = (RATE as f64 * BURST.as_secs_f64()) as usize;
            let burst: Vec<i16> = (0..n)
                .flat_map(|i| {
                    let s = (AMPLITUDE * (2.0 * std::f64::consts::PI * BURST_HZ * i as f64 / RATE as f64).sin()) as i16;
                    [s; CHANNELS]
                })
                .collect();
            stream.write(&burst);
            event(timeline, t0, "burst", &format!(", \"seconds\": {}, \"latency_after\": {:.3}", BURST.as_secs_f64(), ms(stream.latency())));
        }
        let (from, _, sound) = SCRIPT.iter().copied().find(|s| at >= s.0 && at < s.1).unwrap_or((0.0, 0.0, Sound::Silence));
        if sound == Sound::Stall {
            k += 1;
            continue;
        }
        let first = k * frames as u64;
        let mut click = false;
        for i in 0..frames {
            let n = first + i as u64;
            let s = match sound {
                Sound::Tone => AMPLITUDE * (2.0 * std::f64::consts::PI * 440.0 * n as f64 / RATE as f64).sin(),
                Sound::Clicks => {
                    // The first 5 ms of each half second except the
                    // stretch's first: a full-scale 3 kHz burst, unlike
                    // anything else in the script.
                    let into = (at * 1000.0).round() as u64 % 500;
                    if into == 0 && at > from + 0.25 && i < RATE as usize / 200 {
                        click = true;
                        30000.0 * (2.0 * std::f64::consts::PI * 3000.0 * i as f64 / RATE as f64).sin()
                    } else {
                        0.0
                    }
                }
                _ => 0.0,
            };
            chunk[i * CHANNELS..(i + 1) * CHANNELS].fill(s as i16);
        }
        let wrote = t0.elapsed().as_secs_f64();
        stream.write(&chunk);
        if click {
            timeline.lock().unwrap().events.push(format!("{{\"t\": {wrote:.6}, \"what\": \"click\"}}"));
        }
        if k.is_multiple_of(10) {
            timeline.lock().unwrap().latency.push((wrote, ms(stream.latency())));
        }
        if sound == Sound::Tone && SCRIPT.iter().any(|s| s.2 == Sound::Stall && (at - s.1).abs() < 1e-6) {
            event(timeline, t0, "resume", "");
        }
        k += 1;
    }
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

/// `parec` on `monitor`, read on a thread of its own into `rec.raw`, with
/// each piece's arrival time, which the analysis uses to put the recording on
/// this process's clock.
struct Recorder {
    child: std::process::Child,
    reader: std::thread::JoinHandle<Vec<(f64, f64)>>,
}

fn record(monitor: &str, out: &std::path::Path, t0: Instant) -> Recorder {
    let mut child = Command::new("parec")
        .args(["--raw", "--format=s16le", "--rate=48000", "--channels=2", "--latency-msec=5"])
        .arg(format!("--device={monitor}"))
        .stdout(Stdio::piped())
        .spawn()
        .expect("parec");
    let mut pipe = child.stdout.take().unwrap();
    let mut file = std::fs::File::create(out.join("rec.raw")).unwrap();
    let reader = std::thread::spawn(move || {
        let mut chunks = Vec::new();
        let mut total = 0usize;
        let mut buf = vec![0u8; 4096];
        while let Ok(n) = pipe.read(&mut buf) {
            if n == 0 {
                break;
            }
            let t = t0.elapsed().as_secs_f64();
            file.write_all(&buf[..n]).unwrap();
            total += n;
            chunks.push((t, (total / 4) as f64));
        }
        chunks
    });
    Recorder { child, reader }
}

impl Recorder {
    fn stop(mut self) -> Vec<(f64, f64)> {
        // Ours, by the handle it was started with.
        let _ = self.child.kill();
        let _ = self.child.wait();
        self.reader.join().unwrap()
    }
}
