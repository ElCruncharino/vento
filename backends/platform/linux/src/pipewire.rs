// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The desktop's sound as PipeWire streams of its own, where a PipeWire
//! daemon answers: one node per stream, named for its session, rather than
//! ALSA's `default` device with PipeWire's ALSA plugin behind it.
//!
//! **Loaded, not linked.** libpipewire-0.3 is opened with `dlopen` when the
//! first stream wants it (it needs nothing but libc, and loads its SPA
//! plugins itself), and the twenty-two functions this uses are looked up by
//! hand, as `alsa.rs` declares libasound's: no headers, no bindgen, no
//! link-time dependency, so the binary runs where PipeWire is not installed
//! and plays through ALSA there. The ABI used is libpipewire's stable one
//! (`pw_stream_events` version 2, `pw_buffer` with `requested`, `pw_time` by
//! size), so any PipeWire from 0.3.50 on; a library without one of these
//! functions is no PipeWire to this, and its streams are ALSA's.
//!
//! **Pulled, not pushed.** A stream is connected with `RT_PROCESS`: its
//! process callback runs on PipeWire's data thread once a graph cycle, is
//! asked for `requested` frames (the cycle's quantum, in the stream's rate)
//! and takes them from the stream's queue then and there — which is what
//! `Queue::pull` is for, and what AAudio's data callback does on the phone.
//! There is no ring between the queue and the graph, so the device's share
//! of the delay is the graph's: the cycle being played, and whatever the
//! sink downstream adds (nothing much for a null sink; the card's period
//! and PipeWire's headroom for a real one). That share is read off the
//! stream's clock every cycle (`pw_stream_get_time_n`) and told to the queue
//! ([`Queue::set_device`]) with the cycle, which is part of what the queue
//! keeps, and to the stream's latency, which RDP confirms its waves with. The stream asks for
//! a 512-frame cycle (`node.latency`, 10.7 ms at 48 kHz); the graph runs at
//! the smallest any of its nodes asks for.
//!
//! The stream's format is the far end's: PipeWire's adapter converts the
//! rate and the channels to the sink's, so the queue's resampler never runs
//! here.
//!
//! **One connection for the process.** A thread loop and a context are made
//! once, by the first stream, and a connection to the daemon (a core) is made
//! from them while any stream is open and let go of a while after the last
//! closes. Every session's streams are nodes of that one client. All of it
//! is driven by one thread of ours, `vento-pipewire`, which every call that
//! can wait on the daemon is made from: a stream's [`Device`] is handed back
//! at once and only queues a message, so a protocol thread never waits on
//! the sound server, as the trait asks.
//!
//! **The daemon going away.** A daemon that restarts (or dies) breaks the
//! connection: every stream is let go of, and the connection is made again
//! a moment later and then a second apart, three times, the streams made
//! again on the new one as soon as it is up (a restart is heard as a second
//! of silence). A daemon that is not back by then is played around: the
//! streams it had go to ALSA (or, where PipeWire was asked for by name, to
//! silence), and each looks for the daemon again every few seconds and
//! comes back to it when it is there. A stream that fails on its own, whether
//! refused as it is made or failed by the daemon later, is made again the
//! same way, alone, and stays ALSA's after its third failure in a row. A
//! new stream tries the daemon whenever there is no connection, so a
//! PipeWire that starts later is used from the next stream on.
//!
//! What the stream is called is the session's: `media.name` is the
//! desktop's name (its address until the far end says one), changed live
//! when it does, under `application.name` Vento; `media.role` Movie, since
//! it is sound that goes with a picture. libpipewire itself honours
//! `PIPEWIRE_REMOTE` (which daemon), `PIPEWIRE_NODE` (the sink to play to,
//! as `target.object`), `PIPEWIRE_LATENCY` (in place of the 512 frames) and
//! `PIPEWIRE_PROPS`, which is how the test rigs aim it at a null sink.

use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::ptr::null_mut;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use runtime::media::audio::{Device, Format, Queue};

#[link(name = "dl")]
unsafe extern "C" {
    fn dlopen(file: *const c_char, mode: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, name: *const c_char) -> *mut c_void;
    fn dlerror() -> *const c_char;
}
const RTLD_NOW: c_int = 2;

/// What [`crate::audio::Audio`] calls this output, in the facts row and the
/// overlay.
pub const NAME: &str = "PipeWire";

/// The cycle a stream asks the graph for, in frames of its own rate.
const QUANTUM: u32 = 512;

/// How many times the connection, or a stream that failed on its own, is
/// made again, a second apart, before its streams are given up on: as
/// ALSA's device is reopened. A daemon restarting is back well within it.
/// The first try at the connection waits a moment, since a daemon that is
/// being stopped still answers while it closes its clients.
const ATTEMPTS: u32 = 3;
const RETRY: Duration = Duration::from_secs(1);
const FIRST_RETRY: Duration = Duration::from_millis(250);

/// How often a stream that the daemon left to ALSA looks for it again.
const LATER: Duration = Duration::from_secs(5);

/// How long the connection is kept after the last stream closes: RDP opens
/// a stream per format, and a far end that falls quiet for a moment closes
/// its stream and opens another when it plays again.
const IDLE: Duration = Duration::from_secs(10);

/// Anything of libpipewire's that is only ever handled by pointer.
#[repr(C)]
struct Opaque {
    _private: [u8; 0],
}

#[repr(C)]
struct SpaChunk {
    offset: u32,
    size: u32,
    stride: i32,
    flags: i32,
}

#[repr(C)]
struct SpaData {
    type_: u32,
    flags: u32,
    fd: i64,
    mapoffset: u32,
    maxsize: u32,
    data: *mut c_void,
    chunk: *mut SpaChunk,
}

#[repr(C)]
struct SpaBuffer {
    n_metas: u32,
    n_datas: u32,
    metas: *mut c_void,
    datas: *mut SpaData,
}

#[repr(C)]
struct PwBuffer {
    buffer: *mut SpaBuffer,
    user_data: *mut c_void,
    size: u64,
    requested: u64,
    time: u64,
}

