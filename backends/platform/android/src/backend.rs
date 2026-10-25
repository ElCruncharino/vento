// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! Any `session::Backend` as the natives `NativeBackend.java` calls: the one
//! JNI layer the Rust backends are reached through on the phone.
//!
//! A backend's library is its protocol crate, its `session::Backend` adapter
//! (`backend.rs` in that crate) and one line naming the two to this module —
//! [`natives!`](crate::natives) — which writes the `JNI_OnLoad` that registers
//! [`METHODS`] on the module's own natives class. Registered rather than
//! exported, because the natives are the same functions in every library: a
//! `Java_…` symbol per backend would need a copy of them per backend, and one
//! class for all of them would be one class every library registers on, the
//! last answering for all — which the RustDesk backend, a separate APK loaded
//! into the app, would make a question of load order. So each module has a
//! class of its own, `vento-codegen natives` writes it from
//! [`surface`](crate::surface), and the library registers on that.
//!
//! The handle Java holds is a `Box<Handle>`: the backend, and the mailbox its
//! events go through. Java retires the handle under its own lock before
//! `destroy` frees it, so the drawing thread is never inside a pixel read of
//! a freed session; `destroy` joins the session's thread, which proves no
//! event is still being delivered.
//!
//! Events cross as they are taken from the session's [`Mailbox`], on whichever
//! thread put them there — the protocol thread, nearly always — and in the
//! order they were put. That is the mailbox's own coalescing, which is the
//! point of using it here: damage is marked into a list and crosses once per
//! frame, as one `int[]`, rather than once per rectangle, and a cursor, a
//! pointer mode or a clipboard that was superseded before anyone read it
//! never crosses at all.

use crate::{bitmap, string};
pub use common::video::Decoders;
use jni::errors::{Error, LogErrorAndDefault};
use jni::objects::{Global, JIntArray, JLongArray, JMethodID, JObject, JObjectArray, JString};
use jni::signature::{Primitive, ReturnType};
use jni::strings::JNIStr;
use jni::sys::{jboolean, jint, jlong, jvalue};
use jni::{Env, JavaVM, NativeMethod, jni_sig, jni_str, native_method};
use session::backend::{Backend, Credentials, Field};
use session::events::{Event, Mailbox};
use session::kind::{Connect, Dirs, Kind, Platform};
use session::options::Options;
use std::ffi::CStr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};

/// What a library is a library of: the backend [`natives!`](crate::natives)
/// named.
pub struct Module {
    pub kind: &'static Kind,
    /// What decodes the protocol's video, for the one whose picture is video:
    /// the phone's MediaCodec for RustDesk, nothing for the others — so that
    /// a library with no use for the NDK's media library does not link it.
    pub decoders: fn() -> Option<Arc<dyn Decoders>>,
}

/// One per library: each `.so` has its own copy of this crate, statics and
/// all, which is what lets the natives be the same functions everywhere and
/// still know which backend they are in front of.
static MODULE: OnceLock<Module> = OnceLock::new();

/// Writes the library's `JNI_OnLoad`: register [`METHODS`] on `class`, the
/// module's natives class, for `kind`.
///
/// ```ignore
/// platform_android::natives!(
///     class = c"net/pgaskin/remotedesktop/backend/rfb/RfbNatives",
///     kind = crate::backend::KIND,
/// );
/// ```
///
/// `decoders = <fn() -> Option<Arc<dyn Decoders>>>` hands the session a video
/// decoder, for the backend that needs one.
#[macro_export]
macro_rules! natives {
    (class = $class:expr, kind = $kind:expr $(, decoders = $decoders:expr)? $(,)?) => {
        /// Registers this library's natives on its module's class. Called by
        /// the runtime on the thread that loaded the library, which is what
        /// lets `FindClass` see the module's class loader.
        #[unsafe(no_mangle)]
        // The runtime's entry point: what it is handed is the runtime's own.
        #[allow(clippy::not_unsafe_ptr_arg_deref)]
        pub extern "system" fn JNI_OnLoad(
            vm: *mut $crate::backend::RawVm,
            _reserved: *mut ::std::ffi::c_void,
        ) -> i32 {
            #[allow(unused_mut, unused_assignments)]
            let mut decoders: fn() -> Option<::std::sync::Arc<dyn $crate::backend::Decoders>> = || None;
            $(decoders = $decoders;)?
            // SAFETY: `vm` is the runtime's own, as `on_load` asks.
            unsafe { $crate::backend::on_load(vm, $class, $crate::backend::Module { kind: &$kind, decoders }) }
        }
    };
}

