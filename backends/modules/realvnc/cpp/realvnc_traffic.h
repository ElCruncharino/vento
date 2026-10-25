// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

// The byte count this session has received, which the viewer core's JNI
// surface does not offer: none of its 320 exported functions returns one. The
// number is in the core's own bandwidth estimator (the class its logger calls
// `LineSpeedEstimator`), and this file walks the pointers to it.
//
// From the session pointer, with every offset read out of `getLineSpeed`:
//
//     A   = *(token + 0x1c8)        token is the AndroidSession*
//     C   = *(A     + 0x1c8)        null until there is a connection
//     W   = C + 0x2e8               the estimator's owner
//     Est = W + 8                   the estimator, six words and two flags
//
//     Est+0x00  s64  elapsed in the current window, 100 ns units
//     Est+0x08  s64  kilobits accumulated in the current window
//     Est+0x20  s64  round-trip time, 100 ns units
//     Est+0x28  u32  the stream position the current window started at
//
// The window is the framebuffer-update request/complete pair, so the mark moves
// several times a second in a live session; it is cumulative and monotonic, and
// it is 32 bits, so a total has to be accumulated from deltas by the caller.
//
// **Nothing here dereferences an address before proving it is mapped.** A walk
// through a third-party stripped binary is a dependency `check-jni-abi.sh`
// cannot see (the library is pinned by SHA-256, but a pin does not say what is
// at +0x1c8), and a wrong offset would crash the user's session with SIGSEGV
// rather than produce an error that can be reported. write(2) returns EFAULT
// for an unreadable source instead of raising a signal, so every hop is copied
// through a pipe: the bytes read back out are the bytes the kernel validated
// on the way in, which leaves no window between the check and the read.
//
// A header of `static` functions rather than a library, deliberately: two
// translation units need the walk (`realvnc_traffic.c`, the JNI entry
// `RealVncTraffic` reads it through on the phone, and `realvnc_session.cxx`,
// which reads it for `traffic` on the host), and the phone's library has to
// stay byte for byte what it was when the walk and the entry were one file.
// Including the header into each, with its own copy of the state, does that.

#ifndef VENTO_REALVNC_TRAFFIC_H
#define VENTO_REALVNC_TRAFFIC_H

#include <fcntl.h>
#include <pthread.h>
#include <stdint.h>
#include <string.h>
#include <unistd.h>

#define HOP_A 0x1c8
#define HOP_C 0x1c8
#define OWNER 0x2e8
#define EST 8

#define EST_ELAPSED 0x00
#define EST_KILOBITS 0x08
#define EST_RTT 0x20
#define EST_MARK 0x28
#define EST_BYTES 0x30

static pthread_mutex_t probe_lock = PTHREAD_MUTEX_INITIALIZER;
static int probe_pipe[2] = {-1, -1};

// Both ends non-blocking: a full pipe or an empty one must fail the read rather
// than park the session thread in the kernel.
static int probe_open(void) {
    if (probe_pipe[0] < 0 && pipe2(probe_pipe, O_CLOEXEC | O_NONBLOCK) != 0) {
        return 0;
    }
    return probe_pipe[0] >= 0;
}

/** Copy len bytes from addr, or fail if any of them is unmapped. */
static int probe_read(void *dst, uintptr_t addr, size_t len) {
    int ok = 0;
    char sink[EST_BYTES];
    pthread_mutex_lock(&probe_lock);
    if (probe_open()) {
        const ssize_t n = write(probe_pipe[1], (const void *) addr, len);
        if (n == (ssize_t) len) {
            ok = read(probe_pipe[0], dst, len) == (ssize_t) len;
        } else if (n > 0) {
            read(probe_pipe[0], sink, (size_t) n); // a short write leaves the pipe dirty
        }
    }
    pthread_mutex_unlock(&probe_lock);
    return ok;
}

// A misaligned pointer is not one of the core's, even if it is mapped: under
// LP64 every member of this chain is 8-byte aligned.
static int hop(uintptr_t base, size_t offset, uintptr_t *out) {
    uintptr_t value;
    if (!probe_read(&value, base + offset, sizeof value)) {
        return 0;
    }
    *out = value;
    return value != 0 && (value & 7) == 0;
}

/** Finds the estimator of the session with this token (not zero: the caller
    has a session): the start address of its `EST_BYTES`, which the caller
    then copies out with `probe_read`, since that address is as unproven as
    the hops were. Returns 0 where any hop is unreadable, as it is until there
    is a connection.

    The caller does the read, rather than this function, only to keep the
    phone's library byte for byte what it was when the walk and the JNI entry
    were one file: the only split that does (checked in the assembly, on both
    ABIs) has the entry declare the buffer after the walk, as it always did. */
static int realvnc_estimator_at(uintptr_t token, uintptr_t *est) {
    uintptr_t a, c;
    if (!hop(token, HOP_A, &a) || !hop(a, HOP_C, &c)) {
        return 0;
    }
    *est = c + OWNER + EST;
    return 1;
}

#endif