/// `struct pw_time` as far as 0.3.50 has it; asked for by size, so a newer
/// library's longer one is cut to this.
#[repr(C)]
#[derive(Default)]
struct PwTime {
    now: i64,
    rate_num: u32,
    rate_denom: u32,
    ticks: u64,
    delay: i64,
    queued: u64,
    buffered: u64,
    queued_buffers: u32,
    avail_buffers: u32,
}

/// `struct spa_hook`: a listener's place in a list of PipeWire's, which it
/// writes and we only keep, zeroed, at an address that does not move for as
/// long as the object listened to lives.
#[repr(C)]
struct SpaHook {
    link: [*mut c_void; 2],
    cb: [*const c_void; 2],
    removed: *const c_void,
    priv_: *mut c_void,
}

impl SpaHook {
    fn new() -> SpaHook {
        SpaHook {
            link: [null_mut(); 2],
            cb: [std::ptr::null(); 2],
            removed: std::ptr::null(),
            priv_: null_mut(),
        }
    }
}

#[repr(C)]
struct SpaDictItem {
    key: *const c_char,
    value: *const c_char,
}

#[repr(C)]
struct SpaDict {
    flags: u32,
    n_items: u32,
    items: *const SpaDictItem,
}

type Unused = Option<unsafe extern "C" fn()>;

#[repr(C)]
struct PwStreamEvents {
    version: u32,
    destroy: Unused,
    state_changed: Option<unsafe extern "C" fn(*mut c_void, c_int, c_int, *const c_char)>,
    control_info: Unused,
    io_changed: Unused,
    param_changed: Unused,
    add_buffer: Unused,
    remove_buffer: Unused,
    process: Option<unsafe extern "C" fn(*mut c_void)>,
    drained: Unused,
    command: Unused,
    trigger_done: Unused,
}

#[repr(C)]
struct PwProxyEvents {
    version: u32,
    destroy: Unused,
    bound: Unused,
    removed: Unused,
    done: Unused,
    error: Option<unsafe extern "C" fn(*mut c_void, c_int, c_int, *const c_char)>,
    bound_props: Unused,
}

const PW_DIRECTION_OUTPUT: c_int = 1;
const PW_ID_ANY: u32 = 0xffff_ffff;
const PW_ID_CORE: u32 = 0;
const PW_STREAM_FLAG_AUTOCONNECT: c_int = 1 << 0;
const PW_STREAM_FLAG_MAP_BUFFERS: c_int = 1 << 2;
const PW_STREAM_FLAG_RT_PROCESS: c_int = 1 << 4;
const PW_STREAM_STATE_ERROR: c_int = -1;
const PW_STREAM_STATE_UNCONNECTED: c_int = 0;
const EPIPE: c_int = 32;

/// libpipewire's functions this uses, looked up once.
struct Api {
    version: unsafe extern "C" fn() -> *const c_char,
    thread_loop_new: unsafe extern "C" fn(*const c_char, *const SpaDict) -> *mut Opaque,
    thread_loop_get_loop: unsafe extern "C" fn(*mut Opaque) -> *mut Opaque,
    thread_loop_start: unsafe extern "C" fn(*mut Opaque) -> c_int,
    thread_loop_lock: unsafe extern "C" fn(*mut Opaque),
    thread_loop_unlock: unsafe extern "C" fn(*mut Opaque),
    context_new: unsafe extern "C" fn(*mut Opaque, *mut Opaque, usize) -> *mut Opaque,
    context_connect: unsafe extern "C" fn(*mut Opaque, *mut Opaque, usize) -> *mut Opaque,
    core_disconnect: unsafe extern "C" fn(*mut Opaque) -> c_int,
    core_find_proxy: unsafe extern "C" fn(*mut Opaque, u32) -> *mut Opaque,
    proxy_add_listener: unsafe extern "C" fn(*mut Opaque, *mut SpaHook, *const PwProxyEvents, *mut c_void),
    properties_new: unsafe extern "C" fn(*const c_char, ...) -> *mut Opaque,
    properties_set: unsafe extern "C" fn(*mut Opaque, *const c_char, *const c_char) -> c_int,
    stream_new: unsafe extern "C" fn(*mut Opaque, *const c_char, *mut Opaque) -> *mut Opaque,
    stream_add_listener: unsafe extern "C" fn(*mut Opaque, *mut SpaHook, *const PwStreamEvents, *mut c_void),
    stream_connect: unsafe extern "C" fn(*mut Opaque, c_int, u32, c_int, *const *const u8, u32) -> c_int,
    stream_update_properties: unsafe extern "C" fn(*mut Opaque, *const SpaDict) -> c_int,
    stream_dequeue_buffer: unsafe extern "C" fn(*mut Opaque) -> *mut PwBuffer,
    stream_queue_buffer: unsafe extern "C" fn(*mut Opaque, *mut PwBuffer) -> c_int,
    stream_get_time_n: unsafe extern "C" fn(*mut Opaque, *mut PwTime, usize) -> c_int,
    stream_destroy: unsafe extern "C" fn(*mut Opaque),
}

/// The library, or why there is none: looked for once, by the supervisor's
/// first stream, and read by the data thread's callbacks from then on.
static API: OnceLock<Result<Api, String>> = OnceLock::new();

fn api() -> Result<&'static Api, String> {
    API.get_or_init(load).as_ref().map_err(Clone::clone)
}