#[doc(hidden)]
pub use jni::sys::JavaVM as RawVm;

/// What [`natives!`](crate::natives)' `JNI_OnLoad` does.
///
/// # Safety
///
/// `vm` must be the `JavaVM` the runtime handed `JNI_OnLoad`.
#[doc(hidden)]
pub unsafe fn on_load(vm: *mut jni::sys::JavaVM, class: &'static CStr, module: Module) -> i32 {
    let Some(name) = JNIStr::from_cstr(class) else {
        return jni::sys::JNI_ERR;
    };
    if MODULE.set(module).is_err() {
        // A library is loaded once per class loader; a second load of the
        // same one would be registering the same functions again.
        return jni::sys::JNI_VERSION_1_6;
    }
    let vm = unsafe { JavaVM::from_raw(vm) };
    let registered: jni::errors::Result<()> = vm.attach_current_thread(|env| {
        let class = env.find_class(name)?;
        unsafe { env.register_native_methods(&class, METHODS) }
    });
    match registered {
        Ok(()) => jni::sys::JNI_VERSION_1_6,
        Err(e) => {
            // Logged where it can be seen: a library that loads and registers
            // nothing is an UnsatisfiedLinkError on the first call, which says
            // less than this.
            crate::log::init("NativeBackend");
            log::error!("registering natives on {class:?}: {e}");
            jni::sys::JNI_ERR
        }
    }
}

fn module() -> &'static Module {
    MODULE.get().expect("JNI_OnLoad ran")
}

/// What a handle points at.
struct Handle {
    backend: Arc<dyn Backend>,
    /// Kept so that a handle outlives every event it could still deliver; the
    /// outlet holds only a weak reference back to it.
    _mailbox: Arc<Mailbox>,
}

/// SAFETY: `handle` must be one `create` returned that `destroy` has not been
/// called for. Zero is the ordinary "already retired" case.
unsafe fn backend<'a>(handle: jlong) -> Option<&'a dyn Backend> {
    if handle == 0 {
        None
    } else {
        Some(unsafe { &*(handle as *const Handle) }.backend.as_ref())
    }
}

