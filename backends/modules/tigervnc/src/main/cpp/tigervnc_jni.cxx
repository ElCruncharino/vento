// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

// `TigerVncNative`'s entry points: what the Java class declares that
// `tigervnc_session.cxx` does not already answer through the neutral vtable.
// The uniform natives are stamped out by the macro at the bottom over the
// adapter in `backends/platform/android/jni`, which is where the JNI-side
// decisions — thread attachment, string encodings, the cursor's packing and
// hash, the `Bitmap` lock — are made and documented. What is left here is
// the one call that takes this backend's own parameter list and its one
// extra. `nativeAnswerTrust` answers all three of the session's questions —
// a certificate (`onTrustNeeded`, which Java takes to KnownHosts as every
// other backend's), no certificate at all, and an RSA-AES server key — since
// all three are `vento_session_answer_trust` at the protocol thread's end.

#include <jni.h>

#include <stdlib.h>

#include <vento/tigervnc.h>

#include "vento_jni_natives.h"

#define CLASS Java_net_pgaskin_remotedesktop_backend_tigervnc_TigerVncNative

#define NATIVE(ret, name) extern "C" JNIEXPORT ret JNICALL \
    Java_net_pgaskin_remotedesktop_backend_tigervnc_TigerVncNative_##name

NATIVE(jstring, nativeVersion)(JNIEnv *env, jclass) {
    return env->NewStringUTF(vento_tigervnc_version());
}

NATIVE(jlong, nativeCreate)(JNIEnv *env, jclass, jobject listener, jstring address,
                            jstring userName, jstring password, jboolean shared,
                            jboolean anonymousTls, jint encoding, jint compressLevel,
                            jint qualityLevel, jint colorLevel, jboolean h264,
                            jboolean audio, jint connectTimeoutMs) {
    vento_jni_session *h = vento_jni_begin(env, listener);
    if (h == nullptr) {
        return 0;
    }
    char *cAddress = vento_jni_string(env, address);
    char *cUser = vento_jni_string(env, userName);
    char *cPassword = vento_jni_string(env, password);
    vento_tigervnc_config config = {};
    config.address = cAddress;
    config.user_name = cUser;
    config.password = cPassword;
    config.shared = shared != JNI_FALSE;
    config.anonymous_tls = anonymousTls != JNI_FALSE;
    config.encoding = encoding;
    config.compress_level = compressLevel;
    config.quality_level = qualityLevel;
    config.color_level = colorLevel;
    config.h264 = h264 != JNI_FALSE;
    config.audio = audio != JNI_FALSE;
    config.connect_timeout_ms = connectTimeoutMs;
    vento_session *s = vento_tigervnc_create(&config, vento_jni_callbacks(h));
    free(cAddress);
    free(cUser);
    free(cPassword);
    return vento_jni_finish(env, h, s);
}

NATIVE(void, nativeSetPicture)(JNIEnv *, jclass, jlong handle, jint encoding,
                               jint compressLevel, jint qualityLevel, jint colorLevel,
                               jboolean h264) {
    vento_session *s = vento_jni_session_of(handle);
    if (s != nullptr) {
        vento_tigervnc_set_picture(s, encoding, compressLevel, qualityLevel, colorLevel,
                                   h264 != JNI_FALSE);
    }
}

VENTO_JNI_NATIVES(CLASS, nativeAnswerTrust, PLAIN)