fn load() -> Result<Api, String> {
    unsafe {
        let lib = dlopen(c"libpipewire-0.3.so.0".as_ptr(), RTLD_NOW);
        if lib.is_null() {
            let why = dlerror();
            return Err(if why.is_null() {
                "libpipewire-0.3.so.0 did not load".into()
            } else {
                CStr::from_ptr(why).to_string_lossy().into_owned()
            });
        }
        macro_rules! sym {
            ($name:literal) => {{
                let p = dlsym(lib, concat!($name, "\0").as_ptr().cast());
                if p.is_null() {
                    return Err(format!("libpipewire-0.3 has no {}", $name));
                }
                function(p)
            }};
        }
        let init: unsafe extern "C" fn(*mut c_int, *mut c_void) = sym!("pw_init");
        let api = Api {
            version: sym!("pw_get_library_version"),
            thread_loop_new: sym!("pw_thread_loop_new"),
            thread_loop_get_loop: sym!("pw_thread_loop_get_loop"),
            thread_loop_start: sym!("pw_thread_loop_start"),
            thread_loop_lock: sym!("pw_thread_loop_lock"),
            thread_loop_unlock: sym!("pw_thread_loop_unlock"),
            context_new: sym!("pw_context_new"),
            context_connect: sym!("pw_context_connect"),
            core_disconnect: sym!("pw_core_disconnect"),
            core_find_proxy: sym!("pw_core_find_proxy"),
            proxy_add_listener: sym!("pw_proxy_add_listener"),
            properties_new: sym!("pw_properties_new"),
            properties_set: sym!("pw_properties_set"),
            stream_new: sym!("pw_stream_new"),
            stream_add_listener: sym!("pw_stream_add_listener"),
            stream_connect: sym!("pw_stream_connect"),
            stream_update_properties: sym!("pw_stream_update_properties"),
            stream_dequeue_buffer: sym!("pw_stream_dequeue_buffer"),
            stream_queue_buffer: sym!("pw_stream_queue_buffer"),
            stream_get_time_n: sym!("pw_stream_get_time_n"),
            stream_destroy: sym!("pw_stream_destroy"),
        };
        init(null_mut(), null_mut());
        Ok(api)
    }
}

/// A symbol `dlsym` found, as the function pointer type it is declared as
/// here.
unsafe fn function<F: Copy>(p: *mut c_void) -> F {
    const { assert!(size_of::<F>() == size_of::<*mut c_void>()) };
    unsafe { std::mem::transmute_copy::<*mut c_void, F>(&p) }
}

/// What a session's streams are called, shared by the session's output and
/// every stream it opens, so that a rename reaches the streams playing.
pub struct Title(Mutex<String>);

impl Title {
    pub fn new(title: &str) -> Arc<Title> {
        Arc::new(Title(Mutex::new(title.to_string())))
    }

    fn get(&self) -> String {
        self.0.lock().unwrap().clone()
    }

    /// Renames the session's streams, those playing included. Nothing is
    /// said to PipeWire where the name has not changed.
    pub fn set(self: &Arc<Title>, title: &str) {
        {
            let mut current = self.0.lock().unwrap();
            if *current == title {
                return;
            }
            *current = title.to_string();
        }
        // Only where a stream has been made: the supervisor is started by
        // the first, and until then there is nothing to rename.
        if let Some(tx) = SUPERVISOR.get() {
            let _ = tx.send(Msg::Retitle(Arc::clone(self)));
        }
    }
}

/// One stream, as its [`Device`] and the supervisor share it.
struct Slot {
    queue: Arc<Queue>,
    title: Arc<Title>,
    /// Whether ALSA takes the stream over where PipeWire cannot have it
    /// (`auto`), or it goes without sound (`pipewire`, asked for by name).
    fallback: bool,
    /// The device's latency as last read off the stream's clock.
    latency_us: AtomicU64,
    /// ALSA's device, once the stream is ALSA's.
    alsa: Mutex<Option<Box<dyn Device>>>,
}

/// A stream's hold on its slot. Dropping it hands the slot back to the
/// supervisor, which lets go of the PipeWire stream on its own thread.
struct PwDevice {
    slot: Arc<Slot>,
}

impl Device for PwDevice {
    fn latency(&self) -> Duration {
        if let Some(alsa) = self.slot.alsa.lock().unwrap().as_ref() {
            return alsa.latency();
        }
        Duration::from_micros(self.slot.latency_us.load(Ordering::Relaxed))
    }
}

impl Drop for PwDevice {
    fn drop(&mut self) {
        send(Msg::Close(Arc::clone(&self.slot)));
    }
}

/// A PipeWire stream draining `queue`, called `title`: handed back at once,
/// and made by the supervisor as soon as it gets to it — or, where there is
/// no PipeWire, handed to ALSA (`fallback`) or left silent.
pub fn device(queue: Arc<Queue>, title: Arc<Title>, fallback: bool) -> Box<dyn Device> {
    let slot = Arc::new(Slot {
        queue,
        title,
        fallback,
        latency_us: AtomicU64::new(0),
        alsa: Mutex::new(None),
    });
    send(Msg::Open(Arc::clone(&slot)));
    Box::new(PwDevice { slot })
}

/// What the supervisor is told, by the streams' devices and by PipeWire's
/// callbacks (which say only what happened, and leave every call back into
/// PipeWire to it).
enum Msg {
    Open(Arc<Slot>),
    Close(Arc<Slot>),
    /// Stream `bound` (its [`Bound::id`]) stopped working: unconnected, or
    /// failed, and why.
    Ended(u64, String),
    /// The connection numbered so broke: the daemon went away.
    Lost(u64, String),
    /// Stream `bound` played its first cycle, of so many frames.
    Playing(u64, usize),
    Retitle(Arc<Title>),
}

static SUPERVISOR: OnceLock<Sender<Msg>> = OnceLock::new();

fn send(msg: Msg) {
    let tx = SUPERVISOR.get_or_init(|| {
        let (tx, rx) = mpsc::channel();
        let started = std::thread::Builder::new()
            .name("vento-pipewire".into())
            .spawn(move || Supervisor::new().run(rx));
        if let Err(why) = started {
            log::warn!("audio: could not start PipeWire's thread ({why})");
        }
        tx
    });
    // Without its thread there is no one to tell, and a stream's slot is
    // dropped here: silence, as for any output that could not start.
    let _ = tx.send(msg);
}

/// The thread loop and the context: made once, kept for the process.
struct Pw {
    api: &'static Api,
    thread_loop: *mut Opaque,
    context: *mut Opaque,
}

impl Pw {
    fn new(api: &'static Api) -> Result<Pw, String> {
        unsafe {
            let thread_loop = (api.thread_loop_new)(c"vento-pw".as_ptr(), std::ptr::null());
            if thread_loop.is_null() {
                return Err("pw_thread_loop_new failed".into());
            }
            let context = (api.context_new)((api.thread_loop_get_loop)(thread_loop), null_mut(), 0);
            if context.is_null() {
                return Err("pw_context_new failed".into());
            }
            if (api.thread_loop_start)(thread_loop) < 0 {
                return Err("pw_thread_loop_start failed".into());
            }
            Ok(Pw {
                api,
                thread_loop,
                context,
            })
        }
    }