/// The table [`on_load`] registers, in [`surface::NATIVES`]' order. Each entry
/// is checked against its function when it compiles, and its name and
/// descriptor against the generated Java by `scripts/check-jni-symbols.sh`.
///
/// [`surface::NATIVES`]: crate::surface::NATIVES
pub const METHODS: &[NativeMethod] = &[
    native_method! { fn version() -> JString, error_policy = LogErrorAndDefault },
    native_method! {
        fn create(
            events: net.pgaskin.remotedesktop.backend.NativeBackend,
            tag: JString,
            address: JString,
            user_name: JString,
            password: JString,
            options: [JString],
            screen_width: jint,
            screen_height: jint,
            last_width: jint,
            last_height: jint,
            client_name: JString,
            files_dir: JString,
        ) -> jlong,
        error_policy = LogErrorAndDefault,
    },
    native_method! {
        fn answer_credentials(handle: jlong, user_name: JString, password: JString),
        error_policy = LogErrorAndDefault,
    },
    native_method! { fn answer_trust(handle: jlong, accept: jboolean), error_policy = LogErrorAndDefault },
    native_method! { fn disconnect(handle: jlong), error_policy = LogErrorAndDefault },
    native_method! { fn destroy(handle: jlong), error_policy = LogErrorAndDefault },
    native_method! {
        fn pointer(handle: jlong, x: jint, y: jint, button_mask: jint),
        error_policy = LogErrorAndDefault,
    },
    native_method! {
        fn pointer_relative(handle: jlong, dx: jint, dy: jint, button_mask: jint),
        error_policy = LogErrorAndDefault,
    },
    native_method! {
        fn key_down(handle: jlong, keysym: jint, key_id: jlong),
        error_policy = LogErrorAndDefault,
    },
    native_method! { fn key_up(handle: jlong, key_id: jlong), error_policy = LogErrorAndDefault },
    native_method! { fn release_all_keys(handle: jlong), error_policy = LogErrorAndDefault },
    native_method! { fn focus(handle: jlong, focused: jboolean), error_policy = LogErrorAndDefault },
    native_method! { fn clipboard(handle: jlong, text: JString), error_policy = LogErrorAndDefault },
    native_method! {
        fn set_option(handle: jlong, key: JString, value: JString),
        error_policy = LogErrorAndDefault,
    },
    native_method! {
        fn request_desktop_size(handle: jlong, width: jint, height: jint),
        error_policy = LogErrorAndDefault,
    },
    native_method! {
        fn request_display(handle: jlong, index: jint),
        error_policy = LogErrorAndDefault,
    },
    native_method! { fn facts(handle: jlong) -> [jint], error_policy = LogErrorAndDefault },
    native_method! { fn info(handle: jlong) -> [JString], error_policy = LogErrorAndDefault },
    native_method! { fn traffic(handle: jlong) -> [jlong], error_policy = LogErrorAndDefault },
    native_method! {
        raw fn read_region(
            handle: jlong,
            x: jint,
            y: jint,
            width: jint,
            height: jint,
            dst: android.graphics.Bitmap,
            dst_x: jint,
            dst_y: jint,
        ) -> jboolean,
    },
    native_method! {
        raw fn read_thumbnail(handle: jlong, step: jint, dst: android.graphics.Bitmap) -> jboolean,
    },
];

/// The one call that proves the whole path: what the library says it is.
fn version<'local>(env: &mut Env<'local>, _this: JObject<'local>) -> Result<JString<'local>, Error> {
    env.new_string((module().kind.version)())
}

/// Start a session. Returns a handle at once, or zero where the backend
/// could not even start; everything after this arrives on `events`.
///
/// `options` is the connection's options as key, value, key, value, over
/// the catalog's defaults. `lastWidth` × `lastHeight` is the size the far end
/// was left at last time where the app keeps that note, and zero where it
/// does not.
#[allow(clippy::too_many_arguments)]
fn create<'local>(
    env: &mut Env<'local>,
    _this: JObject<'local>,
    events: JObject<'local>,
    tag: JString<'local>,
    address: JString<'local>,
    user_name: JString<'local>,
    password: JString<'local>,
    options: JObjectArray<'local, JString<'local>>,
    screen_width: jint,
    screen_height: jint,
    last_width: jint,
    last_height: jint,
    client_name: JString<'local>,
    files_dir: JString<'local>,
) -> Result<jlong, Error> {
    crate::log::init(&string(env, &tag)?.unwrap_or_else(|| "NativeBackend".into()));
    let module = module();
    let kind = module.kind;

    let mut o = Options::defaults(kind.options);
    if !options.is_null() {
        let n = options.len(env)?;
        for i in (0..n.saturating_sub(1)).step_by(2) {
            let key = options.get_element(env, i)?;
            let value = options.get_element(env, i + 1)?;
            if let Some(key) = string(env, &key)? {
                o.set(&key, &string(env, &value)?.unwrap_or_default());
            }
        }
    }

    let outlet = Arc::new(Outlet::new(env, &events)?);
    let wake = Arc::clone(&outlet);
    let mailbox = Mailbox::new(Some(Box::new(move || wake.wake())));
    let _ = outlet.mailbox.set(Arc::downgrade(&mailbox));

    let address = string(env, &address)?.unwrap_or_default();
    let user = string(env, &user_name)?.unwrap_or_default();
    // Empty is not a password: something that looks like a stored secret
    // and is not one fails the connection instead of asking for the real one.
    let password = string(env, &password)?;
    let client_name = string(env, &client_name)?.unwrap_or_else(|| "Android".into());
    let files = PathBuf::from(string(env, &files_dir)?.unwrap_or_default());
    let dirs = Dirs {
        config: files.clone(),
        state: files,
        realvnc_libs: PathBuf::new(),
    };
    let pair = |w: jint, h: jint| (w > 0 && h > 0).then_some((w as u32, h as u32));
    let connect = Connect {
        address: &address,
        user: &user,
        domain: "",
        password: password.as_deref(),
        options: &o,
        mailbox: Arc::clone(&mailbox),
        dirs: &dirs,
        screen: pair(screen_width, screen_height),
        last_size: pair(last_width, last_height),
        client_name: &client_name,
        platform: Platform::Android,
        decoders: (module.decoders)(),
    };
    match kind.start(&connect) {
        Ok(backend) => Ok(Box::into_raw(Box::new(Handle {
            backend,
            _mailbox: mailbox,
        })) as jlong),
        Err(why) => {
            log::warn!("{}: {why}", kind.id);
            Ok(0)
        }
    }
}

