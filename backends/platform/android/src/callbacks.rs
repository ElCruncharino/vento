// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The Java side of a session, called from the protocol thread.
//!
//! Every method id is looked up once, at create time, on the thread that has an
//! `Env` already. `damaged` runs hundreds of times a frame, and resolving a
//! method by name and signature on each of those would be the most expensive
//! thing in the path.
//!
//! One class per backend declares the interface and they are identical, so this
//! takes the name: `RfbNative$Callbacks` and `IronRdpNative$Callbacks` are the
//! same shape because they are answering the same `Backend`. That shape is
//! `common::Events`, method for method, and this is its JNI implementation:
//! what a protocol asks goes to Java as a question, and what Java answers
//! comes back through `nativeAnswerCredentials` and `nativeAnswerTrust` to the
//! `common::Session` behind the handle. Where the protocols genuinely differ —
//! RDP's credentials carry a domain — the difference is in what is sent
//! *back*, not in what is called here.

use common::Events;
use jni::objects::{Global, JObject};
use jni::signature::{Primitive, ReturnType};
use jni::sys::{jint, jvalue};
use jni::{Env, JavaVM, jni_sig, jni_str};

/// The interface `RfbNative.Callbacks` declares, resolved.
pub struct Callbacks {
    vm: JavaVM,
    target: Global<JObject<'static>>,
    connected: jni::objects::JMethodID,
    desktop_size: jni::objects::JMethodID,
    damaged: jni::objects::JMethodID,
    frame_end: jni::objects::JMethodID,
    cursor: jni::objects::JMethodID,
    pointer_mode: jni::objects::JMethodID,
    bell: jni::objects::JMethodID,
    clipboard: jni::objects::JMethodID,
    credentials_needed: jni::objects::JMethodID,
    trust_needed: jni::objects::JMethodID,
    unverified: jni::objects::JMethodID,
    closed: jni::objects::JMethodID,
}

impl Callbacks {
    pub fn new(
        env: &mut Env,
        target: &JObject,
        class_name: &jni::strings::JNIStr,
    ) -> jni::errors::Result<Callbacks> {
        let class = env.find_class(class_name)?;
        Ok(Callbacks {
            vm: env.get_java_vm()?,
            target: env.new_global_ref(target)?,
            connected: env.get_method_id(&class, jni_str!("onConnected"), jni_sig!("(II)V"))?,
            desktop_size: env.get_method_id(
                &class,
                jni_str!("onDesktopSize"),
                jni_sig!("(II)V"),
            )?,
            damaged: env.get_method_id(&class, jni_str!("onDamage"), jni_sig!("(IIII)V"))?,
            frame_end: env.get_method_id(&class, jni_str!("onFrameEnd"), jni_sig!("()V"))?,
            cursor: env.get_method_id(&class, jni_str!("onCursor"), jni_sig!("([IIIIIJ)V"))?,
            pointer_mode: env.get_method_id(&class, jni_str!("onPointerMode"), jni_sig!("(Z)V"))?,
            bell: env.get_method_id(&class, jni_str!("onBell"), jni_sig!("()V"))?,
            clipboard: env.get_method_id(
                &class,
                jni_str!("onClipboard"),
                jni_sig!("(Ljava/lang/String;)V"),
            )?,
            credentials_needed: env.get_method_id(
                &class,
                jni_str!("onCredentialsNeeded"),
                jni_sig!("(Z)V"),
            )?,
            trust_needed: env.get_method_id(
                &class,
                jni_str!("onTrustNeeded"),
                jni_sig!("(Ljava/lang/String;)V"),
            )?,
            unverified: env.get_method_id(
                &class,
                jni_str!("onUnverified"),
                jni_sig!("(Ljava/lang/String;)V"),
            )?,
            closed: env.get_method_id(
                &class,
                jni_str!("onClosed"),
                jni_sig!("(Ljava/lang/String;)V"),
            )?,
        })
    }

