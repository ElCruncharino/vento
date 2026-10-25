// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

// A desktop video decoder: libavcodec behind one narrow C API, the only FFmpeg
// interface the Rust decoder (`platform_linux::ffmpeg`) uses.
//
// Narrow on purpose. FFmpeg's own API is hundreds of functions and structs
// whose layout changes with every major release; this is about twenty calls and
// three structs of plain integers and pointers, written by hand on both sides,
// so the Rust half needs no generated bindings and no FFmpeg headers, and a new
// FFmpeg changes only this file's implementation. It carries what a
// frame-at-a-time decoder needs: a packet in, and the last picture out
// described plane by plane. Conversion stays the caller's
// (`runtime::media::video::convert`), the same on the phone as here.
//
// Hardware decoding is off unless the caller requests it: a decoder opened with
// VENTO_VIDEO_HW_OFF is a plain software decoder, and libva is not loaded for
// it. When requested, the picture still comes out as planes in memory for the
// caller to read (the surface mapped for reading rather than copied), so the
// caller cannot tell the two apart except by asking; and the decoder falls back
// to software whenever the hardware cannot decode the stream, at the open or
// mid-stream, with the reason kept.
//
// Unless the caller asks to `hold` a VAAPI picture instead: then it is not
// mapped at all but handed out as a `vento_video_frame` that keeps its surface
// until the caller frees it, and that the caller can export as DMA-BUFs for its
// GPU to read, or map where it needs the pixels in memory. That is for a caller
// that draws on the same GPU, and saves the readback, which is most of a
// hardware picture's cost.
#ifndef VENTO_VIDEO_H
#define VENTO_VIDEO_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

// The codecs, numbered as the Rust side (`platform_linux::ffmpeg`) maps its
// `Codec` to them.
enum vento_video_codec {
    VENTO_VIDEO_H264 = 1,
    VENTO_VIDEO_H265 = 2,
    VENTO_VIDEO_VP8 = 3,
    VENTO_VIDEO_VP9 = 4,
    VENTO_VIDEO_AV1 = 5,
};

// How a decoder may decode, as the caller requests it: numbered in the order
// `runtime::media::video::Hardware` declares them.
enum vento_video_hardware {
    // libavcodec's software decoders, and nothing else: the default.
    VENTO_VIDEO_HW_OFF = 0,
    // VAAPI, through libva (loaded then, if it is installed) on the first
    // DRM render node: `Hardware::On`, the one hardware API here.
    VENTO_VIDEO_HW_VAAPI = 1,
    // VAAPI where it has a decoder for the stream, else software: the same
    // as VAAPI here, the only device; the setting means "use the GPU where
    // possible" rather than a choice of API.
    VENTO_VIDEO_HW_AUTO = 2,
};

// Samples packed into words rather than laid out in planes, for a picture's
// `packed`.
enum vento_video_packing {
    VENTO_VIDEO_PLANES = 0,
    // XV30: 4:4:4, one little-endian 32-bit word per pixel in plane 0, U in
    // its low ten bits, Y in the next ten, V in the ten above, the top two
    // unused. VAAPI decodes 10-bit HEVC 4:4:4 into it.
    VENTO_VIDEO_XV30 = 1,
};

// The result of `vento_video_decode` for a packet.
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
// U and V share plane 1, alternating, U first unless `v_first` (NV12, NV21,
// and at more than eight bits P010), and `data[2]` is `data[1]`. Chroma is
// subsampled by `1 << chroma_shift_x` across and `1 << chroma_shift_y` down.
// A `depth` of 8 is a byte per sample; nine to sixteen is two bytes,
// little-endian, the value in the low bits unless `shift` says how far up
// it is (P010 has its ten bits at the top, `shift` 6, and zeros below).
// If `packed` is not VENTO_VIDEO_PLANES, plane 0 alone holds every sample in
// that packing, and `depth` is each sample's.
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
    // How many bits up a sample's value is in its two bytes.
    int32_t shift;
    // A `vento_video_packing`.
    int32_t packed;
    // Nonzero if the picture came out of the hardware: 1 mapped for reading,
    // 2 copied (the driver does not map its surfaces), 3 held.
    int32_t hardware;
    // The decoder that received this packet failed on it, and the software
    // one replaces it from here on: the hardware failed, or `av1` had no
    // device decoder for the stream. The packet was given
    // to the new decoder, but anything it refers to was decoded by the old
    // one, so until a key frame there may be no picture or a broken one.
    // The caller should ask the far end for one, unless no picture had come
    // out before. Set on that one call, whatever its status. -1 where the
    // hardware failed and no software decoder would open in its place: the
    // status is VENTO_VIDEO_REFUSED, and so is every packet's from then on.
    int32_t fell_back;
    // The call's time, from the packet to the picture described (decode,
    // and on hardware the wait for it and the mapping), in microseconds.
    uint32_t decode_us;
    // Of that, mapping the hardware's picture for reading, in microseconds.
    uint32_t map_us;
    // A picture held rather than described, where the decode asked to
    // `hold` and the picture is one that can be (VAAPI's, in NV12 or P010):
    // the caller's, to be freed with `vento_video_frame_free`, and the
    // planes above are not set. NULL otherwise.
    struct vento_video_frame *held;
    // Which surface a held picture is in: a number for the decoder's set of
    // surfaces, never used twice in a process and changed whenever the set
    // is (a new decoder, a new size), and the surface's own in it.
    uint64_t pool;
    uint32_t surface;
};