/// A `null` or empty password cancels, which ends the session — the only
/// answer a prompt has besides a secret. The user name is what was typed, empty for
/// none; the adapter fills it from the session's and, for RDP, splits a
/// domain out of it.
fn answer_credentials<'local>(
    env: &mut Env<'local>,
    _this: JObject<'local>,
    handle: jlong,
    user_name: JString<'local>,
    password: JString<'local>,
) -> Result<(), Error> {
    let Some(backend) = (unsafe { backend(handle) }) else {
        return Ok(());
    };
    let user = string(env, &user_name)?.unwrap_or_default();
    // Empty is a cancel too, as it is for a stored password: nothing that
    // looks like a secret and is not one goes to the far end.
    let password = string(env, &password)?;
    backend.answer_credentials(password.map(|password| Credentials {
        user,
        domain: String::new(),
        password,
    }));
    Ok(())
}

fn answer_trust<'local>(_env: &mut Env<'local>, _this: JObject<'local>, handle: jlong, accept: jboolean) -> Result<(), Error> {
    if let Some(backend) = unsafe { backend(handle) } {
        backend.answer_trust(accept);
    }
    Ok(())
}

fn disconnect<'local>(_env: &mut Env<'local>, _this: JObject<'local>, handle: jlong) -> Result<(), Error> {
    if let Some(backend) = unsafe { backend(handle) } {
        backend.disconnect();
    }
    Ok(())
}

/// Ends the session, waits for its thread, and frees the handle. Nothing may
/// use it afterwards, which Java guarantees by retiring it first.
fn destroy<'local>(_env: &mut Env<'local>, _this: JObject<'local>, handle: jlong) -> Result<(), Error> {
    if handle != 0 {
        let handle = unsafe { Box::from_raw(handle as *mut Handle) };
        handle.backend.destroy();
    }
    Ok(())
}

fn pointer<'local>(
    _env: &mut Env<'local>,
    _this: JObject<'local>,
    handle: jlong,
    x: jint,
    y: jint,
    button_mask: jint,
) -> Result<(), Error> {
    if let Some(backend) = unsafe { backend(handle) } {
        backend.pointer(x, y, button_mask);
    }
    Ok(())
}

fn pointer_relative<'local>(
    _env: &mut Env<'local>,
    _this: JObject<'local>,
    handle: jlong,
    dx: jint,
    dy: jint,
    button_mask: jint,
) -> Result<(), Error> {
    if let Some(backend) = unsafe { backend(handle) } {
        backend.pointer_relative(dx, dy, button_mask);
    }
    Ok(())
}

