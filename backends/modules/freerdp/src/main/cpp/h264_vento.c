// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

// The module's H.264 subsystem for FreeRDP: the library's decoder interface
// (libfreerdp/codec/h264.h, H264_CONTEXT_SUBSYSTEM) over vento's decoders.
// See h264_vento.h for why it exists and how it finds its session.
//
// What the library asks of a subsystem is little: Init makes one for a
// context, Decompress takes one frame and leaves its picture's I420 planes
// in the context (pYUVData, iStride, YUVWidth, YUVHeight) for the library to
// read until the next frame, and Uninit undoes Init. Everything above that
// — the rectangles a frame updates, AVC444's combining of the main picture
// and the auxiliary one, the conversion into the surface — is the library's
// own code, the same whichever subsystem decoded. One context per surface,
// and AVC444's two pictures are two frames of the one stream through its
// one decoder (avc444_process_rects calls the same context twice), which is
// why the planes call hands back every picture in order and never a newer
// one in place of the one asked for.

#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

#include <freerdp/codec/h264.h>

/* The library's own declarations of the subsystem table and the context,
   which it keeps out of its public headers. */
#include "h264.h"

#include <vento/log.h>

#include "h264_vento.h"

#define TAG "FreeRdp"
#define LOGI(...) vento_log(VENTO_LOG_INFO, TAG, __VA_ARGS__)
#define LOGW(...) vento_log(VENTO_LOG_WARN, TAG, __VA_ARGS__)

/* The status line is asked for twice a second rather than every frame: it is
   formatted on the other side of the table each time. */
#define STATUS_EVERY_US 500000u

static _Thread_local VentoH264 *current;

typedef struct {
    VentoH264 *video;
    vento_video_decoder *decoder;
    UINT32 width, height; /* what the decoder was opened for */
    int mode;             /* the person's choice when it was */
    bool hardware;        /* whether it holds one of the hardware places */
    bool restartAsked;    /* for a change of mode, once */
    uint64_t lastStatus;
} Context;

static uint64_t nowMicros(void) {
    struct timespec t;
    clock_gettime(CLOCK_MONOTONIC, &t);
    return (uint64_t) t.tv_sec * 1000000u + (uint64_t) t.tv_nsec / 1000u;
}

bool vento_h264_init(VentoH264 *v, const vento_video_decoders *table) {
    memset(v, 0, sizeof(*v));
    atomic_init(&v->hardwareHeld, 0);
    atomic_init(&v->hardware, -1);
    atomic_init(&v->restart, false);
    atomic_init(&v->failed, false);
    pthread_mutex_init(&v->lock, NULL);
    if (table == NULL) {
        return false;
    }
    if (table->open == NULL || table->decode_planes == NULL || table->close == NULL) {
        if (table->release != NULL) {
            table->release(table->user);
        }
        return false;
    }
    v->table = *table;
    v->available = true;
    return true;
}

void vento_h264_release(VentoH264 *v) {
    if (v->available && v->table.release != NULL) {
        v->table.release(v->table.user);
    }
    v->available = false;
    pthread_mutex_destroy(&v->lock);
}

void vento_h264_enter(VentoH264 *v) {
    current = v;
}

void vento_h264_leave(void) {
    current = NULL;
}

bool vento_h264_status(VentoH264 *v, char *out, size_t len) {
    if (out == NULL || len < 2) {
        return false;
    }
    pthread_mutex_lock(&v->lock);
    const bool have = v->status[0] != '\0';
    if (have) {
        snprintf(out, len, "%s", v->status);
    }
    pthread_mutex_unlock(&v->lock);
    return have;
}

static bool claimHardware(VentoH264 *v) {
    int held = atomic_load(&v->hardwareHeld);
    while (held < VENTO_H264_MAX_HARDWARE) {
        if (atomic_compare_exchange_weak(&v->hardwareHeld, &held, held + 1)) {
            return true;
        }
    }
    return false;
}

static void closeDecoder(Context *c) {
    if (c->video == NULL) {
        return;
    }
    if (c->decoder != NULL) {
        c->video->table.close(c->decoder);
        c->decoder = NULL;
    }
    if (c->hardware) {
        atomic_fetch_sub(&c->video->hardwareHeld, 1);
        c->hardware = false;
    }
}

static bool openDecoder(Context *c, UINT32 width, UINT32 height) {
    const vento_video_decoders *t = &c->video->table;
    c->mode = t->mode != NULL ? t->mode(t->user) : 0;
    // Off asks nothing of the GPU, so it takes no place either; nor does a
    // session whose device has failed it.
    c->hardware = c->mode != 0 && !atomic_load(&c->video->failed) && claimHardware(c->video);
    if (c->mode != 0 && !c->hardware && !atomic_load(&c->video->failed)) {
        LOGI("H.264: %d surfaces already decode in hardware, so the one at %ux%u is software",
             VENTO_H264_MAX_HARDWARE, width, height);
    }
    c->decoder = t->open(t->user, VENTO_VIDEO_H264, (int) width, (int) height, c->hardware);
    c->width = width;
    c->height = height;
    c->restartAsked = false;
    if (c->decoder == NULL) {
        LOGW("H.264: no decoder for %ux%u", width, height);
        closeDecoder(c);
        return false;
    }
    return true;
}