    /// The thread loop's lock, which every call into PipeWire is made
    /// under, as its callbacks run under it.
    fn lock(&self) -> Locked<'_> {
        unsafe { (self.api.thread_loop_lock)(self.thread_loop) };
        Locked(self)
    }
}

struct Locked<'a>(&'a Pw);

impl Drop for Locked<'_> {
    fn drop(&mut self) {
        unsafe { (self.0.api.thread_loop_unlock)(self.0.thread_loop) };
    }
}

/// A connection to the daemon.
struct Core {
    core: *mut Opaque,
    id: u64,
    /// The core's listener; boxed, since PipeWire keeps its address.
    _hook: Box<SpaHook>,
}

/// A stream made for a slot on a connection: the address handed to
/// PipeWire as the stream's data, so boxed, and dropped only once the stream
/// is destroyed.
struct Bound {
    slot: Arc<Slot>,
    stream: *mut Opaque,
    /// Unique for the process, so that what an earlier stream of the slot
    /// said is not taken for this one's.
    id: u64,
    hook: SpaHook,
    /// What the queue was last told (data thread only): the cycle, in
    /// frames, and the device's latency, in microseconds.
    told_quantum: AtomicU64,
    told_latency_us: AtomicU64,
    playing: AtomicBool,
}

/// A slot the supervisor holds, with its stream where it has one.
struct Live {
    slot: Arc<Slot>,
    bound: Option<Box<Bound>>,
    /// Its stream's failures in a row, and when it is to be made again.
    failures: u32,
    retry: Option<Instant>,
    /// Handed to ALSA (or to silence) when the daemon did not come back:
    /// taken back from it when the daemon does, which is looked for every
    /// [`LATER`].
    away: bool,
}

/// What the log last said of PipeWire as a whole, so that it is said once
/// a change and not once a stream.
#[derive(Clone, PartialEq)]
enum Said {
    Nothing,
    Connected,
    Unavailable(String),
}

struct Supervisor {
    pw: Option<Result<Pw, String>>,
    core: Option<Core>,
    cores: u64,
    bounds: u64,
    live: Vec<Live>,
    /// Since the connection broke with streams on it: attempts so far and
    /// when the next is due.
    reconnect: Option<(u32, Instant)>,
    idle_since: Option<Instant>,
    said: Said,
}

impl Supervisor {
    fn new() -> Supervisor {
        Supervisor {
            pw: None,
            core: None,
            cores: 0,
            bounds: 0,
            live: Vec::new(),
            reconnect: None,
            idle_since: None,
            said: Said::Nothing,
        }
    }

    fn run(mut self, rx: Receiver<Msg>) {
        loop {
            let msg = match self.next_due() {
                Some(due) => match rx.recv_timeout(due.saturating_duration_since(Instant::now())) {
                    Ok(msg) => Some(msg),
                    Err(RecvTimeoutError::Timeout) => None,
                    Err(RecvTimeoutError::Disconnected) => return,
                },
                None => match rx.recv() {
                    Ok(msg) => Some(msg),
                    Err(_) => return,
                },
            };
            if let Some(msg) = msg {
                self.handle(msg);
            }
            self.tick();
        }
    }

    fn next_due(&self) -> Option<Instant> {
        let retries = self.live.iter().filter_map(|l| l.retry);
        let reconnect = self.reconnect.map(|(_, at)| at);
        let idle = self.idle_since.filter(|_| self.core.is_some()).map(|t| t + IDLE);
        retries.chain(reconnect).chain(idle).min()
    }

    fn handle(&mut self, msg: Msg) {
        match msg {
            Msg::Open(slot) => self.open(slot),
            Msg::Close(slot) => {
                if let Some(at) = self.live.iter().position(|l| Arc::ptr_eq(&l.slot, &slot)) {
                    let mut live = self.live.remove(at);
                    if live.bound.is_some() {
                        log::debug!(
                            "audio: closing {:?}'s PipeWire stream (device {:.1} ms, queue told {:.1} ms)",
                            slot.title.get(),
                            slot.latency_us.load(Ordering::Relaxed) as f64 / 1000.0,
                            slot.queue.device_buffer().as_secs_f64() * 1000.0,
                        );
                    }
                    self.unbind(&mut live);
                }
                if self.live.is_empty() {
                    self.idle_since = Some(Instant::now());
                    self.reconnect = None;
                }
            }
            Msg::Ended(id, why) => {
                let Some(at) = self.live.iter().position(|l| l.bound.as_ref().is_some_and(|b| b.id == id)) else {
                    return;
                };
                let live = self.live.remove(at);
                self.failed(live, &format!("the PipeWire stream stopped working ({why})"));
            }
            Msg::Lost(id, why) => {
                if self.core.as_ref().is_some_and(|c| c.id == id) {
                    self.lost(&why);
                }
            }
            Msg::Playing(id, frames) => {
                let Some(live) = self.live.iter_mut().find(|l| l.bound.as_ref().is_some_and(|b| b.id == id)) else {
                    return;
                };
                // Failures count in a row: one that plays starts them again.
                live.failures = 0;
                let slot = &live.slot;
                let format = slot.queue.format();
                log::info!(
                    "audio: playing {} Hz in {} channels on PipeWire as {:?} (cycle {frames} frames, {:.1} ms; \
                     device {:.1} ms)",
                    format.rate,
                    format.channels,
                    slot.title.get(),
                    format.duration(frames).as_secs_f64() * 1000.0,
                    slot.latency_us.load(Ordering::Relaxed) as f64 / 1000.0,
                );
            }
            Msg::Retitle(title) => {
                let Some(Ok(pw)) = self.pw.as_ref() else { return };
                let name = CString::new(title.get().replace('\0', "")).unwrap_or_default();
                let item = SpaDictItem {
                    key: c"media.name".as_ptr(),
                    value: name.as_ptr(),
                };
                let dict = SpaDict {
                    flags: 0,
                    n_items: 1,
                    items: &item,
                };
                let _locked = pw.lock();
                for live in self.live.iter().filter(|l| Arc::ptr_eq(&l.slot.title, &title)) {
                    if let Some(bound) = live.bound.as_ref() {
                        unsafe { (pw.api.stream_update_properties)(bound.stream, &dict) };
                    }
                }
            }
        }
    }

