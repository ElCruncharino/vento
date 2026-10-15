// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

// The uniform `native*` entry points of a `*Native` class, stamped out under
// its mangled name. A module's own file keeps `nativeVersion`, `nativeCreate`
// and its extras, because those are where the modules differ; everything
// here is the same shape in all three Java classes, and each body is one call
// into `vento_jni.c`.
//
//   VENTO_JNI_NATIVES(Java_net_pgaskin_remotedesktop_backend_libvnc_LibVncNative,
//                     nativeAnswerTrust, PLAIN)
//
// The second argument is the name of the method that answers the identity
// questions, `nativeAnswerTrust` in every module today, which reaches
// `vento_session_answer_trust`. The third is `PLAIN` for a `nativeAnswerCredentials(handle, user, password)`
// and `DOMAIN` for FreeRDP's, which carries one.
//
// A native binds by mangled name, so a missing one here fails the first time
// it is called rather than at link; `scripts/check-jni-symbols.sh` is what
// asks whether every declared method has its symbol.

#ifndef VENTO_JNI_NATIVES_H
#define VENTO_JNI_NATIVES_H

#include "vento_jni.h"

#ifdef __cplusplus
#define VENTO_JNI_EXPORT extern "C" JNIEXPORT
#else
#define VENTO_JNI_EXPORT JNIEXPORT
#endif

#define VENTO_JNI_CREDENTIALS_PLAIN(CLASS) \
    VENTO_JNI_EXPORT void JNICALL CLASS##_nativeAnswerCredentials( \
            JNIEnv *env, jclass cls, jlong handle, jstring userName, jstring password) { \
        (void) cls; \
        vento_jni_answer_credentials(env, handle, userName, NULL, password); \
    }

#define VENTO_JNI_CREDENTIALS_DOMAIN(CLASS) \
    VENTO_JNI_EXPORT void JNICALL CLASS##_nativeAnswerCredentials( \
            JNIEnv *env, jclass cls, jlong handle, jstring userName, jstring domain, \
            jstring password) { \
        (void) cls; \
        vento_jni_answer_credentials(env, handle, userName, domain, password); \
    }

/* One level of indirection, so that a `CLASS` given as a macro is expanded
   before it is pasted: `##` takes its operands as written. */
#define VENTO_JNI_NATIVES(CLASS, ANSWER_TRUST, CREDENTIALS) \
    VENTO_JNI_NATIVES_(CLASS, ANSWER_TRUST, CREDENTIALS)

#define VENTO_JNI_NATIVES_(CLASS, ANSWER_TRUST, CREDENTIALS) \
    VENTO_JNI_CREDENTIALS_##CREDENTIALS(CLASS) \
    VENTO_JNI_EXPORT void JNICALL CLASS##_##ANSWER_TRUST( \
            JNIEnv *env, jclass cls, jlong handle, jboolean accept) { \
        (void) env; \
        (void) cls; \
        vento_jni_answer_trust(handle, accept); \
    } \
    VENTO_JNI_EXPORT void JNICALL CLASS##_nativeDisconnect(JNIEnv *env, jclass cls, jlong handle) { \
        (void) env; \
        (void) cls; \
        vento_jni_disconnect(handle); \
    } \
    VENTO_JNI_EXPORT void JNICALL CLASS##_nativeDestroy(JNIEnv *env, jclass cls, jlong handle) { \
        (void) cls; \
        vento_jni_destroy(env, handle); \
    } \
    VENTO_JNI_EXPORT void JNICALL CLASS##_nativePointer( \
            JNIEnv *env, jclass cls, jlong handle, jint x, jint y, jint buttonMask) { \
        (void) env; \
        (void) cls; \
        vento_jni_pointer(handle, x, y, buttonMask); \
    } \
    VENTO_JNI_EXPORT void JNICALL CLASS##_nativeKeyDown( \
            JNIEnv *env, jclass cls, jlong handle, jint keysym, jlong keyId) { \
        (void) env; \
        (void) cls; \
        vento_jni_key_down(handle, keysym, keyId); \
    } \
    VENTO_JNI_EXPORT void JNICALL CLASS##_nativeKeyUp( \
            JNIEnv *env, jclass cls, jlong handle, jlong keyId) { \
        (void) env; \
        (void) cls; \
        vento_jni_key_up(handle, keyId); \
    } \
    VENTO_JNI_EXPORT void JNICALL CLASS##_nativeReleaseAllKeys( \
            JNIEnv *env, jclass cls, jlong handle) { \
        (void) env; \
        (void) cls; \
        vento_jni_release_all_keys(handle); \
    } \
    VENTO_JNI_EXPORT void JNICALL CLASS##_nativeFocus( \
            JNIEnv *env, jclass cls, jlong handle, jboolean focused) { \
        (void) env; \
        (void) cls; \
        vento_jni_focus(handle, focused); \
    } \
    VENTO_JNI_EXPORT void JNICALL CLASS##_nativeViewOnly( \
            JNIEnv *env, jclass cls, jlong handle, jboolean viewOnly) { \
        (void) env; \
        (void) cls; \
        vento_jni_view_only(handle, viewOnly); \
    } \
    VENTO_JNI_EXPORT jboolean JNICALL CLASS##_nativeCanResize( \
            JNIEnv *env, jclass cls, jlong handle) { \
        (void) env; \
        (void) cls; \
        return vento_jni_can_resize(handle); \
    } \
    VENTO_JNI_EXPORT jintArray JNICALL CLASS##_nativeMonitors( \
            JNIEnv *env, jclass cls, jlong handle) { \
        (void) cls; \
        return vento_jni_monitors(env, handle); \
    } \
    VENTO_JNI_EXPORT void JNICALL CLASS##_nativeRequestDesktopSize( \
            JNIEnv *env, jclass cls, jlong handle, jint width, jint height) { \
        (void) env; \
        (void) cls; \
        vento_jni_request_desktop_size(handle, width, height); \
    } \
    VENTO_JNI_EXPORT void JNICALL CLASS##_nativeClipboard( \
            JNIEnv *env, jclass cls, jlong handle, jstring text) { \
        (void) cls; \
        vento_jni_clipboard(env, handle, text); \
    } \
    VENTO_JNI_EXPORT jobjectArray JNICALL CLASS##_nativeInfo( \
            JNIEnv *env, jclass cls, jlong handle) { \
        (void) cls; \
        return vento_jni_info(env, handle); \
    } \
    VENTO_JNI_EXPORT jlongArray JNICALL CLASS##_nativeTraffic( \
            JNIEnv *env, jclass cls, jlong handle) { \
        (void) cls; \
        return vento_jni_traffic(env, handle); \
    } \
    VENTO_JNI_EXPORT jboolean JNICALL CLASS##_nativeReadRegion( \
            JNIEnv *env, jclass cls, jlong handle, jint x, jint y, jint w, jint h, \
            jobject bitmap, jint dstX, jint dstY) { \
        (void) cls; \
        return vento_jni_read_region(env, handle, x, y, w, h, bitmap, dstX, dstY); \
    } \
    VENTO_JNI_EXPORT jboolean JNICALL CLASS##_nativeReadThumbnail( \
            JNIEnv *env, jclass cls, jlong handle, jint step, jobject bitmap) { \
        (void) cls; \
        return vento_jni_read_thumbnail(env, handle, step, bitmap); \
    }

#endif
