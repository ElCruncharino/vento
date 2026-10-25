// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

// The platform's video decoders for a C backend: a table of calls over
// `runtime::media::video::Decoders` (MediaCodec on the phone, FFmpeg with
// opt-in VAAPI on the Linux client), filled in by `api::c::video` and passed
// in a backend's config. Function pointers rather than symbols, so that a
// backend library links nothing from the process it is loaded into (the
// Linux client's `libvento_tigervnc.so`, for example, has no Rust in it).
//
// TigerVNC's H.264 rectangles and FreeRDP's H.264 surface commands are
// decoded through this rather than through either library's own decoder, so
// that hardware decoding, its fallback and the facts reported about it share
// RustDesk's code on both platforms.

#ifndef VENTO_VIDEO_H
#define VENTO_VIDEO_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* The codecs, as `runtime::media::video::Codec` names them. */
#define VENTO_VIDEO_H264 1
#define VENTO_VIDEO_H265 2
#define VENTO_VIDEO_VP8 3
#define VENTO_VIDEO_VP9 4
#define VENTO_VIDEO_AV1 5

/* One decoder, for one codec at one size. Not safe to call from two threads
   at once, but may be called from different threads in turn. */
typedef struct vento_video_decoder vento_video_decoder;

/* A picture as separate 8-bit 4:2:0 planes, Y then U then V: `width` by
   `height` samples of Y and half that, rounded up, of U and V, with rows
   `stride` bytes apart. Every plane has 32 extra bytes on each row and 32
   extra rows below the picture, for SIMD readers. Owned by the decoder and
   valid until its next call. */
typedef struct vento_video_planes {
    const uint8_t *data[3];
    int32_t stride[3];
    int width, height;
} vento_video_planes;

typedef struct vento_video_decoders {
    /* Passed to `open`, `mode` and `release`. */
    void *user;
    /* A decoder, or NULL where the platform has none for this codec at this
       size. `hardware` false asks for software regardless of the user's
       setting, so a backend that keeps many decoders can keep most of them
       off the GPU; true follows the setting (`mode`). */
    vento_video_decoder *(*open)(void *user, int codec, int width, int height, bool hardware);
    /* How a decoder opened now would decode, as a number that changes with
       the user's setting: `runtime::media::video::Hardware` in declaration
       order, 0 for off. Cheap enough to call every frame. */
    int (*mode)(void *user);
    /* One frame. `out` is `width * height` words, rows `width` apart, R, G, B,
       A in memory; only the changed rows are written, unless `whole`, which
       converts every row of the picture (for a buffer whose contents are
       unknown). True when a picture came out, with the changed area as one
       rectangle in `changed`: x, y, width, height, with a width of zero when
       nothing changed. */
    bool (*decode)(vento_video_decoder *d, const uint8_t *data, size_t len, uint32_t *out,
                   int width, int height, bool whole, int32_t changed[4]);
    /* One frame into `out` as planes rather than converted, for a backend
       that converts them itself (FreeRDP, whose AVC444 combines two pictures
       into one). Every picture is returned, in decoding order, without
       skipping any for a newer one. True when a picture came out; false
       otherwise, including for a picture that is not 4:2:0. */
    bool (*decode_planes)(vento_video_decoder *d, const uint8_t *data, size_t len,
                          vento_video_planes *out);
    /* Whether the last picture was decoded in hardware: 1, 0, or -1 where the
       decoder does not know or has had no picture yet. */
    int (*hardware)(vento_video_decoder *d);
    /* The decoder's status line for an overlay ("H.264 vaapi map 2.1 ms"),
       into `out` with its NUL, truncated to fit. False, with nothing
       written, when there is no status. */
    bool (*status)(vento_video_decoder *d, char *out, size_t len);
    /* True, once, after the decoder lost the pictures the stream refers back
       to (a hardware decoder that failed mid-stream and was replaced), so
       that the far end is asked for a fresh picture. Polled after `decode`. */
    bool (*take_key_frame_request)(vento_video_decoder *d);
    void (*close)(vento_video_decoder *d);
    /* The backend owns the table from the moment it is handed over. Called
       once, after the last decoder is closed, and also when the backend
       fails to start. */
    void (*release)(void *user);
} vento_video_decoders;

#ifdef __cplusplus
}
#endif

#endif