// One of a held picture's planes as a DMA-BUF: a descriptor of the caller's
// own, which it closes, the DRM format and layout modifier, where the plane
// starts and how far apart its rows are, and its size in samples (the
// surface's, which is larger than the picture where the codec rounds it up).
struct vento_video_layer {
    int32_t fd;
    uint32_t fourcc;
    uint64_t modifier;
    uint32_t offset;
    uint32_t pitch;
    uint32_t width;
    uint32_t height;
};

// A held picture as DMA-BUFs: Y, then U and V interleaved, a layer each.
struct vento_video_export {
    int32_t layers;
    struct vento_video_layer layer[2];
    int32_t bt709;
    int32_t full_range;
};

typedef struct vento_video vento_video;
typedef struct vento_video_frame vento_video_frame;

// sizeof(struct vento_video_picture), and of the export, for a binding to
// check its copies of the structs against.
size_t vento_video_picture_size(void);
size_t vento_video_export_size(void);

// Whether this build of libavcodec has a decoder for the codec.
int vento_video_decodable(int codec);

// A decoder for the codec at about this size (a hint: the stream says what
// size it is), or NULL with the reason in `error` (NUL-terminated, cut to
// `error_len`). Low delay, slice threads as many as the machine has.
//
// `hardware` is a `vento_video_hardware`. Anything but OFF opens the VAAPI
// device (once for the whole process: every decoder that asks shares it,
// whichever thread it is on, and it stays open until the process ends) and
// decodes on it when the stream's profile is one it has. Where there is no
// device, or the device has no decoder for the profile, the decoder is
// software, and `vento_video_software_reason` says why; the open only fails
// where software would.
vento_video *vento_video_open(int codec, int width, int height, int hardware, char *error,
                              size_t error_len);

// The name of libavcodec's decoder behind it ("h264", "libdav1d", ...).
const char *vento_video_name(const vento_video *video);

// The decoder's device, a `vento_video_hardware` (VAAPI), or OFF for software:
// from the start without a device, and after a mid-stream fallback. A decoder
// on a device that cannot take its stream stays on it: its pictures are not
// marked `hardware`, and the reason is kept.
int vento_video_method(const vento_video *video);

// A `vento_video_hardware`'s name: "vaapi", "auto" or "off".
const char *vento_video_method_name(int method);

// Why the decoder is not decoding in hardware though it was asked to, such as
// "vaapi: no device (...)", "vaapi: H.264 High 10 is not supported" or
// "vaapi: decode failed (...)", or "" when hardware was not requested or
// nothing prevents it. Valid until the next call on the decoder.
const char *vento_video_software_reason(const vento_video *video);

// Whether the shared device of a `vento_video_hardware` (VAAPI, which AUTO
// tries too, so both say the same) has been asked for in this process: 0
// never, 1 open, -1 tried and failed; always 0 for OFF, which has none. For
// tests, and to check that OFF opened nothing.
int vento_video_device_state(int method);

// Whether the decoder maps VAAPI's pictures in place (1) rather than as an
// image the driver copies each surface into (0), which it decides once it
// has measured both ways, on alternate pictures after its first two;
// -1 until then.
int vento_video_maps_in_place(const vento_video *video);

// For tests: the next hardware picture of this decoder fails as if the device
// had gone away, which a test cannot otherwise cause; with `software_too`, so
// does opening the software decoder that would replace it, which leaves the
// decoder refusing every packet.
void vento_video_fail_next(vento_video *video, int software_too);

// Decodes one encoded frame (copied, not kept) and reads every picture the
// decoder then has ready, describing the last in `out`.
// On VENTO_VIDEO_REFUSED, `out->error` holds the decoder's error; on
// VENTO_VIDEO_UNSUPPORTED, `format` (cut to `format_len`) names the format.
//
// With `hold`, a picture on VAAPI in NV12 or P010 is waited for (the GPU has
// finished decoding it when this returns) and handed out in `out->held`
// rather than mapped and described; any other picture is described as usual.
int vento_video_decode(vento_video *video, const uint8_t *data, size_t len, int hold,
                       struct vento_video_picture *out, char *format, size_t format_len);

// A held picture's surface as DMA-BUFs, every descriptor a new one of the
// caller's; or a negative error, with the reason in `error` (cut to
// `error_len`).
// From any thread, as often as wanted.
int vento_video_frame_export(vento_video_frame *frame, struct vento_video_export *out,
                             char *error, size_t error_len);

// A held picture mapped for reading and described in `out` as a decode
// describes one, valid until `vento_video_frame_unmap`: for a caller that
// needs the pixels in memory after all. 1, or 0 where it cannot be mapped.
// One mapping at a time per picture; from any one thread at a time.
int vento_video_frame_map(vento_video_frame *frame, struct vento_video_picture *out);
void vento_video_frame_unmap(vento_video_frame *frame);

// Releases a held picture, from any thread: the decoder can decode into its
// surface again once nothing else holds it. The decoder may be closed
// already.
void vento_video_frame_free(vento_video_frame *frame);

// libavcodec's message for an error code.
void vento_video_strerror(int error, char *buf, size_t len);

void vento_video_close(vento_video *video);

#ifdef __cplusplus
}
#endif

#endif
