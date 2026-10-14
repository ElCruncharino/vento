// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The four C and C++ backends, through the vtable in `vento/backend.h`:
//! one adapter over all of them, as `backends/platform/android/jni` is one
//! adapter for the phone.
//!
//! Everything a session is driven by is the same for the four — the vtable is
//! the same — so [`CSession`] holds the raw session and the mailbox and wraps
//! each call once, and [`CBackend`] is the [`Backend`] over it. What differs
//! between the four is their config, their live options and the labels on
//! their info rows, and that is a [`Behaviour`] of function pointers each
//! submodule fills in — the same split the Java `*Backend` classes have, where
//! the uniform natives are stamped by a macro and each class keeps its
//! `nativeCreate` and its `connectionInfo` mapping.
//!
//! The callbacks are trampolined into the mailbox: `user` is an
//! `Arc<Mailbox>` the session carries, recovered by each trampoline as a
//! borrow and reclaimed in [`CBackend::destroy`], which the header promises
//! runs only after the protocol thread has ended and no callback can still be
//! in flight.

use crate::backend::{
    Backend, ConnectionFact, Credentials, Facts, Monitor, Picture, Traffic,
};
use crate::events::{Encryption, Identity, Mailbox, Severity};
use crate::ffi;
use crate::options::Options;
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int, c_void};
use std::sync::{Arc, Mutex};

pub mod freerdp;
pub mod libvnc;
#[cfg(realvnc)]
pub mod realvnc;
pub mod tigervnc;

/// A C string the session's bytes become, UTF-8 as the header promises.
unsafe fn string(ptr: *const c_char) -> String {
    if ptr.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr(ptr) }.to_string_lossy().into_owned()
    }
}

/// The mailbox a callback was handed, as a borrow: `user` is a live
/// `Arc<Mailbox>` for the length of the session.
unsafe fn mailbox<'a>(user: *mut c_void) -> &'a Mailbox {
    unsafe { &*(user as *const Mailbox) }
}

extern "C" fn cb_connected(user: *mut c_void, width: c_int, height: c_int) {
    unsafe { mailbox(user) }.on_connected(width.max(0) as usize, height.max(0) as usize);
}

extern "C" fn cb_desktop_size(user: *mut c_void, width: c_int, height: c_int) {
    unsafe { mailbox(user) }.on_desktop_size(width.max(0) as usize, height.max(0) as usize);
}

extern "C" fn cb_damaged(user: *mut c_void, x: c_int, y: c_int, width: c_int, height: c_int) {
    unsafe { mailbox(user) }.on_damaged(
        x.max(0) as usize,
        y.max(0) as usize,
        width.max(0) as usize,
        height.max(0) as usize,
    );
}

extern "C" fn cb_frame_end(user: *mut c_void) {
    unsafe { mailbox(user) }.on_frame_end();
}

extern "C" fn cb_cursor(
    user: *mut c_void,
    pixels: *const u32,
    width: c_int,
    height: c_int,
    hot_x: c_int,
    hot_y: c_int,
) {
    let (w, h) = (width.max(0) as usize, height.max(0) as usize);
    let words: &[u32] = if pixels.is_null() || w == 0 || h == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(pixels, w * h) }
    };
    unsafe { mailbox(user) }.on_cursor(words, w, h, hot_x, hot_y);
}

extern "C" fn cb_pointer_mode(user: *mut c_void, relative: bool) {
    unsafe { mailbox(user) }.on_pointer_mode(relative);
}

extern "C" fn cb_bell(user: *mut c_void) {
    unsafe { mailbox(user) }.on_bell();
}

extern "C" fn cb_clipboard(user: *mut c_void, utf8: *const c_char) {
    unsafe { mailbox(user) }.on_clipboard(&unsafe { string(utf8) });
}

extern "C" fn cb_credentials_needed(user: *mut c_void, needs_user: bool) {
    unsafe { mailbox(user) }.on_credentials_needed(needs_user);
}

extern "C" fn cb_trust_needed(user: *mut c_void, fingerprint: *const c_char) {
    unsafe { mailbox(user) }.on_trust_needed(&unsafe { string(fingerprint) });
}

extern "C" fn cb_unverified(user: *mut c_void, why: *const c_char) {
    unsafe { mailbox(user) }.on_unverified(&unsafe { string(why) });
}

extern "C" fn cb_question(user: *mut c_void, title: *const c_char, text: *const c_char) {
    unsafe { mailbox(user) }.on_question(&unsafe { string(title) }, &unsafe { string(text) });
}

