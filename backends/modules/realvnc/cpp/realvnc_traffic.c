// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

// `RealVncTraffic.nativeEstimator`: the pointer walk in `realvnc_traffic.h`,
// read by the phone's Java through a `long[]`. This is all of this module's
// native library on the phone, and it links only against bionic: the viewer
// core is already in the process, and the walk starts from a pointer the
// core hands to Java (`Session.getToken`) rather than from a symbol of the
// core's.

#define _GNU_SOURCE // pipe2

#include <jni.h>

#include "realvnc_traffic.h"

JNIEXPORT jboolean JNICALL
Java_net_pgaskin_remotedesktop_backend_realvnc_RealVncTraffic_nativeEstimator(
        JNIEnv *env, jclass clazz, jlong token, jlongArray out) {
    (void) clazz;
    if (token == 0 || out == NULL || (*env)->GetArrayLength(env, out) < 4) {
        return JNI_FALSE;
    }
    uintptr_t at;
    if (!realvnc_estimator_at((uintptr_t) token, &at)) {
        return JNI_FALSE;
    }
    unsigned char est[EST_BYTES];
    if (!probe_read(est, at, sizeof est)) {
        return JNI_FALSE;
    }

    int64_t elapsed, kilobits, rtt;
    uint32_t mark;
    memcpy(&elapsed, est + EST_ELAPSED, sizeof elapsed);
    memcpy(&kilobits, est + EST_KILOBITS, sizeof kilobits);
    memcpy(&rtt, est + EST_RTT, sizeof rtt);
    memcpy(&mark, est + EST_MARK, sizeof mark);

    const jlong values[] = {elapsed, kilobits, rtt, (jlong) mark};
    (*env)->SetLongArrayRegion(env, out, 0, 4, values);
    return JNI_TRUE;
}