static BOOL vento_h264_subsystem_init(H264_CONTEXT *h264) {
    if (h264->Compressor) {
        return FALSE;
    }
    Context *c = calloc(1, sizeof(*c));
    if (c == NULL) {
        return FALSE;
    }
    // NULL for a context made outside any session's surface command: the
    // one the library makes at connect for nothing but to see whether it
    // can (codecs.c), which is never fed, and would otherwise be a warning
    // that H.264 is missing in every connection's log.
    c->video = current != NULL && current->available ? current : NULL;
    h264->pSystemData = c;
    return TRUE;
}

static void vento_h264_subsystem_uninit(H264_CONTEXT *h264) {
    Context *c = h264->pSystemData;
    if (c == NULL) {
        return;
    }
    closeDecoder(c);
    free(c);
    h264->pSystemData = NULL;
}

/* 1 with a picture's planes in the context, below zero without one, as the
   library's other subsystems answer. */
static int vento_h264_subsystem_decompress(H264_CONTEXT *WINPR_RESTRICT h264,
                                           const BYTE *WINPR_RESTRICT data, UINT32 size) {
    Context *c = h264->pSystemData;
    if (c == NULL || c->video == NULL) {
        return -1;
    }
    VentoH264 *v = c->video;
    const vento_video_decoders *t = &v->table;

    // Opened at the first frame rather than at Init, which the library calls
    // once before it knows the surface's size and again after.
    if (c->decoder == NULL || c->width != h264->width || c->height != h264->height) {
        closeDecoder(c);
        if (!openDecoder(c, h264->width, h264->height)) {
            return -1;
        }
    } else if (!c->restartAsked && t->mode != NULL && t->mode(t->user) != c->mode) {
        // The person changed how video is decoded. A decoder in the new mode
        // would have none of the pictures the stream refers back to, so the
        // server is asked to start the pipeline again instead: its new
        // surfaces are new contexts, whose decoders are opened in the new
        // mode. This one goes on decoding until then.
        LOGI("H.264: hardware decoding changed, restarting the graphics pipeline");
        c->restartAsked = true;
        atomic_store(&v->failed, false);
        atomic_store(&v->restart, true);
    }

    vento_video_planes planes;
    const bool picture = t->decode_planes(c->decoder, data, size, &planes);
    if (t->take_key_frame_request != NULL && t->take_key_frame_request(c->decoder)) {
        // A hardware decoder failed mid-stream and was replaced by a software
        // one, which has none of the pictures the stream refers back to; and
        // a server sends a key frame only when it starts a stream.
        LOGW("H.264: the decoder lost its reference pictures, restarting the graphics pipeline");
        atomic_store(&v->failed, true);
        atomic_store(&v->restart, true);
    }
    if (!picture) {
        return -1;
    }
    for (size_t i = 0; i < 3; i++) {
        h264->pYUVData[i] = (BYTE *) planes.data[i];
        h264->iStride[i] = (UINT32) planes.stride[i];
    }
    h264->YUVWidth = (UINT32) planes.width;
    h264->YUVHeight = (UINT32) planes.height;

    atomic_store(&v->hardware, t->hardware != NULL ? t->hardware(c->decoder) : -1);
    const uint64_t now = nowMicros();
    if (t->status != NULL && now - c->lastStatus >= STATUS_EVERY_US) {
        char line[sizeof(v->status)];
        c->lastStatus = now;
        if (t->status(c->decoder, line, sizeof(line))) {
            pthread_mutex_lock(&v->lock);
            memcpy(v->status, line, sizeof(line));
            pthread_mutex_unlock(&v->lock);
        }
    }
    return 1;
}

static int vento_h264_subsystem_compress(H264_CONTEXT *WINPR_RESTRICT h264,
                                         const BYTE **WINPR_RESTRICT src,
                                         const UINT32 *WINPR_RESTRICT stride,
                                         BYTE **WINPR_RESTRICT dst, UINT32 *WINPR_RESTRICT size) {
    (void) h264;
    (void) src;
    (void) stride;
    (void) dst;
    (void) size;
    return -1;
}

/* The slot patches/freerdp adds to the library's table. */
__attribute__((visibility("hidden"))) const H264_CONTEXT_SUBSYSTEM g_Subsystem_external = {
        "vento",
        vento_h264_subsystem_init,
        vento_h264_subsystem_uninit,
        vento_h264_subsystem_decompress,
        vento_h264_subsystem_compress,
};
