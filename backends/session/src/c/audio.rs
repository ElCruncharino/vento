// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The platform's sound output as `vento/audio.h`'s table, for a C backend
//! whose far end sends sound: TigerVNC's QEMU audio and FreeRDP's rdpsnd,
//! which until now each had a sink of their own on each platform.
//!
//! What that buys is the same thing [`video`](super::video) buys for
//! pictures: one queue and one policy for every backend on both platforms —
//! how far ahead sound is kept, what is dropped when it gets too far ahead,
//! the device opened off the protocol thread — and the session's mute,
//! which a window sets on the [`Outputs`]' controls without the backend
//! hearing of it.

use std::ffi::{c_int, c_void};
use std::sync::Arc;

use common::audio::{Format, Outputs, Stream};

use super::ffi;

/// `outputs` as the table a backend's config carries. The table's `user` is
/// a box holding its own count of `outputs`, which is the backend's from the
/// moment the table is handed over and comes back through `release`.
pub fn table(outputs: Arc<dyn Outputs>) -> ffi::vento_audio {
    ffi::vento_audio {
        user: Box::into_raw(Box::new(outputs)) as *mut c_void,
        open: Some(open),
        write: Some(write),
        set_gain: Some(set_gain),
        latency: Some(latency),
        close: Some(close),
        release: Some(release),
    }
}

unsafe extern "C" fn open(user: *mut c_void, format: c_int, rate: c_int, channels: c_int) -> *mut ffi::vento_audio_stream {
    let outputs = unsafe { &*(user as *const Arc<dyn Outputs>) };
    if format as u32 != ffi::VENTO_AUDIO_S16 {
        return std::ptr::null_mut();
    }
    let (Ok(rate), Ok(channels)) = (u32::try_from(rate), usize::try_from(channels)) else {
        return std::ptr::null_mut();
    };
    match Format::new(rate, channels).and_then(|f| outputs.open(f)) {
        Some(stream) => Box::into_raw(Box::new(stream)) as *mut ffi::vento_audio_stream,
        None => std::ptr::null_mut(),
    }
}

/// The stream behind a handle, this thread's for the call.
///
/// # Safety
///
/// `s` came from [`open`] and is not closed, and nothing else is inside a
/// call on it, as the header says. A [`Stream`] is `Send`, so the thread may
/// differ from call to call.
unsafe fn stream<'a>(s: *mut ffi::vento_audio_stream) -> &'a mut Stream {
    unsafe { &mut *(s as *mut Stream) }
}

unsafe extern "C" fn write(s: *mut ffi::vento_audio_stream, data: *const c_void, len: usize) {
    if s.is_null() || data.is_null() || len == 0 {
        return;
    }
    let bytes = unsafe { std::slice::from_raw_parts(data as *const u8, len) };
    unsafe { stream(s) }.write_bytes(bytes);
}

unsafe extern "C" fn set_gain(s: *mut ffi::vento_audio_stream, gain: f32) {
    if !s.is_null() {
        unsafe { stream(s) }.set_gain(gain);
    }
}

unsafe extern "C" fn latency(s: *mut ffi::vento_audio_stream) -> c_int {
    if s.is_null() {
        return 0;
    }
    unsafe { stream(s) }.latency().as_millis().min(c_int::MAX as u128) as c_int
}

unsafe extern "C" fn close(s: *mut ffi::vento_audio_stream) {
    if !s.is_null() {
        drop(unsafe { Box::from_raw(s as *mut Stream) });
    }
}

unsafe extern "C" fn release(user: *mut c_void) {
    if !user.is_null() {
        drop(unsafe { Box::from_raw(user as *mut Arc<dyn Outputs>) });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::audio::{Controls, Device, Queue};
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    struct Quiet;

    impl Device for Quiet {
        fn latency(&self) -> Duration {
            Duration::from_millis(7)
        }
    }

    struct Fake {
        controls: Arc<Controls>,
        queues: Mutex<Vec<Arc<Queue>>>,
        dropped: Arc<AtomicBool>,
    }

    impl Outputs for Fake {
        fn controls(&self) -> &Arc<Controls> {
            &self.controls
        }

        fn device(&self, queue: Arc<Queue>) -> Result<Box<dyn Device>, String> {
            self.queues.lock().unwrap().push(queue);
            Ok(Box::new(Quiet))
        }
    }

    impl Drop for Fake {
        fn drop(&mut self) {
            self.dropped.store(true, Ordering::Relaxed);
        }
    }

    #[test]
    fn a_stream_goes_through_the_table() {
        let dropped = Arc::new(AtomicBool::new(false));
        let fake = Arc::new(Fake {
            controls: Arc::new(Controls::new()),
            queues: Mutex::new(Vec::new()),
            dropped: Arc::clone(&dropped),
        });
        let t = table(Arc::clone(&fake) as Arc<dyn Outputs>);
        let s16 = ffi::VENTO_AUDIO_S16 as c_int;
        unsafe {
            assert!(t.open.unwrap()(t.user, 99, 48_000, 2).is_null(), "no such format");
            assert!(t.open.unwrap()(t.user, s16, 1, 2).is_null(), "no such rate");
            assert!(t.open.unwrap()(t.user, s16, 48_000, 0).is_null(), "no channels");
            let s = t.open.unwrap()(t.user, s16, 48_000, 2);
            assert!(!s.is_null());
            assert!(!fake.controls.playing());

            // 100 ms of stereo, written three bytes at a time.
            let bytes: Vec<u8> = (0..4800 * 2).flat_map(|i: i32| (i as i16).to_ne_bytes()).collect();
            for chunk in bytes.chunks(3) {
                t.write.unwrap()(s, chunk.as_ptr().cast(), chunk.len());
            }
            t.write.unwrap()(s, std::ptr::null(), 0);
            assert!(fake.controls.playing());
            assert_eq!(t.latency.unwrap()(s), 107);

            let queue = Arc::clone(&fake.queues.lock().unwrap()[0]);
            let format = queue.format();
            t.set_gain.unwrap()(s, 0.5);
            let mut out = vec![0i16; 8];
            queue.pull(&mut out, format);
            assert_eq!(out, [0, 0, 1, 1, 2, 2, 3, 3]);
            queue.pull(&mut out, format);
            // Samples 8 to 15 at half: 4, 4, 5, 5, ...
            assert_eq!(out, [4, 4, 5, 5, 6, 6, 7, 7]);

            t.close.unwrap()(s);
            assert!(!fake.controls.playing());
            drop(fake);
            assert!(!dropped.load(Ordering::Relaxed));
            t.release.unwrap()(t.user);
        }
        assert!(dropped.load(Ordering::Relaxed));
    }
}
