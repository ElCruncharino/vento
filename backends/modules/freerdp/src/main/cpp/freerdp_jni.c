// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

// `FreeRdpNative`'s entry points: what the Java class declares that
// `freerdp_session.c` does not already answer through the neutral vtable.
// The uniform natives are stamped out by the macro at the bottom over the
// adapter in `backends/platform/android/jni`, which is where the JNI-side
// decisions — thread attachment, string encodings, the cursor's packing and
// hash, the `Bitmap` lock — are made and documented. What is left here is
// the one call that takes this backend's own parameter list, and the one
// extra it has.
//
// One thing about this library is Android's rather than the protocol's, and
// it cost a session to find out: **nothing of ours may define `JNI_OnLoad`.**
// WinPR exports one of its own and takes its `JavaVM` from it, because it
// reads the timezone through Java. It is in the same object as the `JavaVM`
// the timezone code reads, so a second one here does not replace it: the link
// fails on a duplicate symbol, and anything a shim wants done at load time
// belongs somewhere else — which is why the adapter resolves its method ids on
// the first `nativeCreate` rather than at load.

#include <jni.h>

#include <stdlib.h>

#include <vento/freerdp.h>

#include "vento_jni_natives.h"

#define CLASS Java_net_pgaskin_remotedesktop_backend_freerdp_FreeRdpNative

JNIEXPORT jstring JNICALL
Java_net_pgaskin_remotedesktop_backend_freerdp_FreeRdpNative_nativeVersion(JNIEnv *env,
                                                                          jclass cls) {
    (void) cls;
    return (*env)->NewStringUTF(env, vento_freerdp_version());
}

JNIEXPORT jlong JNICALL
Java_net_pgaskin_remotedesktop_backend_freerdp_FreeRdpNative_nativeCreate(
        JNIEnv *env, jclass cls, jobject listener, jstring address, jstring userName,
        jstring domain, jstring password, jstring nla, jboolean legacyTls, jboolean compression,
        jstring graphics, jstring experience, jstring sound, jint scale, jint width, jint height,
        jint monitors, jint keyboardLayout, jstring clientName, jstring configPath,
        jint connectTimeoutMs) {
    (void) cls;
    vento_jni_session *h = vento_jni_begin(env, listener);
    if (!h) {
        return 0;
    }
    char *strings[] = {
            vento_jni_string(env, address),    vento_jni_string(env, userName),
            vento_jni_string(env, domain),     vento_jni_string(env, password),
            vento_jni_string(env, nla),        vento_jni_string(env, graphics),
            vento_jni_string(env, experience), vento_jni_string(env, sound),
            vento_jni_string(env, clientName), vento_jni_string(env, configPath),
    };
    const vento_freerdp_config config = {
            .address = strings[0],
            .user_name = strings[1],
            .domain = strings[2],
            .password = strings[3],
            .nla = strings[4],
            .legacy_tls = legacyTls != JNI_FALSE,
            .compression = compression != JNI_FALSE,
            .graphics = strings[5],
            .experience = strings[6],
            .sound = strings[7],
            .scale = scale,
            .width = width,
            .height = height,
            .monitors = monitors,
            .keyboard_layout = keyboardLayout,
            .client_name = strings[8],
            .config_path = strings[9],
            .connect_timeout_ms = connectTimeoutMs,
    };
    vento_session *s = vento_freerdp_create(&config, vento_jni_callbacks(h));
    for (size_t i = 0; i < sizeof(strings) / sizeof(strings[0]); i++) {
        free(strings[i]);
    }
    return vento_jni_finish(env, h, s);
}

JNIEXPORT void JNICALL
Java_net_pgaskin_remotedesktop_backend_freerdp_FreeRdpNative_nativeSetMonitorCount(
        JNIEnv *env, jclass cls, jlong handle, jint count) {
    (void) env;
    (void) cls;
    vento_session *s = vento_jni_session_of(handle);
    if (s) {
        vento_freerdp_set_monitor_count(s, count);
    }
}

VENTO_JNI_NATIVES(CLASS, nativeAnswerTrust, DOMAIN)