    /// Attach this thread and make one void call.
    ///
    /// A Java exception thrown out of a callback becomes a log line rather than
    /// an abort: the protocol thread is in the middle of a framebuffer update
    /// and unwinding it would lose the connection over a bug on the other side
    /// of the boundary. The same argument `jni`'s `LogErrorAndDefault` makes for
    /// the inbound direction.
    fn call(&self, method: jni::objects::JMethodID, args: &[jvalue]) {
        let result: jni::errors::Result<()> = self.vm.attach_current_thread(|env| {
            unsafe {
                env.call_method_unchecked(
                    &self.target,
                    method,
                    ReturnType::Primitive(Primitive::Void),
                    args,
                )
            }?;
            Ok(())
        });
        if let Err(e) = result {
            log::warn!("callback failed: {e}");
        }
    }

    fn with_string(&self, method: jni::objects::JMethodID, text: &str) {
        let result: jni::errors::Result<()> = self.vm.attach_current_thread(|env| {
            let s = env.new_string(text)?;
            unsafe {
                env.call_method_unchecked(
                    &self.target,
                    method,
                    ReturnType::Primitive(Primitive::Void),
                    &[jvalue { l: s.as_raw() }],
                )
            }?;
            Ok(())
        });
        if let Err(e) = result {
            log::warn!("callback failed: {e}");
        }
    }
}

impl Events for Callbacks {
    fn connected(&self, width: usize, height: usize) {
        self.call(self.connected, &[int(width as i32), int(height as i32)]);
    }

    fn desktop_size(&self, width: usize, height: usize) {
        self.call(self.desktop_size, &[int(width as i32), int(height as i32)]);
    }

    fn damaged(&self, x: usize, y: usize, width: usize, height: usize) {
        self.call(
            self.damaged,
            &[
                int(x as i32),
                int(y as i32),
                int(width as i32),
                int(height as i32),
            ],
        );
    }

    fn frame_end(&self) {
        self.call(self.frame_end, &[]);
    }

    /// `pixels` is the framebuffer's own `R, G, B, A` word order; Java's
    /// `Bitmap.setPixels` wants `Color`'s `0xAARRGGBB`, so red and blue swap
    /// here rather than on a thread that is drawing — and the hash is taken
    /// here too, because the pixels are already in cache from the swap.
    fn cursor(&self, pixels: &[u32], width: usize, height: usize, hot_x: i32, hot_y: i32) {
        let argb: Vec<u32> = pixels.iter().map(|&p| common::cursor::swap_rb(p)).collect();
        let hash = common::cursor::hash(&argb, width, height) as i64;
        let argb: Vec<jint> = argb.iter().map(|&p| p as jint).collect();
        let result: jni::errors::Result<()> = self.vm.attach_current_thread(|env| {
            let array = env.new_int_array(argb.len())?;
            if !argb.is_empty() {
                array.set_region(env, 0, &argb)?;
            }
            unsafe {
                env.call_method_unchecked(
                    &self.target,
                    self.cursor,
                    ReturnType::Primitive(Primitive::Void),
                    &[
                        jvalue { l: array.as_raw() },
                        int(width as i32),
                        int(height as i32),
                        int(hot_x),
                        int(hot_y),
                        jvalue { j: hash },
                    ],
                )
            }?;
            Ok(())
        });
        if let Err(e) = result {
            log::warn!("cursor callback failed: {e}");
        }
    }

    fn pointer_mode(&self, relative: bool) {
        self.call(self.pointer_mode, &[jvalue { z: relative }]);
    }

    fn bell(&self) {
        self.call(self.bell, &[]);
    }

    fn clipboard(&self, text: &str) {
        self.with_string(self.clipboard, text);
    }

    /// Returns nothing itself — the answer arrives through
    /// `Session::answer_credentials`, because on the other side of this is a
    /// dialog and a person.
    fn credentials_needed(&self, needs_user: bool) {
        self.call(self.credentials_needed, &[jvalue { z: needs_user }]);
    }

    /// Answered through `Session::answer_trust`; the app is what holds the pin
    /// store, so the decision is not ours to take here.
    fn trust_needed(&self, fingerprint: &str) {
        self.with_string(self.trust_needed, fingerprint);
    }

    fn unverified(&self, why: &str) {
        self.with_string(self.unverified, why);
    }

    fn closed(&self, detail: &str) {
        self.with_string(self.closed, detail);
    }
}

fn int(v: i32) -> jvalue {
    jvalue { i: v }
}
