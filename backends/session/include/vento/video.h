// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

// The platform's video decoders, as a C backend is handed them: a table of
// calls over `media::video::Decoders` (MediaCodec on the phone, the Linux
// client's FFmpeg with its opt-in VAAPI), filled in by `session::c::video` and
// passed in a backend's config. Function pointers rather than symbols, so that
// a backend library links nothing of the process it is loaded into — the Linux
// client's `libvento_tigervnc.so` is one, and has no Rust in it.
//
// TigerVNC's H.264 rectangles and FreeRDP's H.264 surface commands are
// decoded through this rather than through a decoder of either library's
// own, so that the hardware decoding, its fallback and the facts about it are
// the same code as RustDesk's on both platforms.

#ifndef VENTO_VIDEO_H
#define VENTO_VIDEO_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* The codecs, as `media::video::Codec` names them. */
#define VENTO_VIDEO_H264 1
#define VENTO_VIDEO_H265 2
#define VENTO_VIDEO_VP8 3
#define VENTO_VIDEO_VP9 4
#define VENTO_VIDEO_AV1 5

/* One decoder, for one codec at one size. Not safe to call into from two
   threads at once; from one thread after another is what it is for. */
typedef struct vento_video_decoder vento_video_decoder;

/* A picture as 8-bit 4:2:0 planes, Y then U then V, each its own: `width`
   by `height` samples of Y and half that, rounded up, of each of the other
   two, rows `stride` bytes apart. Every plane has 32 bytes more on each row
   and 32 rows more below it than the picture covers, for whatever reads the
   planes a vector at a time. The decoder's until its next call. */
typedef struct vento_video_planes {
    const uint8_t *data[3];
    int32_t stride[3];
    int width, height;
} vento_video_planes;

typedef struct vento_video_decoders {
    /* The decoders themselves, handed to `open`, `mode` and `release`. */
    void *user;
    /* A decoder, or NULL where the platform has none for this codec at this
       size. `hardware` false asks for software whatever the person chose,
       which is how a backend that keeps many decoders keeps most of them off
       the GPU; true decodes as the person chose (`mode`). */
    vento_video_decoder *(*open)(void *user, int codec, int width, int height, bool hardware);
    /* How a decoder opened now would decode, as a number that changes when
       the person's choice does: `media::video::Hardware` in declaration
       order, 0 for off. Cheap enough to ask every frame. */
    int (*mode)(void *user);
    /* One frame. `out` is `width * height` words, rows `width` apart, R, G, B,
       A in memory; only the rows that changed are written, unless `whole`,
       which converts every row of the picture (a buffer that has lost track
       of what is in it). True when a picture came out, with what it changed
       as one rectangle in `changed` — x, y, width, height, a width of zero
       where nothing did. */
    bool (*decode)(vento_video_decoder *d, const uint8_t *data, size_t len, uint32_t *out,
                   int width, int height, bool whole, int32_t changed[4]);
    /* One frame into `out` as planes rather than converted: for a backend that
       converts them itself (FreeRDP, whose AVC444 makes one picture of two).
       Every picture comes out, one for each frame that makes one and in the
       order they went in, none passed over for a newer one. True when one
       did; false where it did not, which includes a picture that is not
       4:2:0. */
    bool (*decode_planes)(vento_video_decoder *d, const uint8_t *data, size_t len,
                          vento_video_planes *out);
    /* Whether the last picture was decoded in hardware: 1, 0, or -1 where the
       decoder does not know or has had no picture yet. */
    int (*hardware)(vento_video_decoder *d);
    /* The decoder's line for an overlay ("H.264 vaapi map 2.1 ms"), into
       `out` with its NUL, cut short to fit. False, with nothing written,
       where it has nothing to say. */
    bool (*status)(vento_video_decoder *d, char *out, size_t len);
    /* True, once, after the decoder lost the pictures the stream refers back
       to (a hardware decoder that failed mid-stream and was replaced), so
       that the far end is asked for a fresh picture. Polled after `decode`. */
    bool (*take_key_frame_request)(vento_video_decoder *d);
    void (*close)(vento_video_decoder *d);
    /* The table is the backend's from the moment it is handed over: this is
       called once, after the last decoder is closed, and also when the
       backend fails to start at all. */
    void (*release)(void *user);
} vento_video_decoders;

#ifdef __cplusplus
}
#endif

#endif