extern "C" fn cb_security_needed(
    user: *mut c_void,
    identity: c_int,
    encryption: c_int,
    signature: *const c_char,
    catchphrase: *const c_char,
    matching_address: *const c_char,
) {
    unsafe { mailbox(user) }.on_security_needed(
        Identity::from_c(identity),
        Encryption::from_c(encryption),
        &unsafe { string(signature) },
        &unsafe { string(catchphrase) },
        &unsafe { string(matching_address) },
    );
}

extern "C" fn cb_message(
    user: *mut c_void,
    text: *const c_char,
    severity: c_int,
    question: bool,
    confirm_label: *const c_char,
) {
    unsafe { mailbox(user) }.on_message(
        &unsafe { string(text) },
        Severity::from_c(severity),
        question,
        &unsafe { string(confirm_label) },
    );
}

extern "C" fn cb_closed(user: *mut c_void, detail: *const c_char) {
    unsafe { mailbox(user) }.on_closed(&unsafe { string(detail) });
}

/// The callbacks table, pointed at `user`.
fn callbacks(user: *mut c_void) -> ffi::vento_callbacks {
    ffi::vento_callbacks {
        user,
        connected: Some(cb_connected),
        desktop_size: Some(cb_desktop_size),
        damaged: Some(cb_damaged),
        frame_end: Some(cb_frame_end),
        cursor: Some(cb_cursor),
        pointer_mode: Some(cb_pointer_mode),
        bell: Some(cb_bell),
        clipboard: Some(cb_clipboard),
        credentials_needed: Some(cb_credentials_needed),
        trust_needed: Some(cb_trust_needed),
        unverified: Some(cb_unverified),
        question: Some(cb_question),
        security_needed: Some(cb_security_needed),
        message: Some(cb_message),
        closed: Some(cb_closed),
    }
}

/// A started session and the vtable it is reached through. The raw pointers
/// are the C library's and are safe to touch from any thread, as the header
/// says of every `vento_session_*` call.
struct CSession {
    session: *mut ffi::vento_session,
    /// The `Arc<Mailbox>` the callbacks carry, as a raw pointer to reclaim
    /// once the session is destroyed.
    user: *const Mailbox,
}

// SAFETY: the C session is documented as callable from any thread, and the
// user pointer is only reclaimed after `destroy` has joined the protocol
// thread.
unsafe impl Send for CSession {}
unsafe impl Sync for CSession {}

impl CSession {
    fn ops(&self) -> &ffi::vento_session_ops {
        unsafe { &*(*self.session).ops }
    }

    fn pointer(&self, x: i32, y: i32, mask: i32) {
        unsafe { (self.ops().pointer.unwrap())(self.session, x, y, mask) }
    }

    fn key_down(&self, keysym: u32, key_id: i64) {
        unsafe { (self.ops().key_down.unwrap())(self.session, keysym, key_id) }
    }

    fn key_up(&self, key_id: i64) {
        unsafe { (self.ops().key_up.unwrap())(self.session, key_id) }
    }

    fn release_all_keys(&self) {
        unsafe { (self.ops().release_all_keys.unwrap())(self.session) }
    }

    fn focus(&self, focused: bool) {
        unsafe { (self.ops().focus.unwrap())(self.session, focused) }
    }

    fn view_only(&self, view_only: bool) {
        unsafe { (self.ops().view_only.unwrap())(self.session, view_only) }
    }

    fn can_resize(&self) -> bool {
        unsafe { (self.ops().can_resize.unwrap())(self.session) }
    }

    fn monitors(&self) -> Vec<i32> {
        let mut flat = vec![0i32; 4 * ffi::VENTO_MAX_MONITORS as usize];
        let n = unsafe { (self.ops().monitors.unwrap())(self.session, flat.as_mut_ptr()) };
        flat.truncate((n.max(0) as usize * 4).min(flat.len()));
        flat
    }

    fn request_desktop_size(&self, width: i32, height: i32) {
        unsafe { (self.ops().request_desktop_size.unwrap())(self.session, width, height) }
    }

    fn clipboard(&self, text: &str) {
        if let Ok(c) = CString::new(text) {
            unsafe { (self.ops().clipboard.unwrap())(self.session, c.as_ptr()) }
        }
    }

    /// The info rows, in the backend's own order, or `None` where there are
    /// none yet. Freed with the header's `vento_strings_free`, which is
    /// `free` on each item and the array.
    fn info(&self) -> Option<Vec<String>> {
        let mut strings = ffi::vento_strings {
            items: std::ptr::null_mut(),
            count: 0,
        };
        if !unsafe { (self.ops().info.unwrap())(self.session, &mut strings) } {
            return None;
        }
        let rows = (0..strings.count)
            .map(|i| unsafe { string(*strings.items.add(i)) })
            .collect();
        unsafe {
            for i in 0..strings.count {
                ffi::free(*strings.items.add(i) as *mut c_void);
            }
            ffi::free(strings.items as *mut c_void);
        }
        Some(rows)
    }