fn key_down<'local>(
    _env: &mut Env<'local>,
    _this: JObject<'local>,
    handle: jlong,
    keysym: jint,
    key_id: jlong,
) -> Result<(), Error> {
    if let Some(backend) = unsafe { backend(handle) } {
        backend.key_down(keysym as u32, key_id);
    }
    Ok(())
}

fn key_up<'local>(_env: &mut Env<'local>, _this: JObject<'local>, handle: jlong, key_id: jlong) -> Result<(), Error> {
    if let Some(backend) = unsafe { backend(handle) } {
        backend.key_up(key_id);
    }
    Ok(())
}

fn release_all_keys<'local>(_env: &mut Env<'local>, _this: JObject<'local>, handle: jlong) -> Result<(), Error> {
    if let Some(backend) = unsafe { backend(handle) } {
        backend.release_all_keys();
    }
    Ok(())
}

fn focus<'local>(_env: &mut Env<'local>, _this: JObject<'local>, handle: jlong, focused: jboolean) -> Result<(), Error> {
    if let Some(backend) = unsafe { backend(handle) } {
        backend.focus(focused);
    }
    Ok(())
}

fn clipboard<'local>(env: &mut Env<'local>, _this: JObject<'local>, handle: jlong, text: JString<'local>) -> Result<(), Error> {
    if let Some(backend) = unsafe { backend(handle) }
        && let Some(text) = string(env, &text)?
    {
        backend.clipboard(&text);
    }
    Ok(())
}

/// One option, changed on the running session. What is live and what is
/// remembered for next time is the adapter's to say.
fn set_option<'local>(
    env: &mut Env<'local>,
    _this: JObject<'local>,
    handle: jlong,
    key: JString<'local>,
    value: JString<'local>,
) -> Result<(), Error> {
    if let Some(backend) = unsafe { backend(handle) }
        && let Some(key) = string(env, &key)?
    {
        backend.set_option(&key, &string(env, &value)?.unwrap_or_default());
    }
    Ok(())
}

fn request_desktop_size<'local>(
    _env: &mut Env<'local>,
    _this: JObject<'local>,
    handle: jlong,
    width: jint,
    height: jint,
) -> Result<(), Error> {
    if let Some(backend) = unsafe { backend(handle) } {
        backend.request_desktop_size(width.max(0) as u32, height.max(0) as u32);
    }
    Ok(())
}

fn request_display<'local>(_env: &mut Env<'local>, _this: JObject<'local>, handle: jlong, index: jint) -> Result<(), Error> {
    if let Some(backend) = unsafe { backend(handle) } {
        backend.request_display(index);
    }
    Ok(())
}

/// `session::backend::Facts` as ints, in one crossing: width, height, the
/// current display, flags (1 can resize, 2 view only, 4 pointer relative),
/// how many monitors, then four ints per monitor and four per display —
/// x, y, width, height, `Monitor.fromFlat`'s shape.
fn facts<'local>(env: &mut Env<'local>, _this: JObject<'local>, handle: jlong) -> Result<JIntArray<'local>, Error> {
    let Some(backend) = (unsafe { backend(handle) }) else {
        return Ok(JIntArray::null());
    };
    let f = backend.facts();
    let flags = f.can_resize as jint | (f.view_only as jint) << 1 | (f.pointer_relative as jint) << 2;
    let mut flat = vec![
        f.width as jint,
        f.height as jint,
        f.display,
        flags,
        f.monitors.len() as jint,
    ];
    for m in f.monitors.iter().chain(&f.displays) {
        flat.extend([m.x, m.y, m.width, m.height]);
    }
    let array = env.new_int_array(flat.len())?;
    array.set_region(env, 0, &flat)?;
    Ok(array)
}

