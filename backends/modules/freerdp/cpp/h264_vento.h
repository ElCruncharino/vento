// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

// FreeRDP's H.264 decoded by vento's decoders (vento/video.h) instead of one
// of the library's own; shared by the session and the module's H.264
// subsystem (h264_vento.c).
//
// The library picks its H.264 decoder from a table of subsystems, one per
// decoder it was built with, and patches/freerdp adds a slot to it, tried
// first, for a subsystem linked in by the application; this module's is the
// only one the library has. It implements the decoder's half of the library's
// interface: a frame in, its picture out as I420 planes, which the library's
// own code then combines (AVC444's two pictures into one) and converts into
// the surface, as it does with the planes from its built-in decoders.
//
// A subsystem is created with only the library's H.264 context, which doesn't
// know the session, so the session marks whose contexts are being created
// while it is inside the graphics pipeline's surface command (where the
// library creates, resets and feeds them), and the subsystem keeps that for
// the context's lifetime.

#ifndef VENTO_FREERDP_H264_VENTO_H
#define VENTO_FREERDP_H264_VENTO_H

#include <pthread.h>
#include <stdatomic.h>
#include <stdbool.h>
#include <stddef.h>

#include <vento/video.h>

/* One session's H.264: the platform's decoders, and the state of the picture
   they last decoded. Every member is safe to use from any thread. */
typedef struct VentoH264 {
    vento_video_decoders table;
    bool available;
    /* Decoders holding one of the hardware slots (VENTO_H264_MAX_HARDWARE). */
    atomic_int hardwareHeld;
    /* Whether the last picture was hardware's: 1, 0, or -1 before any. */
    atomic_int hardware;
    /* Set when a decoder loses track of its stream: it failed mid-stream and
       was replaced, or the user changed the video decoding mode. Cleared by
       the session, which asks the server to restart the graphics
       pipeline. */
    atomic_bool restart;
    /* A hardware decoder failed mid-stream: the decoders opened for the new
       stream are software, so a device that failed once is not retried until
       the user changes the mode. */
    atomic_bool failed;
    pthread_mutex_t lock;
    char status[160]; /* under `lock`: the last decoder's line, "" for none */
} VentoH264;

/* A server sends a surface per monitor, and a GPU has only a few decode
   sessions: contexts beyond this many decode in software, as TigerVNC's do
   beyond the same number. */
#define VENTO_H264_MAX_HARDWARE 4

/* Internal to the library, not part of the backend's interface: only
   vento/freerdp.h's symbols are exported. */
#pragma GCC visibility push(hidden)

/* Takes ownership of `table` whatever it returns; false, with the table
   released, if it is NULL or cannot decode planes. */
bool vento_h264_init(VentoH264 *v, const vento_video_decoders *table);

/* Called after the last H.264 context is freed, which is after the GDI is. */
void vento_h264_release(VentoH264 *v);

/* H.264 contexts created on this thread from here until `leave` belong to
   `v`. */
void vento_h264_enter(VentoH264 *v);
void vento_h264_leave(void);

/* Copies the last decoder's status line for the overlay into `out`,
   NUL-terminated. */
bool vento_h264_status(VentoH264 *v, char *out, size_t len);

#pragma GCC visibility pop

#endif