    /// Retries and reconnects that are due, and the idle connection's end.
    fn tick(&mut self) {
        let now = Instant::now();
        if let Some((attempts, at)) = self.reconnect
            && at <= now
        {
            match self.connect() {
                Ok(()) => {
                    self.reconnect = None;
                    log::info!("audio: PipeWire is back; making its {} stream(s) again", self.live.len());
                    for i in 0..self.live.len() {
                        self.live[i].failures = 0;
                        self.live[i].retry = Some(now);
                    }
                }
                Err(why) if attempts + 1 >= ATTEMPTS => {
                    self.reconnect = None;
                    let why = format!("PipeWire did not come back ({why})");
                    for i in 0..self.live.len() {
                        let slot = Arc::clone(&self.live[i].slot);
                        self.give_up(&slot, &why);
                        self.live[i].away = true;
                        self.live[i].retry = Some(now + LATER);
                    }
                }
                Err(_) => self.reconnect = Some((attempts + 1, now + RETRY)),
            }
        }
        if self.reconnect.is_none() {
            for i in 0..self.live.len() {
                if self.live[i].retry.is_some_and(|at| at <= now) {
                    self.live[i].retry = None;
                    if self.live[i].away {
                        // Quietly, every few seconds, until the daemon is
                        // back; then the stream is taken from ALSA (whose
                        // thread stops within a period) and made again.
                        if self.connect().is_err() {
                            self.live[i].retry = Some(now + LATER);
                            continue;
                        }
                        let live = &mut self.live[i];
                        live.away = false;
                        live.failures = 0;
                        live.slot.alsa.lock().unwrap().take();
                        log::info!("audio: PipeWire is back; {:?} plays on it again", live.slot.title.get());
                    }
                    if let Err(why) = self.bind(i) {
                        let live = self.live.remove(i);
                        self.failed(live, &format!("the PipeWire stream could not be made ({why})"));
                        // The rest are looked at in the next tick, which
                        // comes at once where one of them is due.
                        break;
                    }
                }
            }
        }
        if self.live.is_empty()
            && self.core.is_some()
            && self.idle_since.is_some_and(|t| now.duration_since(t) >= IDLE)
        {
            log::debug!("audio: no stream for {} s; letting go of PipeWire", IDLE.as_secs());
            self.disconnect();
        }
    }

    /// A new stream: made on the connection, which is made first where
    /// there is none; or, where there is no PipeWire, ALSA's.
    fn open(&mut self, slot: Arc<Slot>) {
        self.idle_since = None;
        let mut live = Live {
            slot,
            bound: None,
            failures: 0,
            retry: None,
            away: false,
        };
        if self.reconnect.is_some() {
            // Made with the others when the daemon is back, or given up on
            // with them.
            self.live.push(live);
            return;
        }
        if let Err(why) = self.connect() {
            self.give_up(&live.slot, &why);
            return;
        }
        self.live.push(live);
        let i = self.live.len() - 1;
        if let Err(why) = self.bind(i) {
            live = self.live.remove(i);
            self.failed(live, &format!("the PipeWire stream could not be made ({why})"));
        }
    }

    /// The connection, made where there is none.
    fn connect(&mut self) -> Result<(), String> {
        if self.core.is_some() {
            return Ok(());
        }
        let pw = match self.pw.get_or_insert_with(|| api().and_then(Pw::new)) {
            Ok(pw) => pw,
            Err(why) => return Err(format!("no PipeWire ({why})")),
        };
        let api = pw.api;
        let _locked = pw.lock();
        let core = unsafe { (api.context_connect)(pw.context, null_mut(), 0) };
        if core.is_null() {
            let errno = std::io::Error::last_os_error();
            return Err(format!("no PipeWire daemon answered ({errno})"));
        }
        self.cores += 1;
        let id = self.cores;
        let mut hook = Box::new(SpaHook::new());
        // The core's own errors: a broken connection is one, with EPIPE.
        let proxy = unsafe { (api.core_find_proxy)(core, PW_ID_CORE) };
        if !proxy.is_null() {
            unsafe { (api.proxy_add_listener)(proxy, &mut *hook, &CORE_EVENTS, id as usize as *mut c_void) };
        }
        self.core = Some(Core { core, id, _hook: hook });
        drop(_locked);
        if self.said != Said::Connected {
            let version = unsafe { CStr::from_ptr((api.version)()) }.to_string_lossy();
            log::info!("audio: connected to PipeWire (library {version}); the far end's sound plays on its own streams");
            self.said = Said::Connected;
        }
        Ok(())
    }

    /// Lets go of the connection and every stream on it.
    fn disconnect(&mut self) {
        let Some(Ok(pw)) = self.pw.as_ref() else { return };
        let api = pw.api;
        let _locked = pw.lock();
        for live in &mut self.live {
            if let Some(bound) = live.bound.take() {
                unsafe { (api.stream_destroy)(bound.stream) };
            }
        }
        if let Some(core) = self.core.take() {
            unsafe { (api.core_disconnect)(core.core) };
        }
    }

    /// The daemon went away: everything on the connection is let go of, and
    /// the connection is made again at once and then a second apart.
    fn lost(&mut self, why: &str) {
        log::warn!("audio: lost PipeWire ({why}); connecting again");
        self.disconnect();
        self.said = Said::Nothing;
        if self.live.is_empty() {
            return;
        }
        for live in &mut self.live {
            live.retry = None;
        }
        self.reconnect = Some((0, Instant::now() + FIRST_RETRY));
    }