/// The connection panel's rows, four strings each: the field's name as
/// `ConnectionFact.Field` spells it, the label, the value, and `"d"` for a
/// diagnostic or empty.
fn info<'local>(
    env: &mut Env<'local>,
    _this: JObject<'local>,
    handle: jlong,
) -> Result<JObjectArray<'local, JString<'local>>, Error> {
    let Some(backend) = (unsafe { backend(handle) }) else {
        return Ok(JObjectArray::<JString>::null());
    };
    let rows = backend.info();
    let array = JObjectArray::<JString>::new(env, rows.len() * 4, JString::null())?;
    for (i, row) in rows.iter().enumerate() {
        let cells = [field(row.field), row.label, &row.value, if row.diagnostic { "d" } else { "" }];
        for (j, cell) in cells.iter().enumerate() {
            let s = env.new_string(*cell)?;
            array.set_element(env, i * 4 + j, &s)?;
        }
    }
    Ok(array)
}

fn field(field: Field) -> &'static str {
    match field {
        Field::DesktopName => "DESKTOP_NAME",
        Field::Connection => "CONNECTION",
        Field::Security => "SECURITY",
        Field::Protocol => "PROTOCOL",
        Field::Encoding => "ENCODING",
        Field::LineSpeed => "LINE_SPEED",
        Field::Data => "DATA",
        Field::ServerPixels => "SERVER_PIXELS",
        Field::ViewerPixels => "VIEWER_PIXELS",
        Field::Other => "OTHER",
    }
}

/// Received and sent, in bytes, with -1 for a direction nobody counts; `null`
/// while there is nothing to count. Raw counts rather than the row, because
/// the row is worded in the phone's own number format.
fn traffic<'local>(env: &mut Env<'local>, _this: JObject<'local>, handle: jlong) -> Result<JLongArray<'local>, Error> {
    let Some(traffic) = (unsafe { backend(handle) }).and_then(|b| b.traffic()) else {
        return Ok(JLongArray::null());
    };
    let both = [traffic.received as jlong, traffic.sent.map_or(-1, |s| s as jlong)];
    let array = env.new_long_array(both.len())?;
    array.set_region(env, 0, &both)?;
    Ok(array)
}

/// Copy a desktop rectangle into `dst` at `(dst_x, dst_y)`, 1:1, leaving the
/// rest of `dst` alone — straight into the bitmap's rows, with no buffer in
/// between.
///
/// Called from the drawing thread, concurrently with everything else, which
/// every adapter is written for: the framebuffer is behind a reader-writer
/// lock and this takes the read side.
#[allow(clippy::too_many_arguments)]
fn read_region<'local>(
    env: jni::EnvUnowned<'local>,
    _this: JObject<'local>,
    handle: jlong,
    x: jint,
    y: jint,
    width: jint,
    height: jint,
    dst: JObject<'local>,
    dst_x: jint,
    dst_y: jint,
) -> jboolean {
    let Some(backend) = (unsafe { backend(handle) }) else {
        return false;
    };
    if x < 0 || y < 0 || width <= 0 || height <= 0 || dst_x < 0 || dst_y < 0 {
        return false;
    }
    let Some(mut locked) = (unsafe { bitmap::Locked::new(env.as_raw(), dst.as_raw()) }) else {
        return false;
    };
    let (width, height, dst_x, dst_y) = (width as usize, height as usize, dst_x as usize, dst_y as usize);
    if locked.width < dst_x + width || locked.height < dst_y + height {
        return false;
    }
    let stride = locked.stride;
    backend.read_region(x as usize, y as usize, width, height, locked.bytes(), stride, dst_x, dst_y)
}

/// The whole desktop at `1/step`, into a bitmap Java has already sized.
fn read_thumbnail<'local>(
    env: jni::EnvUnowned<'local>,
    _this: JObject<'local>,
    handle: jlong,
    step: jint,
    dst: JObject<'local>,
) -> jboolean {
    let Some(backend) = (unsafe { backend(handle) }) else {
        return false;
    };
    if step <= 0 {
        return false;
    }
    let Some(picture) = backend.thumbnail(step as usize) else {
        return false;
    };
    let Some(mut locked) = (unsafe { bitmap::Locked::new(env.as_raw(), dst.as_raw()) }) else {
        return false;
    };
    if locked.width < picture.width || locked.height < picture.height {
        return false;
    }
    let stride = locked.stride;
    let row = picture.width * 4;
    let bytes = locked.bytes();
    for (y, src) in picture.rgba.chunks_exact(row).enumerate() {
        bytes[y * stride..y * stride + row].copy_from_slice(src);
    }
    true
}

