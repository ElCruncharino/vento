// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

// FreeRDP's H.264 decoded by vento's decoders (vento/video.h) rather than by
// a decoder of the library's own: what the session and the module's H.264
// subsystem (h264_vento.c) share.
//
// The library picks its H.264 decoder from a table of subsystems, one per
// decoder it was built with, and patches/freerdp adds a slot to it for one
// the application links in, tried first; this module's is the only one the
// library has. What it does is the decoder's half of the library's
// interface: a frame in, its picture out as I420 planes, which the library's
// own code then combines (AVC444's two pictures into one) and converts into
// the surface, exactly as it does with the planes of the decoders it ships.
//
// A subsystem is made with nothing but the library's H.264 context, which
// knows nothing of the session, so the session says whose contexts are being
// made for as long as it is inside the graphics pipeline's surface command —
// which is where the library makes, resets and feeds them — and the
// subsystem keeps that for the context's life.

#ifndef VENTO_FREERDP_H264_VENTO_H
#define VENTO_FREERDP_H264_VENTO_H

#include <pthread.h>
#include <stdatomic.h>
#include <stdbool.h>
#include <stddef.h>

#include <vento/video.h>

/* One session's H.264: the platform's decoders, and what is known about the
   picture they last decoded. Every member is safe from any thread. */
typedef struct VentoH264 {
    vento_video_decoders table;
    bool available;
    /* Decoders holding one of the hardware places (VENTO_H264_MAX_HARDWARE). */
    atomic_int hardwareHeld;
    /* Whether the last picture was hardware's: 1, 0, or -1 before any. */
    atomic_int hardware;
    /* Set where a decoder can no longer follow the stream it is part of: it
       failed mid-stream and was replaced, or the person changed how video
       is decoded. Taken by the session, which asks the server to start the
       graphics pipeline again. */
    atomic_bool restart;
    /* A hardware decoder failed mid-stream: the decoders opened for the new
       stream are software, so that a device that fails once is not tried
       again until the person changes the mode. */
    atomic_bool failed;
    pthread_mutex_t lock;
    char status[160]; /* under `lock`: the last decoder's line, "" for none */
} VentoH264;

/* A server sends a surface per monitor, and a GPU has a handful of decode
   sessions to give: a context past these decodes in software, as TigerVNC's
   past the same number do. */
#define VENTO_H264_MAX_HARDWARE 4

/* The library's own symbols, not the backend's: only vento/freerdp.h's are
   exported. */
#pragma GCC visibility push(hidden)

/* `table`, which is the session's from the call on whatever comes back;
   false, with it released, where it is NULL or cannot decode planes. */
bool vento_h264_init(VentoH264 *v, const vento_video_decoders *table);

/* After the last H.264 context is freed, which is after the GDI is. */
void vento_h264_release(VentoH264 *v);

/* The H.264 contexts this thread makes from here until `leave` are `v`'s. */
void vento_h264_enter(VentoH264 *v);
void vento_h264_leave(void);

/* The last decoder's line for the overlay, into `out` with its NUL. */
bool vento_h264_status(VentoH264 *v, char *out, size_t len);

#pragma GCC visibility pop

#endif