    /// Makes the stream of `self.live[i]` on the connection.
    fn bind(&mut self, i: usize) -> Result<(), String> {
        self.connect()?;
        let Some(Ok(pw)) = self.pw.as_ref() else { unreachable!("connected without PipeWire") };
        let core = self.core.as_ref().expect("connected").core;
        let api = pw.api;
        self.bounds += 1;
        let live = &mut self.live[i];
        let slot = &live.slot;
        let format = slot.queue.format();
        let mut bound = Box::new(Bound {
            slot: Arc::clone(slot),
            stream: null_mut(),
            id: self.bounds,
            hook: SpaHook::new(),
            told_quantum: AtomicU64::new(0),
            told_latency_us: AtomicU64::new(0),
            playing: AtomicBool::new(false),
        });
        let _locked = pw.lock();
        unsafe {
            let props = (api.properties_new)(std::ptr::null::<c_char>());
            if props.is_null() {
                return Err("pw_properties_new failed".into());
            }
            let set = |k: &CStr, v: &str| {
                let v = CString::new(v.replace('\0', "")).unwrap_or_default();
                (api.properties_set)(props, k.as_ptr(), v.as_ptr());
            };
            for (key, value) in stream_properties(format, &slot.title.get()) {
                set(key, &value);
            }
            let stream = (api.stream_new)(core, c"vento".as_ptr(), props);
            if stream.is_null() {
                return Err("pw_stream_new failed".into());
            }
            bound.stream = stream;
            let data = &mut *bound as *mut Bound as *mut c_void;
            (api.stream_add_listener)(stream, &mut bound.hook, &STREAM_EVENTS, data);
            let pod = format_pod(format);
            let params = [pod.as_ptr()];
            let flags = PW_STREAM_FLAG_AUTOCONNECT | PW_STREAM_FLAG_MAP_BUFFERS | PW_STREAM_FLAG_RT_PROCESS;
            let err = (api.stream_connect)(stream, PW_DIRECTION_OUTPUT, PW_ID_ANY, flags, params.as_ptr(), 1);
            if err < 0 {
                (api.stream_destroy)(stream);
                return Err(format!("pw_stream_connect failed ({})", std::io::Error::from_raw_os_error(-err)));
            }
        }
        slot.queue.set_output(NAME);
        live.bound = Some(bound);
        Ok(())
    }

    /// `live`'s stream stopped working, or could not be made (`why` says
    /// which): made again a second later, alone, and handed to ALSA (or to
    /// silence) at its third failure in a row. Both ways alike: a stream
    /// that `pw_stream_new` or `pw_stream_connect` refused at once had gone
    /// to ALSA for good at the first, where one the daemon failed a moment
    /// later was tried again.
    fn failed(&mut self, mut live: Live, why: &str) {
        self.unbind(&mut live);
        live.failures += 1;
        if live.failures >= ATTEMPTS {
            self.give_up(&live.slot, why);
        } else {
            log::warn!("audio: {why}; trying again in a second");
            live.retry = Some(Instant::now() + RETRY);
            self.live.push(live);
        }
    }

    /// Lets go of `live`'s stream, if it has one.
    fn unbind(&mut self, live: &mut Live) {
        let Some(bound) = live.bound.take() else { return };
        let Some(Ok(pw)) = self.pw.as_ref() else { return };
        let _locked = pw.lock();
        // Waits for the data thread to be done with it, after which nothing
        // of PipeWire's holds `bound` and it can go.
        unsafe { (pw.api.stream_destroy)(bound.stream) };
    }

    /// What is left for a stream PipeWire cannot have: ALSA, or silence.
    fn give_up(&mut self, slot: &Arc<Slot>, why: &str) {
        let said = Said::Unavailable(why.to_string());
        let first = self.said != said;
        if slot.fallback {
            if first {
                log::info!("audio: {why}; the far end's sound plays through ALSA");
            }
            match crate::alsa::device(Arc::clone(&slot.queue)) {
                Ok(device) => *slot.alsa.lock().unwrap() = Some(device),
                Err(why) => log::warn!("audio: could not start ALSA's output either ({why}); the sound is dropped"),
            }
        } else if first {
            log::warn!("audio: {why}; PipeWire was asked for, so the far end's sound is dropped");
        }
        self.said = said;
    }
}

/// A stream's properties: what it is, whose, called what, and the cycle it
/// asks for.
fn stream_properties(format: Format, title: &str) -> Vec<(&'static CStr, String)> {
    vec![
        (c"media.type", "Audio".into()),
        (c"media.category", "Playback".into()),
        (c"media.role", "Movie".into()),
        (c"media.name", title.into()),
        (c"application.name", "Vento".into()),
        (c"application.icon-name", "vento".into()),
        // On the node as well as the client, so that a mixer (and
        // scripts/testaudio/sink.sh) tells whose stream it is.
        (c"application.process.id", std::process::id().to_string()),
        (c"node.name", "vento".into()),
        (c"node.latency", format!("{QUANTUM}/{}", format.rate)),
    ]
}

unsafe extern "C" fn on_core_error(data: *mut c_void, _seq: c_int, res: c_int, message: *const c_char) {
    let id = data as usize as u64;
    if res == -EPIPE {
        send(Msg::Lost(id, text(message)));
    } else {
        log::debug!("audio: PipeWire says: {} ({res})", text(message));
    }
}

unsafe extern "C" fn on_state(data: *mut c_void, _old: c_int, state: c_int, error: *const c_char) {
    let bound = unsafe { &*(data as *const Bound) };
    if state == PW_STREAM_STATE_ERROR || state == PW_STREAM_STATE_UNCONNECTED {
        let why = if state == PW_STREAM_STATE_ERROR { text(error) } else { "unconnected".into() };
        send(Msg::Ended(bound.id, why));
    }
}

fn text(s: *const c_char) -> String {
    if s.is_null() {
        "no reason given".into()
    } else {
        unsafe { CStr::from_ptr(s) }.to_string_lossy().into_owned()
    }
}