/// Where a session's events leave for Java: `NativeBackend`'s `on…` methods,
/// resolved once, and the mailbox they are drained from.
struct Outlet {
    vm: JavaVM,
    target: Global<JObject<'static>>,
    connected: JMethodID,
    desktop_size: JMethodID,
    frame: JMethodID,
    cursor: JMethodID,
    pointer_mode: JMethodID,
    bell: JMethodID,
    clipboard: JMethodID,
    credentials_needed: JMethodID,
    trust_needed: JMethodID,
    unverified: JMethodID,
    closed: JMethodID,
    mailbox: OnceLock<Weak<Mailbox>>,
    /// Held while a thread drains, so that two threads with events never
    /// deliver them out of order.
    draining: Mutex<()>,
    /// Set by a wake that found somebody else draining, and read by them once
    /// they have let go, so that what it was woken for is not left waiting
    /// for the next event.
    requested: AtomicBool,
}

impl Outlet {
    /// Resolved on the creating thread, against the object's own class: the
    /// protocol thread that delivers later has no class loader that could
    /// find `NativeBackend` by name, and resolving per call would be the most
    /// expensive thing in the path.
    fn new(env: &mut Env, target: &JObject) -> jni::errors::Result<Outlet> {
        let class = env.get_object_class(target)?;
        let id = |env: &mut Env, name, sig| env.get_method_id(&class, name, sig);
        Ok(Outlet {
            vm: env.get_java_vm()?,
            target: env.new_global_ref(target)?,
            connected: id(env, jni_str!("onConnected"), jni_sig!("(II)V"))?,
            desktop_size: id(env, jni_str!("onDesktopSize"), jni_sig!("(II)V"))?,
            frame: id(env, jni_str!("onFrame"), jni_sig!("([I)V"))?,
            cursor: id(env, jni_str!("onCursor"), jni_sig!("([IIIIIJ)V"))?,
            pointer_mode: id(env, jni_str!("onPointerMode"), jni_sig!("(Z)V"))?,
            bell: id(env, jni_str!("onBell"), jni_sig!("()V"))?,
            clipboard: id(env, jni_str!("onClipboard"), jni_sig!("(Ljava/lang/String;)V"))?,
            credentials_needed: id(env, jni_str!("onCredentialsNeeded"), jni_sig!("(Z)V"))?,
            trust_needed: id(env, jni_str!("onTrustNeeded"), jni_sig!("(Ljava/lang/String;)V"))?,
            unverified: id(env, jni_str!("onUnverified"), jni_sig!("(Ljava/lang/String;)V"))?,
            closed: id(env, jni_str!("onClosed"), jni_sig!("(Ljava/lang/String;)V"))?,
            mailbox: OnceLock::new(),
            draining: Mutex::new(()),
            requested: AtomicBool::new(false),
        })
    }

    /// The mailbox's wake: deliver everything waiting, on this thread.
    ///
    /// A wake on a thread that is already delivering — a callback that
    /// answers straight back into the session, which can queue another
    /// event — finds the lock held, and the loop below it takes the new
    /// event on its next turn; so does one on another thread. Nothing here
    /// blocks, which a protocol thread holding its own locks needs.
    fn wake(&self) {
        let Some(mailbox) = self.mailbox.get().and_then(Weak::upgrade) else {
            return;
        };
        self.requested.store(true, Ordering::SeqCst);
        loop {
            let Ok(guard) = self.draining.try_lock() else {
                return;
            };
            self.requested.store(false, Ordering::SeqCst);
            while let Some(event) = mailbox.take() {
                self.deliver(event);
            }
            drop(guard);
            if !self.requested.load(Ordering::SeqCst) {
                return;
            }
        }
    }

