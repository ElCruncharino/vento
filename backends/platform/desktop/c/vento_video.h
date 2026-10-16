// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

// A video decoder on a desktop: libavcodec behind one narrow C API, which is
// all of FFmpeg the Rust decoder (`platform_desktop::ffmpeg`) sees.
//
// Narrow on purpose. FFmpeg's own API is hundreds of functions and structs
// whose layout changes with every major release; this is six calls and one
// struct of plain integers and pointers, written by hand on both sides, so
// the Rust half needs no generated bindings and no FFmpeg headers, and a new
// FFmpeg is a change to this file's implementation alone. What crosses it is
// what a frame-at-a-time decoder needs: a packet in, and the last picture out
// described plane by plane, for the caller to convert. Conversion stays the
// caller's (`common::video::convert`), the same on the phone as here.
#ifndef VENTO_VIDEO_H
#define VENTO_VIDEO_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

// The codecs, numbered as the Rust side's `Codec` asks for them.
enum vento_video_codec {
    VENTO_VIDEO_H264 = 1,
    VENTO_VIDEO_H265 = 2,
    VENTO_VIDEO_VP8 = 3,
    VENTO_VIDEO_VP9 = 4,
    VENTO_VIDEO_AV1 = 5,
};

// What `vento_video_decode` made of a packet.
enum vento_video_status {
    // A picture came out, and `out` describes it.
    VENTO_VIDEO_PICTURE = 1,
    // The packet was taken, and no picture is ready yet.
    VENTO_VIDEO_NONE = 0,
    // The decoder refused the packet; `error` says why.
    VENTO_VIDEO_REFUSED = -1,
    // A picture came out in a format that is not YUV a caller can convert;
    // `format` names it.
    VENTO_VIDEO_UNSUPPORTED = -2,
};

// A decoded picture, as the caller reads it in place. Valid until the next
// `vento_video_decode` or `vento_video_close` on the same decoder.
//
// Three planes, Y, U and V, each `stride` bytes a row. When `interleaved`,
// U and V share plane 1, alternating, U first unless `v_first` (NV12, NV21),
// and `data[2]` is `data[1]`. Chroma is subsampled by `1 << chroma_shift_x`
// across and `1 << chroma_shift_y` down. A `depth` of 8 is a byte a sample;
// nine to sixteen is two bytes, little-endian, the value in the low bits.
struct vento_video_picture {
    const uint8_t *data[3];
    int32_t stride[3];
    int32_t width;
    int32_t height;
    int32_t chroma_shift_x;
    int32_t chroma_shift_y;
    int32_t interleaved;
    int32_t v_first;
    int32_t depth;
    // BT.709's matrix rather than BT.601's.
    int32_t bt709;
    // Full range (0-255) rather than limited (16-235).
    int32_t full_range;
    // The error of a receive that failed after the picture, or 0: a frame
    // lost behind the one returned, for the caller to log.
    int32_t error;
    // Pictures that came out of this packet before the returned one and were
    // dropped unread, each a whole picture the last overwrote.
    uint32_t skipped;
};

typedef struct vento_video vento_video;

// sizeof(struct vento_video_picture), for a binding to check its copy of the
// struct against.
size_t vento_video_picture_size(void);

// Whether this build of libavcodec has a decoder for the codec.
int vento_video_decodable(int codec);

// A decoder for the codec at about this size (a hint: the stream says what
// size it is), or NULL with the reason in `error` (NUL-terminated, cut to
// `error_len`). Low delay, slice threads as many as the machine has.
vento_video *vento_video_open(int codec, int width, int height, char *error, size_t error_len);

// The name of libavcodec's decoder behind it ("h264", "libdav1d", ...).
const char *vento_video_name(const vento_video *video);

// One encoded frame in, which is copied and not kept, and every picture the
// decoder then has ready read, of which the last is described in `out`.
// On VENTO_VIDEO_REFUSED, `out->error` holds the decoder's error; on
// VENTO_VIDEO_UNSUPPORTED, `format` (cut to `format_len`) names the format.
int vento_video_decode(vento_video *video, const uint8_t *data, size_t len,
                       struct vento_video_picture *out, char *format, size_t format_len);

// libavcodec's words for an error code.
void vento_video_strerror(int error, char *buf, size_t len);

void vento_video_close(vento_video *video);

#ifdef __cplusplus
}
#endif

#endif