/// A graph cycle: the stream's share of it, from its queue.
unsafe extern "C" fn on_process(data: *mut c_void) {
    let bound = unsafe { &*(data as *const Bound) };
    let Some(Ok(api)) = API.get() else { return };
    let b = unsafe { (api.stream_dequeue_buffer)(bound.stream) };
    if b.is_null() {
        return;
    }
    let slot = &bound.slot;
    let format = slot.queue.format();
    let stride = format.frame_bytes();
    unsafe {
        let sb = (*b).buffer;
        let d = &mut *(*sb).datas;
        if d.data.is_null() {
            (api.stream_queue_buffer)(bound.stream, b);
            return;
        }
        let room = d.maxsize as usize / stride;
        let asked = (*b).requested as usize;
        let n = if asked > 0 { asked.min(room) } else { room };
        // How long until this cycle's first frame is heard: what is
        // downstream of the stream (the graph's delay, in its clock's
        // ticks), what the stream's converter holds, and this cycle. Told
        // before the queue is pulled from, since the first pull is where a
        // queue that filled while the stream connected is trimmed to its
        // target, and the cycle is part of the target.
        let mut t = PwTime::default();
        let latency = if (api.stream_get_time_n)(bound.stream, &mut t, size_of::<PwTime>()) == 0 {
            heard_in(format, n, &t)
        } else {
            format.duration(n)
        };
        let micros = latency.as_micros() as u64;
        slot.latency_us.store(micros, Ordering::Relaxed);
        tell(bound, n, micros);
        let out = std::slice::from_raw_parts_mut(d.data as *mut i16, n * format.channels);
        slot.queue.pull(out, format);
        let chunk = &mut *d.chunk;
        chunk.offset = 0;
        chunk.stride = stride as i32;
        chunk.size = (n * stride) as u32;
        (api.stream_queue_buffer)(bound.stream, b);
    }
}

/// Tells the queue what the device holds and takes at once, where that has
/// changed by a millisecond or more (the graph's delay settles over the
/// first cycles, and moves when the sink or the quantum does); and the
/// supervisor, once, that the stream plays.
fn tell(bound: &Bound, frames: usize, latency_us: u64) {
    let quantum = frames as u64;
    let told = bound.told_latency_us.load(Ordering::Relaxed);
    if bound.told_quantum.load(Ordering::Relaxed) != quantum || told.abs_diff(latency_us) >= 1000 {
        bound.told_quantum.store(quantum, Ordering::Relaxed);
        bound.told_latency_us.store(latency_us, Ordering::Relaxed);
        let format = bound.slot.queue.format();
        bound.slot.queue.set_device(Duration::from_micros(latency_us), format.duration(frames));
    }
    if !bound.playing.swap(true, Ordering::Relaxed) {
        send(Msg::Playing(bound.id, frames));
    }
}

static STREAM_EVENTS: PwStreamEvents = PwStreamEvents {
    version: 2,
    destroy: None,
    state_changed: Some(on_state),
    control_info: None,
    io_changed: None,
    param_changed: None,
    add_buffer: None,
    remove_buffer: None,
    process: Some(on_process),
    drained: None,
    command: None,
    trigger_done: None,
};

static CORE_EVENTS: PwProxyEvents = PwProxyEvents {
    version: 1,
    destroy: None,
    bound: None,
    removed: None,
    done: None,
    error: Some(on_core_error),
    bound_props: None,
};

/// How long until the first of a cycle's `n` frames is heard: the cycle,
/// what the stream's converter holds, and what is downstream of the stream
/// (the graph's delay, in its clock's ticks) where the time says its clock.
/// Neither `Duration::from_secs_f64` nor `+`, which panic past what a
/// `Duration` holds, on PipeWire's data thread: a delay no `Duration` holds
/// (a nonsense delay or rate from the graph) counts as none.
fn heard_in(format: Format, n: usize, t: &PwTime) -> Duration {
    let mut latency = format.duration(n).saturating_add(format.duration(t.buffered as usize));
    if t.rate_denom > 0 {
        let downstream = t.delay.max(0) as f64 * t.rate_num as f64 / t.rate_denom as f64;
        latency = latency.saturating_add(Duration::try_from_secs_f64(downstream).unwrap_or_default());
    }
    latency
}

/// SPA's channel positions for a stream in WAVEFORMATEXTENSIBLE's order,
/// which is what RDP, QEMU, SPICE and the decoders hand over: FL FR FC LFE
/// RL RR SL SR.
fn positions(channels: usize) -> Vec<u32> {
    const MONO: u32 = 2;
    const ORDER: [u32; 8] = [3, 4, 5, 6, 12, 13, 7, 8];
    if channels == 1 { vec![MONO] } else { ORDER[..channels.min(8)].to_vec() }
}

/// A pod as `pw_stream_connect` is handed it: in 64-bit words, since SPA
/// reads a pod as `struct spa_pod` and its values in place, which wants it
/// 8-byte aligned (a `Vec<u8>` is aligned as malloc happens to align it).
/// Every pod is a whole number of words, padding included.
struct Pod(Vec<u64>);

impl Pod {
    fn as_ptr(&self) -> *const u8 {
        self.0.as_ptr().cast()
    }

    #[cfg(test)]
    fn bytes(&self) -> Vec<u8> {
        self.0.iter().flat_map(|w| w.to_ne_bytes()).collect()
    }
}