    /// One event, as one call. A Java exception thrown out of a callback
    /// becomes a log line rather than an abort: the protocol thread is in
    /// the middle of a framebuffer update and unwinding it would lose the
    /// connection over a bug on the other side of the boundary.
    fn deliver(&self, event: Event) {
        let result: jni::errors::Result<()> = self.vm.attach_current_thread(|env| {
            let int = |v: usize| jvalue { i: v as jint };
            match event {
                Event::Connected { width, height } => self.call(env, self.connected, &[int(width), int(height)]),
                Event::DesktopSize { width, height } => {
                    self.call(env, self.desktop_size, &[int(width), int(height)])
                }
                Event::Frame { rects, .. } => {
                    let flat: Vec<jint> = rects
                        .iter()
                        .flat_map(|r| [r.x, r.y, r.width, r.height].map(|v| v as jint))
                        .collect();
                    let array = env.new_int_array(flat.len())?;
                    array.set_region(env, 0, &flat)?;
                    self.call(env, self.frame, &[jvalue { l: array.as_raw() }])
                }
                Event::Cursor(cursor) => {
                    // `Color`-packed for `Bitmap.setPixels`, and hashed here
                    // because the pixels are already in cache from the swap.
                    let (argb, w, h, hot_x, hot_y) = match cursor {
                        Some(c) => {
                            let argb: Vec<u32> = c
                                .rgba
                                .chunks_exact(4)
                                .map(|p| common::cursor::swap_rb(u32::from_le_bytes([p[0], p[1], p[2], p[3]])))
                                .collect();
                            (argb, c.width, c.height, c.hot_x, c.hot_y)
                        }
                        None => (Vec::new(), 0, 0, 0, 0),
                    };
                    let hash = common::cursor::hash(&argb, w, h) as jlong;
                    let ints: Vec<jint> = argb.iter().map(|&p| p as jint).collect();
                    let array = env.new_int_array(ints.len())?;
                    if !ints.is_empty() {
                        array.set_region(env, 0, &ints)?;
                    }
                    self.call(
                        env,
                        self.cursor,
                        &[
                            jvalue { l: array.as_raw() },
                            int(w),
                            int(h),
                            jvalue { i: hot_x },
                            jvalue { i: hot_y },
                            jvalue { j: hash },
                        ],
                    )
                }
                Event::PointerMode { relative } => self.call(env, self.pointer_mode, &[jvalue { z: relative }]),
                Event::Bell => self.call(env, self.bell, &[]),
                Event::Clipboard(text) => self.text(env, self.clipboard, &text),
                Event::CredentialsNeeded { needs_user } => {
                    self.call(env, self.credentials_needed, &[jvalue { z: needs_user }])
                }
                Event::TrustNeeded { fingerprint } => self.text(env, self.trust_needed, &fingerprint),
                Event::Unverified { why } => self.text(env, self.unverified, &why),
                Event::Closed { detail } => self.text(env, self.closed, &detail),
                // The C API's three, which no Rust backend asks.
                other @ (Event::Question { .. } | Event::SecurityNeeded { .. } | Event::Message { .. }) => {
                    log::warn!("not delivered on Android: {other:?}");
                    Ok(())
                }
            }
        });
        if let Err(e) = result {
            log::warn!("callback failed: {e}");
        }
    }

    fn call(&self, env: &mut Env, method: JMethodID, args: &[jvalue]) -> jni::errors::Result<()> {
        unsafe { env.call_method_unchecked(&self.target, method, ReturnType::Primitive(Primitive::Void), args) }?;
        Ok(())
    }

    fn text(&self, env: &mut Env, method: JMethodID, text: &str) -> jni::errors::Result<()> {
        let s = env.new_string(text)?;
        self.call(env, method, &[jvalue { l: s.as_raw() }])
    }
}
