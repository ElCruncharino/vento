// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

// The Android half of a C backend, written once: `vento_callbacks` over a Java
// `Callbacks` object, `vento_log` onto logcat, a `Bitmap` lock around
// `read_region` and `read_thumbnail`, and the bodies of the uniform `native*`
// entry points — which `vento_jni_natives.h` stamps out under a module's own
// mangled class name, so that a module's `*_jni.c` is its `nativeVersion`,
// its `nativeCreate`, its extras, and one macro.
//
// This is `backends/platform/android` for the C backends, as `callbacks.rs`
// and `bitmap.rs` there are for the Rust ones, and it makes the same
// decisions they do: the method ids are resolved once, the cursor is swapped
// into `Color`'s packing and hashed here, where the pixels are already in
// cache from the copy, and a Java exception thrown out of a callback costs a
// frame rather than the session.
//
// Compiled into each module's own library rather than shared between them —
// the CMake fragment in `backends/cmake/vento_jni.cmake` adds it as an object
// library — so each module's copy keeps its own method-id cache, which is the
// one per-process static here and is per `Callbacks` interface anyway.

#ifndef VENTO_JNI_H
#define VENTO_JNI_H

#include <jni.h>

#include <vento/backend.h>

#ifdef __cplusplus
extern "C" {
#endif

/* One session's Android side: the VM, a global reference to its `Callbacks`,
   and the `vento_callbacks` built over them. The `jlong` Java holds is a
   pointer to one of these. */
typedef struct vento_jni_session vento_jni_session;

/* The first half of a `nativeCreate`: resolves the `Callbacks` methods (once
   per module, from the first listener's class), takes a global reference to
   `listener`, and builds the callbacks a backend's `create` is given. NULL out
   of memory. */
vento_jni_session *vento_jni_begin(JNIEnv *env, jobject listener);

/* The callbacks `vento_jni_begin` built, valid until `vento_jni_finish`
   either adopts the session or gives up. */
const vento_callbacks *vento_jni_callbacks(const vento_jni_session *h);

/* The second half: the session the backend created, or NULL, which frees the
   adapter's half again and yields 0. Returns the handle Java keeps. */
jlong vento_jni_finish(JNIEnv *env, vento_jni_session *h, vento_session *s);

/* The session behind a handle, for a module's own extras; NULL for 0. */
vento_session *vento_jni_session_of(jlong handle);

/* A UTF-8 copy of a Java string — real UTF-8, through UTF-16, rather than
   `GetStringUTFChars`' modified one — or NULL for null. The caller frees it. */
char *vento_jni_string(JNIEnv *env, jstring js);

/* The bodies of the uniform natives, in `vento_jni_natives.h`'s order. Each
   tolerates a handle of 0. */
void vento_jni_answer_credentials(JNIEnv *env, jlong handle, jstring user_name, jstring domain,
                                  jstring password);
void vento_jni_answer_trust(jlong handle, jboolean accept);
void vento_jni_disconnect(jlong handle);
/* Joins the protocol thread, then drops the Java reference: `closed` is the
   last callback and `destroy` returns after it, so the reference is provably
   unused by then. Java retires the handle under its own lock before calling
   this, which is the discipline every backend here follows. */
void vento_jni_destroy(JNIEnv *env, jlong handle);
void vento_jni_pointer(jlong handle, jint x, jint y, jint button_mask);
void vento_jni_key_down(jlong handle, jint keysym, jlong key_id);
void vento_jni_key_up(jlong handle, jlong key_id);
void vento_jni_release_all_keys(jlong handle);
void vento_jni_focus(jlong handle, jboolean focused);
void vento_jni_view_only(jlong handle, jboolean view_only);
jboolean vento_jni_can_resize(jlong handle);
jintArray vento_jni_monitors(JNIEnv *env, jlong handle);
void vento_jni_request_desktop_size(jlong handle, jint width, jint height);
void vento_jni_clipboard(JNIEnv *env, jlong handle, jstring text);
jobjectArray vento_jni_info(JNIEnv *env, jlong handle);
jlongArray vento_jni_traffic(JNIEnv *env, jlong handle);
jboolean vento_jni_read_region(JNIEnv *env, jlong handle, jint x, jint y, jint width, jint height,
                               jobject bitmap, jint dst_x, jint dst_y);
jboolean vento_jni_read_thumbnail(JNIEnv *env, jlong handle, jint step, jobject bitmap);

#ifdef __cplusplus
}
#endif

#endif