/// The EnumFormat parameter for S16 interleaved at `format`: what SPA's
/// `spa_format_audio_raw_build` makes, laid out by hand, since the builder
/// is inline functions in SPA's headers, nothing a `dlopen` reaches.
fn format_pod(format: Format) -> Pod {
    const SPA_TYPE_ID: u32 = 3;
    const SPA_TYPE_INT: u32 = 4;
    const SPA_TYPE_ARRAY: u32 = 13;
    const SPA_TYPE_OBJECT: u32 = 15;
    const SPA_TYPE_OBJECT_FORMAT: u32 = 0x40003;
    const SPA_PARAM_ENUM_FORMAT: u32 = 3;
    const SPA_FORMAT_MEDIA_TYPE: u32 = 1;
    const SPA_FORMAT_MEDIA_SUBTYPE: u32 = 2;
    const SPA_FORMAT_AUDIO_FORMAT: u32 = 0x10001;
    const SPA_FORMAT_AUDIO_RATE: u32 = 0x10003;
    const SPA_FORMAT_AUDIO_CHANNELS: u32 = 0x10004;
    const SPA_FORMAT_AUDIO_POSITION: u32 = 0x10005;
    const SPA_MEDIA_TYPE_AUDIO: u32 = 1;
    const SPA_MEDIA_SUBTYPE_RAW: u32 = 1;
    /// `SPA_AUDIO_FORMAT_S16`, the native one of S16_LE (0x103) and S16_BE.
    #[cfg(target_endian = "little")]
    const SPA_AUDIO_FORMAT_S16: u32 = 0x103;
    #[cfg(target_endian = "big")]
    const SPA_AUDIO_FORMAT_S16: u32 = 0x104;

    fn word(v: &mut Vec<u8>, x: u32) {
        v.extend_from_slice(&x.to_ne_bytes());
    }
    fn pad(v: &mut Vec<u8>) {
        while !v.len().is_multiple_of(8) {
            v.push(0);
        }
    }
    let mut body = Vec::new();
    word(&mut body, SPA_TYPE_OBJECT_FORMAT);
    word(&mut body, SPA_PARAM_ENUM_FORMAT);
    for (key, ty, value) in [
        (SPA_FORMAT_MEDIA_TYPE, SPA_TYPE_ID, SPA_MEDIA_TYPE_AUDIO),
        (SPA_FORMAT_MEDIA_SUBTYPE, SPA_TYPE_ID, SPA_MEDIA_SUBTYPE_RAW),
        (SPA_FORMAT_AUDIO_FORMAT, SPA_TYPE_ID, SPA_AUDIO_FORMAT_S16),
        (SPA_FORMAT_AUDIO_RATE, SPA_TYPE_INT, format.rate),
        (SPA_FORMAT_AUDIO_CHANNELS, SPA_TYPE_INT, format.channels as u32),
    ] {
        // A property: key, flags, then a pod of its own (size, type, value).
        word(&mut body, key);
        word(&mut body, 0);
        word(&mut body, 4);
        word(&mut body, ty);
        word(&mut body, value);
        pad(&mut body);
    }
    let positions = positions(format.channels);
    word(&mut body, SPA_FORMAT_AUDIO_POSITION);
    word(&mut body, 0);
    // An array pod: its child's size and type, then the values.
    word(&mut body, 8 + 4 * positions.len() as u32);
    word(&mut body, SPA_TYPE_ARRAY);
    word(&mut body, 4);
    word(&mut body, SPA_TYPE_ID);
    for p in positions {
        word(&mut body, p);
    }
    pad(&mut body);
    let mut pod = Vec::new();
    word(&mut pod, body.len() as u32);
    word(&mut pod, SPA_TYPE_OBJECT);
    pod.extend_from_slice(&body);
    Pod(pod.chunks_exact(8).map(|w| u64::from_ne_bytes(w.try_into().unwrap())).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Byte for byte what `spa_format_audio_raw_build` makes for S16
    /// 44.1 kHz stereo (dumped from PipeWire 1.6.9's headers).
    #[test]
    #[cfg(target_endian = "little")]
    fn the_format_pod_is_spas() {
        let golden = "a00000000f00000003000400030000000100000000000000040000000300000001000000000000000200000000000000\
                      040000000300000001000000000000000100010000000000040000000300000003010000000000000300010000000000\
                      040000000400000044ac0000000000000400010000000000040000000400000002000000000000000500010000000000\
                      100000000d00000004000000030000000300000004000000";
        let pod = format_pod(Format::new(44_100, 2).unwrap()).bytes();
        let hex: String = pod.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(hex, golden);
    }

    /// The cycle, the converter's frames and the graph's delay, and a time
    /// that says something no `Duration` holds counted as no delay rather
    /// than panicking the data thread.
    #[test]
    fn a_nonsense_delay_counts_as_none() {
        let format = Format::new(48_000, 2).unwrap();
        let t = |delay: i64, rate_num: u32, rate_denom: u32, buffered: u64| PwTime {
            delay,
            rate_num,
            rate_denom,
            buffered,
            ..PwTime::default()
        };
        // 480 frames, 240 in the converter, 960 ticks of a 48 kHz clock.
        assert_eq!(heard_in(format, 480, &t(960, 1, 48_000, 240)), Duration::from_millis(35));
        assert_eq!(heard_in(format, 480, &t(960, 1, 0, 0)), Duration::from_millis(10));
        assert_eq!(heard_in(format, 480, &t(-5, 1, 48_000, 0)), Duration::from_millis(10));
        assert_eq!(heard_in(format, 480, &t(i64::MAX, u32::MAX, 1, 0)), Duration::from_millis(10));
        assert!(heard_in(format, 480, &t(0, 1, 1, u64::MAX)) >= Duration::from_millis(10));
    }

    #[test]
    fn a_pods_size_is_its_body_and_the_positions_follow_the_channels() {
        for channels in 1..=8 {
            let pod = format_pod(Format::new(48_000, channels).unwrap()).bytes();
            let size = u32::from_ne_bytes(pod[..4].try_into().unwrap()) as usize;
            assert_eq!(pod.len(), 8 + size, "{channels} channels");
            assert_eq!(pod.len() % 8, 0);
        }
        assert_eq!(positions(1), [2]);
        assert_eq!(positions(2), [3, 4]);
        assert_eq!(positions(6), [3, 4, 5, 6, 12, 13]);
    }

    #[test]
    fn a_stream_is_named_for_its_session_and_asks_for_512_frames() {
        let props = stream_properties(Format::new(44_100, 2).unwrap(), "build box");
        let get = |k: &CStr| props.iter().find(|(key, _)| *key == k).map(|(_, v)| v.as_str());
        assert_eq!(get(c"media.name"), Some("build box"));
        assert_eq!(get(c"application.name"), Some("Vento"));
        assert_eq!(get(c"node.latency"), Some("512/44100"));
        assert_eq!(get(c"media.role"), Some("Movie"));
    }

    /// The layouts PipeWire's headers give these, on a 64-bit target.
    #[test]
    #[cfg(target_pointer_width = "64")]
    fn the_structs_are_pipewires_size() {
        assert_eq!(size_of::<SpaHook>(), 48);
        assert_eq!(size_of::<PwBuffer>(), 40);
        assert_eq!(size_of::<SpaData>(), 40);
        // 0.3.50's, without the later `size`.
        assert_eq!(size_of::<PwTime>(), 56);
        assert_eq!(size_of::<SpaDict>(), 16);
    }
}
