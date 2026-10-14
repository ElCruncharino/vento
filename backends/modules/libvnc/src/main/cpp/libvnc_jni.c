// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

// `LibVncNative`'s entry points: what the Java class declares that
// `libvnc_session.c` does not already answer through the neutral vtable.
// The uniform natives are stamped out by the macro at the bottom over the
// adapter in `backends/platform/android/jni`, which is where the JNI-side
// decisions — thread attachment, string encodings, the cursor's packing and
// hash, the `Bitmap` lock — are made and documented. What is left here is
// the one call that takes this backend's own parameter list, and the one
// extra it has.

#include <jni.h>

#include <stdlib.h>

#include <vento/libvnc.h>

#include "vento_jni_natives.h"

#define CLASS Java_net_pgaskin_remotedesktop_backend_libvnc_LibVncNative

JNIEXPORT jstring JNICALL
Java_net_pgaskin_remotedesktop_backend_libvnc_LibVncNative_nativeVersion(JNIEnv *env, jclass cls) {
    (void) cls;
    return (*env)->NewStringUTF(env, vento_libvnc_version());
}

JNIEXPORT jlong JNICALL
Java_net_pgaskin_remotedesktop_backend_libvnc_LibVncNative_nativeCreate(
        JNIEnv *env, jclass cls, jobject listener, jstring address, jstring userName,
        jstring password, jboolean shared, jboolean anonymousTls, jstring encoding,
        jint compressLevel, jint qualityLevel, jint colorLevel, jint connectTimeoutMs) {
    (void) cls;
    vento_jni_session *h = vento_jni_begin(env, listener);
    if (!h) {
        return 0;
    }
    char *cAddress = vento_jni_string(env, address);
    char *cUser = vento_jni_string(env, userName);
    char *cPassword = vento_jni_string(env, password);
    char *cEncoding = vento_jni_string(env, encoding);
    const vento_libvnc_config config = {
            .address = cAddress,
            .user_name = cUser,
            .password = cPassword,
            .shared = shared != JNI_FALSE,
            .anonymous_tls = anonymousTls != JNI_FALSE,
            .encoding = cEncoding,
            .compress_level = compressLevel,
            .quality_level = qualityLevel,
            .color_level = colorLevel,
            .connect_timeout_ms = connectTimeoutMs,
    };
    vento_session *s = vento_libvnc_create(&config, vento_jni_callbacks(h));
    free(cAddress);
    free(cUser);
    free(cPassword);
    free(cEncoding);
    return vento_jni_finish(env, h, s);
}

JNIEXPORT void JNICALL
Java_net_pgaskin_remotedesktop_backend_libvnc_LibVncNative_nativeSetEncodings(
        JNIEnv *env, jclass cls, jlong handle, jstring encoding, jint compressLevel,
        jint qualityLevel) {
    (void) cls;
    vento_session *s = vento_jni_session_of(handle);
    if (!s) {
        return;
    }
    char *e = vento_jni_string(env, encoding);
    vento_libvnc_set_encodings(s, e, compressLevel, qualityLevel);
    free(e);
}

VENTO_JNI_NATIVES(CLASS, nativeAnswerTrust, PLAIN)