    /// Received and sent, or `None` while there is nothing to count.
    fn traffic(&self) -> Option<[u64; 2]> {
        let mut out = [0u64; 2];
        unsafe { (self.ops().traffic.unwrap())(self.session, out.as_mut_ptr()) }.then_some(out)
    }

    #[allow(clippy::too_many_arguments)]
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
        // The library writes at (dst_y, dst_x) into `dst` itself; the caller
        // guarantees the destination holds the region, as the header says.
        unsafe {
            (self.ops().read_region.unwrap())(
                self.session,
                x as c_int,
                y as c_int,
                width as c_int,
                height as c_int,
                dst.as_mut_ptr() as *mut c_void,
                dst_stride,
                dst_x as c_int,
                dst_y as c_int,
            )
        }
    }

    fn thumbnail(&self, step: usize, width: usize, height: usize) -> Option<Picture> {
        let step = step.max(1);
        if width == 0 || height == 0 {
            return None;
        }
        let tw = width.div_ceil(step);
        let th = height.div_ceil(step);
        let mut rgba = vec![0u8; tw * th * 4];
        let ok = unsafe {
            (self.ops().read_thumbnail.unwrap())(
                self.session,
                step as c_int,
                rgba.as_mut_ptr() as *mut c_void,
                tw * 4,
                tw as c_int,
                th as c_int,
            )
        };
        ok.then_some(Picture {
            width: tw,
            height: th,
            rgba,
        })
    }

    fn answer_credentials(&self, user: &str, domain: &str, password: Option<&str>) {
        let user = CString::new(user).unwrap_or_default();
        let domain = CString::new(domain).unwrap_or_default();
        let password = password.and_then(|p| CString::new(p).ok());
        unsafe {
            (self.ops().answer_credentials.unwrap())(
                self.session,
                user.as_ptr(),
                domain.as_ptr(),
                password.as_ref().map_or(std::ptr::null(), |p| p.as_ptr()),
            )
        }
    }

    fn answer_trust(&self, accept: bool) {
        unsafe { (self.ops().answer_trust.unwrap())(self.session, accept) }
    }

    fn disconnect(&self) {
        unsafe { (self.ops().disconnect.unwrap())(self.session) }
    }

    /// Ends the session, waits for its thread, frees it, and then reclaims
    /// the mailbox the callbacks carried — in that order, because the
    /// callbacks are only provably finished once `destroy` has returned.
    fn destroy(&self) {
        unsafe {
            (self.ops().destroy.unwrap())(self.session);
            drop(Arc::from_raw(self.user));
        }
    }
}

/// What differs between the four C backends.
struct Behaviour {
    /// The info rows as this backend's panel reads them, with its own labels;
    /// `strings` is what `info` returned, and `traffic` is already gathered.
    info: fn(&[String], Option<Traffic>) -> Vec<ConnectionFact>,
    /// Whether the sent direction is counted. RealVNC's core counts one way,
    /// so its row says "in" alone rather than claiming a zero out.
    sent_counted: bool,
    /// A live option change, after the value has joined `options`. View-only
    /// goes through the vtable; the rest is the backend's own live call.
    set_option: fn(&CSession, &Options, key: &str),
    /// Whether the credentials carry a domain, which only the RDP backend's
    /// do; the others send a blank one.
    rdp_domain: bool,
}

/// A C backend as a [`Backend`].
pub struct CBackend {
    session: CSession,
    mailbox: Arc<Mailbox>,
    options: Mutex<Options>,
    user: String,
    domain: String,
    behaviour: &'static Behaviour,
    /// RealVNC answers its security prompt with a remember through a call of
    /// its own; everyone else has only the plain yes or no.
    #[cfg(realvnc)]
    answer_security: Option<fn(&CSession, bool, bool)>,
}

impl CBackend {
    fn traffic_raw(&self) -> Option<Traffic> {
        let [received, sent] = self.session.traffic()?;
        Some(Traffic {
            received,
            sent: self.behaviour.sent_counted.then_some(sent),
        })
    }
}

impl Backend for CBackend {
    fn pointer(&self, x: i32, y: i32, button_mask: i32) {
        self.session.pointer(x, y, button_mask & 0x1ff);
    }

    fn key_down(&self, keysym: u32, key_id: i64) {
        self.session.key_down(keysym, key_id);
    }

    fn key_up(&self, key_id: i64) {
        self.session.key_up(key_id);
    }

    fn release_all_keys(&self) {
        self.session.release_all_keys();
    }

    fn focus(&self, focused: bool) {
        self.session.focus(focused);
    }

    fn set_view_only(&self, view_only: bool) {
        self.set_option("ViewOnly", if view_only { "true" } else { "false" });
    }

    fn clipboard(&self, text: &str) {
        self.session.clipboard(text);
    }

    fn request_desktop_size(&self, width: u32, height: u32) {
        self.session
            .request_desktop_size(width.min(i32::MAX as u32) as i32, height.min(i32::MAX as u32) as i32);
    }

    fn set_option(&self, key: &str, value: &str) {
        let mut options = self.options.lock().unwrap();
        options.set(key, value);
        if key == "ViewOnly" {
            self.session.view_only(options.bool(key));
        }
        (self.behaviour.set_option)(&self.session, &options, key);
    }

    fn facts(&self) -> Facts {
        let (width, height) = self.mailbox.size();
        let connected = self.mailbox.connected();
        Facts {
            width,
            height,
            monitors: Monitor::from_flat(&self.session.monitors()),
            displays: Vec::new(),
            display: -1,
            can_resize: connected && self.session.can_resize(),
            view_only: self.options.lock().unwrap().bool("ViewOnly"),
            pointer_relative: false,
        }
    }

    fn info(&self) -> Vec<ConnectionFact> {
        if !self.mailbox.connected() {
            return Vec::new();
        }
        match self.session.info() {
            Some(strings) => (self.behaviour.info)(&strings, self.traffic_raw()),
            None => Vec::new(),
        }
    }

    fn traffic(&self) -> Option<Traffic> {
        self.traffic_raw()
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
        self.session
            .read_region(x, y, width, height, dst, dst_stride, dst_x, dst_y)
    }

    fn thumbnail(&self, step: usize) -> Option<Picture> {
        let (width, height) = self.mailbox.size();
        self.session.thumbnail(step, width, height)
    }

    fn answer_credentials(&self, credentials: Option<Credentials>) {
        match credentials {
            None => self.session.answer_credentials("", "", None),
            Some(c) => {
                let (domain, user) = if !self.behaviour.rdp_domain {
                    (String::new(), self.user.clone())
                } else if c.user.is_empty() {
                    (self.domain.clone(), self.user.clone())
                } else if c.domain.is_empty() {
                    super::split_domain(&c.user)
                } else {
                    (c.domain.clone(), c.user.clone())
                };
                // An empty user name is sent as the one the session was
                // created with, as the Java prompts fill it in.
                let user = if user.is_empty() { self.user.clone() } else { user };
                self.session.answer_credentials(&user, &domain, Some(&c.password));
            }
        }
    }

    fn answer_trust(&self, accept: bool) {
        self.session.answer_trust(accept);
    }

    #[cfg(realvnc)]
    fn answer_security(&self, accept: bool, remember: bool) {
        match self.answer_security {
            Some(f) => f(&self.session, accept, remember),
            None => self.session.answer_trust(accept),
        }
    }

    fn disconnect(&self) {
        self.session.disconnect();
    }

    fn destroy(&self) {
        self.session.destroy();
    }
}

/// Builds a session: the config built by `make` from a mailbox's `user`
/// pointer and the callbacks, wrapped in a [`CBackend`] with `behaviour`.
/// `make` returns null where the session could not be set up.
fn start(
    c: &super::Connect,
    behaviour: &'static Behaviour,
    make: impl FnOnce(*const ffi::vento_callbacks) -> *mut ffi::vento_session,
    #[cfg(realvnc)] answer_security: Option<fn(&CSession, bool, bool)>,
) -> Result<Arc<dyn Backend>, String> {
    let mailbox = Arc::clone(&c.mailbox);
    // The mailbox the callbacks carry, as a raw pointer the session owns.
    let user = Arc::into_raw(Arc::clone(&mailbox));
    let table = callbacks(user as *mut c_void);
    let session = make(&table);
    if session.is_null() {
        // Nothing started: reclaim the pointer the callbacks would have.
        unsafe { drop(Arc::from_raw(user)) };
        return Err("could not start the connection".into());
    }
    let (domain, user_name) = c.domain_and_user();
    Ok(Arc::new(CBackend {
        session: CSession { session, user },
        mailbox,
        options: Mutex::new(c.options.clone()),
        user: user_name,
        domain,
        behaviour,
        #[cfg(realvnc)]
        answer_security,
    }))
}
