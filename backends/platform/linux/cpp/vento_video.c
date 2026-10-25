// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

// vento_video.h over libavcodec. See the header for the interface; this file
// is the only one that includes FFmpeg's headers.
//
// The libavcodec settings chosen here:
//
// - **Low delay, and slice threads only.** A decoder with frame threads holds
//   one frame per thread before the first comes out, and H.264 without
//   `LOW_DELAY` holds frames back to reorder them: input lag on a desktop, for
//   a stream that has nothing to reorder. dav1d reads the same flag as a frame
//   delay of one.
// - **AV1 by name.** libavcodec's own `av1` decoder is a front for hardware
//   acceleration and decodes nothing without it, so AV1 in software is
//   dav1d's wrapper, which the build links in, rather than a decoder that
//   opens and then fails every frame; `av1` is what decodes on hardware.
// - **Only the last picture.** Each picture out of one packet is a whole
//   picture, and converting one that the next overwrites before it is shown
//   is wasted work, so the others are dropped.
//
// And for hardware decoding, when requested:
//
// - **One device, for the process.** Opening a VAAPI device is tens of
//   milliseconds (the driver loads and initialises), and a session makes a
//   decoder on every resize and codec switch, on whichever protocol thread it
//   runs on. So the first decoder to request it opens it, under a lock, and
//   every later one takes a reference to the same; it is kept until the
//   process ends. A device that failed to open is not tried again: a
//   machine without one does not grow one mid-session, and a retry per
//   decoder would be the same delay and the same log line every time.
// - **The stream's profile decides, through `get_format`.** libavcodec offers
//   the hardware's format when it has a hardware decoder for the codec, and
//   when the driver then has no decoder for the profile (iHD has none for
//   H.264 High 10) it asks again without it. What comes back then is the
//   software format, the reason is kept, and the stream decodes in software
//   from the same context. AV1 is the exception: `av1` decodes only on
//   hardware, so without it the decoder is reopened as dav1d.
// - **Mapped, not copied.** The decoded surface is mapped for reading
//   (`av_hwframe_map`) and described in place, which measured about 40% less
//   time and half the CPU of `av_hwframe_transfer_data`'s copy into a frame
//   of our own; the caller's conversion reads the planes once either way.
//   Where the driver will not map a surface at all, it is copied.
// - **VAAPI's surface mapped in place, where that measures faster.** A map
//   for reading is, in libavcodec, a new image the driver copies the
//   surface into (`vaGetImage`), since the surface's own memory may be
//   uncached (Intel's Gen7-9, where reading it is many times slower). A
//   direct map (`vaDeriveImage`) skips that copy, which on Lunar Lake's iHD
//   was 4.5 ms of a 1080p H.264 frame's 7.4 against 2 ms, plus 1.5 ms to
//   unmap and reads half as fast: about 8% off a frame's decode and
//   conversion and 10% of its CPU. Which way is faster depends on the machine
//   (and the picture size: at 128x128 the image was), so each decoder
//   measures both, on alternate pictures (the map, a read of every plane,
//   the unmap), and maps the faster way from then on.
// - **Held, where the caller draws on the GPU.** Asked to `hold`, a VAAPI
//   picture is neither mapped nor copied: the decoder waits for the surface
//   (`vaSyncSurface`, which a map would have waited for too) and hands out a
//   reference to the frame, which keeps the surface out of the decoder's
//   reach until the caller lets go. The decoder's pool of surfaces grows
//   rather than waits (libavcodec's VAAPI pools have no fixed size since
//   VA-API 1.0), so a held picture costs a surface of memory and never a
//   picture. Exported, it is `vaExportSurfaceHandle`'s DMA-BUFs, a layer a
//   plane (`R8` and `GR88` for NV12), which FFmpeg's own DRM mapping would
//   need libdrm for; mapped after all, it is the same `av_hwframe_map` a
//   decode makes. Only NV12 and P010: XV30 has no two-layer form the caller
//   could convert the same way.
// - **A failure is a fallback, not an error.** A hardware decode or map that
//   fails mid-stream closes the decoder, opens the software one in its place
//   within the same handle, gives it the packet again and says so
//   (`fell_back`), for the caller to ask for a key frame. The session goes on
//   in software rather than losing its picture.

#include "vento_video.h"

#include <errno.h>
#include <fcntl.h>
#include <pthread.h>
#include <stdarg.h>
#include <stdatomic.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <unistd.h>

#include <libavcodec/avcodec.h>
#include <libavutil/error.h>
#include <libavutil/hwcontext.h>
#include <libavutil/pixdesc.h>

// libva's declarations as `va_shim.c` defines them, hidden: the calls made
// here (to sync and export a held picture's surface, and `vaErrorStr`) are the
// shim's.
#pragma GCC visibility push(hidden)
#include <libavutil/hwcontext_vaapi.h>
#include <va/va_drmcommon.h>
#pragma GCC visibility pop

#include "va_shim.h"

// The internal status of a packet the hardware failed to decode, never
// returned to the caller: the decoder falls back and decodes the packet again.
#define HARDWARE_FAILED (-100)

// How a decoder maps VAAPI surfaces for reading: not known yet, as an image
// the driver copies into, or in place.
enum { MAP_UNKNOWN, MAP_IMAGE, MAP_DIRECT };
// The number of trial pictures of each way, alternating, compared by their
// medians, since one picture's wait for the GPU can be twice another's; and
// the number of pictures before the first trial, while the device sets up its
// surfaces.
#define MAP_TRIALS 8
#define MAP_TRIALS_FROM 2

struct vento_video {
    int codec;
    int width;
    int height;
    AVCodecContext *context;
    AVPacket *packet;
    // The picture last described, and the next one, received while it waits:
    // two, so that only the last of several is read.
    AVFrame *frame;
    AVFrame *newer;
    // A hardware picture's planes, mapped (or copied) for reading.
    AVFrame *mapped;
    int64_t pts;
    // The device decoded on (a `vento_video_hardware`), or OFF, and the
    // format its surfaces come out in.
    int method;
    enum AVPixelFormat hw_format;
    // VAAPI was asked for and not yet tried: a decoder falls back to
    // software once and is not put on the device again.
    int untried;
    // Copied rather than mapped, once a map has been refused as impossible.
    int copy;
    // Pictures out of the device; how VAAPI's are mapped for reading (a
    // MAP_ verdict); and until that is known the trials of each way so far,
    // what each cost (image's, direct's), and which way the mapping held now
    // was a trial of (or -1), since its unmap is part of that trial's cost.
    uint64_t hw_pictures;
    int map_verdict;
    int trials;
    uint64_t trial_us[2][MAP_TRIALS];
    int held_trial;
    // For tests (`vento_video_fail_next`): the next hardware picture fails,
    // and with `fail_software` so does the software decoder that would take
    // its place.
    int fail_next;
    int fail_software;
    // The decoder failed on the device mid-stream and no software decoder
    // opened in its place: the libavcodec error that said so, with which every
    // later packet is refused without being decoded. 0 while it decodes.
    int dead;
    // Why the decoder is not on hardware, if anything stands in its way.
    char reason[256];
    // The surfaces the last held picture came from, referenced so that their
    // address is not another set's while the number is in use, and their
    // number (see `pools`).
    AVBufferRef *pool_ref;
    uint64_t pool;
};

// A held picture: the decoder's frame, referenced, and a mapping of it.
struct vento_video_frame {
    AVFrame *frame;
    AVFrame *mapped;
};

// The last number given to a set of surfaces, in the whole process: a
// holder's cache of what it imported is keyed by it, and two decoders' sets
// can have surfaces of the same id.
static atomic_uint_fast64_t pools;

size_t vento_video_picture_size(void) {
    return sizeof(struct vento_video_picture);
}

size_t vento_video_export_size(void) {
    return sizeof(struct vento_video_export);
}

// The software decoder for the codec, or (`hardware`) the one libavcodec
// uses on a device, which differs only for AV1.
static const AVCodec *find(int codec, int hardware) {
    switch (codec) {
    case VENTO_VIDEO_H264:
        return avcodec_find_decoder(AV_CODEC_ID_H264);
    case VENTO_VIDEO_H265:
        return avcodec_find_decoder(AV_CODEC_ID_HEVC);
    case VENTO_VIDEO_VP8:
        return avcodec_find_decoder(AV_CODEC_ID_VP8);
    case VENTO_VIDEO_VP9:
        return avcodec_find_decoder(AV_CODEC_ID_VP9);
    case VENTO_VIDEO_AV1:
        return avcodec_find_decoder_by_name(hardware ? "av1" : "libdav1d");
    default:
        return NULL;
    }
}

// The codec's display name, for a reason string.
static const char *codec_name(int codec) {
    switch (codec) {
    case VENTO_VIDEO_H264:
        return "H.264";
    case VENTO_VIDEO_H265:
        return "H.265";
    case VENTO_VIDEO_VP8:
        return "VP8";
    case VENTO_VIDEO_VP9:
        return "VP9";
    case VENTO_VIDEO_AV1:
        return "AV1";
    default:
        return "this codec";
    }
}

int vento_video_decodable(int codec) {
    return find(codec, 0) != NULL;
}

void vento_video_strerror(int error, char *buf, size_t len) {
    if (len == 0)
        return;
    if (av_strerror(error, buf, len) < 0)
        snprintf(buf, len, "error %d", error);
}

const char *vento_video_method_name(int method) {
    switch (method) {
    case VENTO_VIDEO_HW_OFF:
        return "off";
    case VENTO_VIDEO_HW_VAAPI:
        return "vaapi";
    case VENTO_VIDEO_HW_AUTO:
        return "auto";
    default:
        return "unknown";
    }
}

static uint64_t now_us(void) {
    struct timespec t;
    clock_gettime(CLOCK_MONOTONIC, &t);
    return (uint64_t)t.tv_sec * 1000000 + (uint64_t)t.tv_nsec / 1000;
}


// The shared VAAPI device.
static struct {
    AVBufferRef *ref;
    // 0 not tried, 1 open, -1 failed (`why` holds the reason).
    int state;
    char why[128];
} vaapi;
static pthread_mutex_t device_lock = PTHREAD_MUTEX_INITIALIZER;

int vento_video_device_state(int method) {
    // AUTO's device is VAAPI's: the one there is to try.
    if (method != VENTO_VIDEO_HW_VAAPI && method != VENTO_VIDEO_HW_AUTO)
        return 0;
    pthread_mutex_lock(&device_lock);
    int state = vaapi.state;
    pthread_mutex_unlock(&device_lock);
    return state;
}

// A reference to the VAAPI device, opening it on first use, or NULL with
// "vaapi: <reason>" in `why`.
static AVBufferRef *device(char *why, size_t why_len) {
    pthread_mutex_lock(&device_lock);
    if (vaapi.state == 0) {
        // The first DRM render node, which is FFmpeg's default.
        int made = av_hwdevice_ctx_create(&vaapi.ref, AV_HWDEVICE_TYPE_VAAPI, NULL, NULL, 0);
        if (made >= 0) {
            vaapi.state = 1;
        } else {
            vaapi.state = -1;
            const char *missing = NULL;
            if (!vento_va_loaded(&missing)) {
                snprintf(vaapi.why, sizeof(vaapi.why), "no libva (%s)", missing);
            } else if (made == AVERROR_EXTERNAL) {
                // libavcodec returns only this when there is no render node
                // or the driver does not initialise; its log says which.
                snprintf(vaapi.why, sizeof(vaapi.why), "no device");
            } else {
                char error[64];
                vento_video_strerror(made, error, sizeof(error));
                snprintf(vaapi.why, sizeof(vaapi.why), "no device (%s)", error);
            }
        }
    }
    AVBufferRef *ref = vaapi.ref ? av_buffer_ref(vaapi.ref) : NULL;
    if (!ref)
        snprintf(why, why_len, "vaapi: %s", vaapi.ref ? "out of memory" : vaapi.why);
    pthread_mutex_unlock(&device_lock);
    return ref;
}

// Sets the reason the decoder is not decoding the stream on the device.
static void say(vento_video *video, const char *fmt, ...) {
    va_list ap;
    va_start(ap, fmt);
    vsnprintf(video->reason, sizeof(video->reason), fmt, ap);
    va_end(ap);
}

// libavcodec's format callback: the hardware's format where it is offered, and
// the software's where it is not, which is also where a hardware decoder that
// failed to start for this stream's profile ends up: libavcodec asks again
// without it. Called again on every change of the stream's parameters.
static enum AVPixelFormat get_format(AVCodecContext *context, const enum AVPixelFormat *formats) {
    vento_video *video = context->opaque;
    for (const enum AVPixelFormat *f = formats; *f != AV_PIX_FMT_NONE; f++) {
        // Chosen, though not yet decoded on: libavcodec starts the
        // hardware's decoder after this, and asks again without it where the
        // driver has none for the profile. So the reason stands until a
        // picture comes out of the device.
        if (*f == video->hw_format)
            return *f;
    }
    const char *profile = avcodec_profile_name(context->codec_id, context->profile);
    const char *sw = av_get_pix_fmt_name(context->sw_pix_fmt);
    if (profile)
        say(video, "%s: %s %s is not supported", vento_video_method_name(video->method),
            codec_name(video->codec), profile);
    else
        say(video, "%s: %s in %s is not supported", vento_video_method_name(video->method),
            codec_name(video->codec), sw ? sw : "this format");
    for (const enum AVPixelFormat *f = formats; *f != AV_PIX_FMT_NONE; f++) {
        const AVPixFmtDescriptor *desc = av_pix_fmt_desc_get(*f);
        if (desc && !(desc->flags & AV_PIX_FMT_FLAG_HWACCEL))
            return *f;
    }
    return AV_PIX_FMT_NONE;
}

// A context for the codec, on `device` when there is one (which it takes) and
// in software when not, into `video->context`; or a libavcodec error.
static int open_context(vento_video *video, int method, AVBufferRef *device) {
    const AVCodec *found = find(video->codec, device != NULL);
    if (!found) {
        av_buffer_unref(&device);
        return AVERROR_DECODER_NOT_FOUND;
    }
    AVCodecContext *context = avcodec_alloc_context3(found);
    if (!context) {
        av_buffer_unref(&device);
        return AVERROR(ENOMEM);
    }
    // A hint rather than a promise: the stream says what size it is, and
    // what is read is the frame's own.
    context->width = video->width;
    context->height = video->height;
    context->flags |= AV_CODEC_FLAG_LOW_DELAY;
    context->thread_type = FF_THREAD_SLICE;
    context->thread_count = 0; // as many as the machine has
    if (device) {
        context->hw_device_ctx = device;
        context->get_format = get_format;
        context->opaque = video;
    }
    int opened = avcodec_open2(context, found, NULL);
    if (opened < 0) {
        avcodec_free_context(&context);
        return opened;
    }
    avcodec_free_context(&video->context);
    video->context = context;
    video->method = device ? method : VENTO_VIDEO_HW_OFF;
    video->hw_format = device ? AV_PIX_FMT_VAAPI : AV_PIX_FMT_NONE;
    video->copy = 0;
    return 0;
}

// A decoder on the VAAPI device where it was requested and has not been
// tried, if the device opens and so does a decoder on it; else the software
// decoder, with the device's failure as the decoder's reason. A libavcodec
// error where not even software opens.
static int open_decoder(vento_video *video) {
    if (video->untried) {
        video->untried = 0;
        char why[sizeof(video->reason)];
        AVBufferRef *ref = device(why, sizeof(why));
        int opened = ref ? open_context(video, VENTO_VIDEO_HW_VAAPI, ref) : -1;
        if (opened >= 0)
            return 0;
        if (ref) {
            char e[64];
            vento_video_strerror(opened, e, sizeof(e));
            snprintf(why, sizeof(why), "vaapi: the decoder did not open (%s)", e);
        }
        say(video, "%s", why);
    }
    if (video->fail_software) {
        video->fail_software = 0;
        return AVERROR(ENOMEM);
    }
    return open_context(video, VENTO_VIDEO_HW_OFF, NULL);
}

void vento_video_close(vento_video *video) {
    if (!video)
        return;
    av_frame_free(&video->mapped);
    av_frame_free(&video->newer);
    av_frame_free(&video->frame);
    av_packet_free(&video->packet);
    avcodec_free_context(&video->context);
    av_buffer_unref(&video->pool_ref);
    free(video);
}

vento_video *vento_video_open(int codec, int width, int height, int hardware, char *error,
                              size_t error_len) {
    if (error_len > 0)
        error[0] = '\0';
    if (!find(codec, 0)) {
        snprintf(error, error_len, "no decoder for it in this libavcodec");
        return NULL;
    }
    vento_video *video = calloc(1, sizeof(*video));
    if (!video) {
        snprintf(error, error_len, "out of memory");
        return NULL;
    }
    video->codec = codec;
    video->width = width;
    video->height = height;
    video->hw_format = AV_PIX_FMT_NONE;
    video->held_trial = -1;
    video->packet = av_packet_alloc();
    video->frame = av_frame_alloc();
    video->newer = av_frame_alloc();
    video->mapped = av_frame_alloc();
    if (!video->packet || !video->frame || !video->newer || !video->mapped) {
        snprintf(error, error_len, "out of memory");
        vento_video_close(video);
        return NULL;
    }

    // The VAAPI device where it opens, and a decoder on it that opens;
    // anything else is the software decoder, with the reason kept. `auto` is
    // the same, VAAPI and then software: the only device to try.
    video->untried = hardware == VENTO_VIDEO_HW_VAAPI || hardware == VENTO_VIDEO_HW_AUTO;
    int opened = open_decoder(video);
    if (opened < 0) {
        vento_video_strerror(opened, error, error_len);
        vento_video_close(video);
        return NULL;
    }
    return video;
}

const char *vento_video_name(const vento_video *video) {
    // A dead decoder has no context: it is named for the software decoder
    // that would not open.
    return video->context ? video->context->codec->name : find(video->codec, 0)->name;
}

int vento_video_method(const vento_video *video) {
    return video->method;
}

const char *vento_video_software_reason(const vento_video *video) {
    return video->reason;
}

int vento_video_maps_in_place(const vento_video *video) {
    return video->map_verdict == MAP_UNKNOWN ? -1 : video->map_verdict == MAP_DIRECT;
}

void vento_video_fail_next(vento_video *video, int software_too) {
    video->fail_next = 1;
    video->fail_software = software_too != 0;
}

// The frame's matrix and range, for `describe` and an export.
static void colour(const AVFrame *f, int32_t *bt709, int32_t *full_range) {
    // BT.601 for BT.601 and for an untagged stream (the encoders seen so far
    // produce BT.601 untagged, and libavcodec's own scaler assumes it for
    // untagged input); BT.709 for any other tagged colour space.
    *bt709 = !(f->colorspace == AVCOL_SPC_BT470BG || f->colorspace == AVCOL_SPC_SMPTE170M ||
               f->colorspace == AVCOL_SPC_UNSPECIFIED);
    // The YUVJ formats are full range whatever the frame's range says.
    *full_range = f->color_range == AVCOL_RANGE_JPEG || f->format == AV_PIX_FMT_YUVJ420P ||
                  f->format == AV_PIX_FMT_YUVJ422P || f->format == AV_PIX_FMT_YUVJ444P;
}

// The received frame described into `out`, or false for a format that is not
// YUV of eight to sixteen bits a sample with Y in a plane of its own and U and
// V in planes of their own or interleaved in one (or packed as XV30): RGB, a
// palette, big-endian samples (which no decoder on a little-endian machine
// hands back), hardware surfaces, packed formats other than XV30, or chroma
// subsampled other than 4:2:0, 4:2:2, 4:4:0 or 4:4:4. An alpha plane after
// the three is not read.
static int describe(const AVFrame *f, struct vento_video_picture *out) {
    const AVPixFmtDescriptor *desc = av_pix_fmt_desc_get((enum AVPixelFormat)f->format);
    if (!desc || f->width <= 0 || f->height <= 0)
        return 0;

    colour(f, &out->bt709, &out->full_range);
    out->width = f->width;
    out->height = f->height;

    // The only packed format read, which VAAPI decodes 10-bit HEVC 4:4:4
    // into, and which no descriptor of planes can describe.
    if (f->format == AV_PIX_FMT_XV30LE) {
        if (!f->data[0] || f->linesize[0] < f->width * 4)
            return 0;
        out->data[0] = out->data[1] = out->data[2] = f->data[0];
        out->stride[0] = out->stride[1] = out->stride[2] = f->linesize[0];
        out->packed = VENTO_VIDEO_XV30;
        out->depth = 10;
        return 1;
    }

    const uint64_t unwanted = AV_PIX_FMT_FLAG_BE | AV_PIX_FMT_FLAG_PAL | AV_PIX_FMT_FLAG_BITSTREAM |
                              AV_PIX_FMT_FLAG_HWACCEL | AV_PIX_FMT_FLAG_RGB | AV_PIX_FMT_FLAG_BAYER |
                              AV_PIX_FMT_FLAG_FLOAT | AV_PIX_FMT_FLAG_XYZ;
    if ((desc->flags & unwanted) || !(desc->flags & AV_PIX_FMT_FLAG_PLANAR) ||
        desc->nb_components < 3 || desc->log2_chroma_w > 1 || desc->log2_chroma_h > 1)
        return 0;

    const AVComponentDescriptor *c = desc->comp;
    int depth = c[0].depth;
    int shift = c[0].shift;
    int bytes = depth > 8 ? 2 : 1;
    for (int i = 0; i < 3; i++) {
        if (c[i].depth != depth || c[i].shift != shift)
            return 0;
    }
    // A sample shifted up (P010's ten bits at the top of sixteen) still has
    // to fit in its two bytes, and a byte's sample is not shifted at all.
    if (depth < 8 || depth + shift > 8 * bytes || c[0].plane != 0 || c[0].step != bytes ||
        c[0].offset != 0)
        return 0;
    // Y, U and V in a plane each; or U and V side by side in the second
    // (NV12 and P010 have U first, NV21 V).
    int separate = c[1].plane == 1 && c[2].plane == 2 && c[1].step == bytes && c[2].step == bytes &&
                   c[1].offset == 0 && c[2].offset == 0;
    int paired = c[1].plane == 1 && c[2].plane == 1 && c[1].step == 2 * bytes &&
                 c[2].step == 2 * bytes &&
                 ((c[1].offset == 0 && c[2].offset == bytes) ||
                  (c[1].offset == bytes && c[2].offset == 0));
    if (!separate && !paired)
        return 0;

    int cw = AV_CEIL_RSHIFT(f->width, desc->log2_chroma_w);
    int row_bytes[3] = {f->width * bytes, paired ? cw * 2 * bytes : cw * bytes, cw * bytes};
    for (int i = 0; i < (paired ? 2 : 3); i++) {
        if (!f->data[i] || f->linesize[i] < row_bytes[i])
            return 0;
    }

    out->data[0] = f->data[0];
    out->data[1] = f->data[1];
    out->data[2] = paired ? f->data[1] : f->data[2];
    out->stride[0] = f->linesize[0];
    out->stride[1] = f->linesize[1];
    out->stride[2] = paired ? f->linesize[1] : f->linesize[2];
    out->chroma_shift_x = desc->log2_chroma_w;
    out->chroma_shift_y = desc->log2_chroma_h;
    out->interleaved = paired;
    out->v_first = paired && c[2].offset == 0;
    out->depth = depth;
    out->shift = shift;
    return 1;
}

// Sets `video->reason` to "<method>: <what> (<error>)" for a hardware failure,
// unless `get_format` already set a reason (the device does not support the
// stream), which is more useful: `av1` fails its first frame when no device
// can take the stream.
static int hardware_failed(vento_video *video, const char *what, int error) {
    if (video->reason[0])
        return HARDWARE_FAILED;
    char e[64];
    vento_video_strerror(error, e, sizeof(e));
    say(video, "%s: %s (%s)", vento_video_method_name(video->method), what, e);
    return HARDWARE_FAILED;
}

// Whether an error from a decoder on a device is the device's rather than
// the stream's. A stream that is broken is broken in software too, and is
// what libavcodec calls invalid data; what the hardware path fails with is
// anything else (VAAPI's own failures are EIO, an `av1` that has no hardware
// for the stream after all ENOSYS), and is worth a software decoder.
static int device_error(const vento_video *video, int error) {
    return video->method != VENTO_VIDEO_HW_OFF && error != AVERROR_INVALIDDATA &&
           error != AVERROR(EAGAIN) && error != AVERROR_EOF;
}

static int compare_us(const void *a, const void *b) {
    uint64_t x = *(const uint64_t *)a, y = *(const uint64_t *)b;
    return (x > y) - (x < y);
}

// The median of one way's trials (sorts them in place).
static uint64_t median(uint64_t trials[MAP_TRIALS]) {
    qsort(trials, MAP_TRIALS, sizeof(trials[0]), compare_us);
    return (trials[MAP_TRIALS / 2 - 1] + trials[MAP_TRIALS / 2]) / 2;
}

// Every row of a mapped picture's planes read once, as a conversion reads
// them, in microseconds.
static uint64_t read_through(const AVFrame *f) {
    uint64_t start = now_us();
    const AVPixFmtDescriptor *desc = av_pix_fmt_desc_get((enum AVPixelFormat)f->format);
    uint64_t sum = 0;
    for (int plane = 0; desc && plane < 4 && f->data[plane]; plane++) {
        int rows = plane == 0 || plane == 3 ? f->height
                                            : AV_CEIL_RSHIFT(f->height, desc->log2_chroma_h);
        size_t words = (size_t)abs(f->linesize[plane]) / sizeof(uint64_t);
        for (int r = 0; r < rows; r++) {
            const uint64_t *row =
                (const uint64_t *)(f->data[plane] + (ptrdiff_t)r * f->linesize[plane]);
            for (size_t w = 0; w < words; w++)
                sum += row[w];
        }
    }
    // Used, as far as the compiler knows, so that the reads are made.
    __asm__ __volatile__("" : : "r"(sum) : "memory");
    return now_us() - start;
}

// Whether a picture on the device can be held: VAAPI's, in a format whose
// layers the caller converts (see the header).
static int holdable(const vento_video *video, const AVFrame *f) {
    if (video->method != VENTO_VIDEO_HW_VAAPI || !f->hw_frames_ctx)
        return 0;
    enum AVPixelFormat sw = ((const AVHWFramesContext *)f->hw_frames_ctx->data)->sw_format;
    return sw == AV_PIX_FMT_NV12 || sw == AV_PIX_FMT_P010LE;
}

// The VAAPI picture handed out in `out->held`, once the GPU has decoded it;
// a libavcodec error where the wait fails, which is the device's.
static int hold_picture(vento_video *video, const AVFrame *f, struct vento_video_picture *out) {
    const AVHWFramesContext *frames = (const AVHWFramesContext *)f->hw_frames_ctx->data;
    const AVVAAPIDeviceContext *device = frames->device_ctx->hwctx;
    VASurfaceID surface = (VASurfaceID)(uintptr_t)f->data[3];
    // Wait for the decode to finish before handing the surface out: the
    // caller's GPU reads the DMA-BUF with no fence from the decoder to wait
    // on.
    VAStatus synced = vaSyncSurface(device->display, surface);
    if (synced != VA_STATUS_SUCCESS)
        return AVERROR(EIO);
    if (!video->pool_ref || video->pool_ref->data != f->hw_frames_ctx->data) {
        av_buffer_unref(&video->pool_ref);
        video->pool_ref = av_buffer_ref(f->hw_frames_ctx);
        if (!video->pool_ref)
            return AVERROR(ENOMEM);
        video->pool = atomic_fetch_add(&pools, 1) + 1;
    }
    vento_video_frame *held = calloc(1, sizeof(*held));
    if (!held)
        return AVERROR(ENOMEM);
    held->frame = av_frame_clone(f);
    if (!held->frame) {
        free(held);
        return AVERROR(ENOMEM);
    }
    out->held = held;
    out->pool = video->pool;
    out->surface = surface;
    out->width = f->width;
    out->height = f->height;
    out->depth = frames->sw_format == AV_PIX_FMT_P010LE ? 10 : 8;
    colour(f, &out->bt709, &out->full_range);
    return 0;
}

// One decode of the packet into `out`: a status of the header's, or
// HARDWARE_FAILED with the reason kept.
static int decode_packet(vento_video *video, const uint8_t *data, size_t len, int hold,
                         struct vento_video_picture *out, char *format, size_t format_len) {
    // libavcodec copies a packet it does not own into one with the padding
    // its bitstream readers need, so `data` is lent rather than kept.
    video->packet->data = (uint8_t *)data;
    video->packet->size = (int)len;
    video->packet->pts = video->pts++;
    int sent = avcodec_send_packet(video->context, video->packet);
    video->packet->data = NULL;
    video->packet->size = 0;
    if (sent < 0) {
        if (device_error(video, sent))
            return hardware_failed(video, "decode failed", sent);
        out->error = sent;
        return VENTO_VIDEO_REFUSED;
    }

    int got = 0;
    for (;;) {
        int received = avcodec_receive_frame(video->context, video->newer);
        if (received < 0) {
            if (device_error(video, received)) {
                av_frame_unref(video->frame);
                return hardware_failed(video, "decode failed", received);
            }
            if (received != AVERROR(EAGAIN) && received != AVERROR_EOF)
                out->error = received;
            break;
        }
        av_frame_unref(video->frame);
        av_frame_move_ref(video->frame, video->newer);
        got = 1;
    }
    if (!got)
        return VENTO_VIDEO_NONE;

    const AVFrame *picture = video->frame;
    if (hold && video->hw_format != AV_PIX_FMT_NONE && picture->format == video->hw_format &&
        holdable(video, picture)) {
        uint64_t waiting = now_us();
        int held = video->fail_next ? AVERROR(EIO) : hold_picture(video, picture, out);
        video->fail_next = 0;
        out->map_us = (uint32_t)(now_us() - waiting);
        if (held < 0) {
            av_frame_unref(video->frame);
            return hardware_failed(video, "sync failed", held);
        }
        video->hw_pictures++;
        out->hardware = 3;
        video->reason[0] = '\0';
        return VENTO_VIDEO_PICTURE;
    }
    if (video->hw_format != AV_PIX_FMT_NONE && picture->format == video->hw_format) {
        uint64_t mapping = now_us();
        int read = 0;
        int trial = -1;
        if (video->fail_next) {
            video->fail_next = 0;
            read = AVERROR(EIO);
        } else {
            if (!video->copy) {
                int direct = 0;
                if (video->map_verdict != MAP_UNKNOWN)
                    direct = video->map_verdict == MAP_DIRECT;
                else if (video->hw_pictures >= MAP_TRIALS_FROM && video->trials < 2 * MAP_TRIALS)
                    direct = trial = video->trials & 1;
                // Into the surfaces' own software format (NV12, P010, ...),
                // which is what a format of NONE asks for.
                video->mapped->format = AV_PIX_FMT_NONE;
                read = av_hwframe_map(video->mapped, picture,
                                      AV_HWFRAME_MAP_READ | (direct ? AV_HWFRAME_MAP_DIRECT : 0));
                if (direct && read == AVERROR(EINVAL)) {
                    // The driver cannot map its surfaces in place (FFmpeg
                    // tried when it made them), which is the verdict.
                    video->map_verdict = MAP_IMAGE;
                    trial = -1;
                    av_frame_unref(video->mapped);
                    video->mapped->format = AV_PIX_FMT_NONE;
                    read = av_hwframe_map(video->mapped, picture, AV_HWFRAME_MAP_READ);
                }
                if (read == AVERROR(ENOSYS))
                    video->copy = 1;
            }
            if (video->copy) {
                av_frame_unref(video->mapped);
                read = av_hwframe_transfer_data(video->mapped, picture, 0);
                if (read >= 0)
                    read = av_frame_copy_props(video->mapped, picture);
            }
        }
        if (trial >= 0 && read >= 0) {
            // What the caller's conversion will make of the memory, which
            // is the half of the cost the map does not show.
            uint64_t mapped = now_us() - mapping;
            video->trial_us[trial][video->trials / 2] = mapped + read_through(video->mapped);
            video->trials++;
            video->held_trial = trial;
        }
        out->map_us = (uint32_t)(now_us() - mapping);
        if (read < 0) {
            av_frame_unref(video->frame);
            return hardware_failed(video, video->copy ? "copy failed" : "map failed", read);
        }
        video->hw_pictures++;
        picture = video->mapped;
        out->hardware = video->copy ? 2 : 1;
        // On hardware, so nothing stands in its way, whatever the device
        // said of the stream's earlier profile.
        video->reason[0] = '\0';
    }
    if (!describe(picture, out)) {
        const char *name = av_get_pix_fmt_name((enum AVPixelFormat)picture->format);
        if (out->hardware) {
            snprintf(video->reason, sizeof(video->reason), "%s: its pictures are %s",
                     vento_video_method_name(video->method), name ? name : "an unknown format");
            av_frame_unref(video->mapped);
            av_frame_unref(video->frame);
            return HARDWARE_FAILED;
        }
        snprintf(format, format_len, "%s", name ? name : "an unknown format");
        av_frame_unref(video->frame);
        return VENTO_VIDEO_UNSUPPORTED;
    }
    return VENTO_VIDEO_PICTURE;
}

int vento_video_decode(vento_video *video, const uint8_t *data, size_t len, int hold,
                       struct vento_video_picture *out, char *format, size_t format_len) {
    uint64_t start = now_us();
    memset(out, 0, sizeof(*out));
    if (format_len > 0)
        format[0] = '\0';
    // The last picture described is the caller's until now, and so is the
    // mapping of it, which holds the surface it was decoded into.
    uint64_t unmapping = now_us();
    av_frame_unref(video->mapped);
    if (video->held_trial >= 0) {
        video->trial_us[video->held_trial][(video->trials - 1) / 2] += now_us() - unmapping;
        video->held_trial = -1;
        if (video->trials == 2 * MAP_TRIALS)
            video->map_verdict = median(video->trial_us[1]) < median(video->trial_us[0])
                                     ? MAP_DIRECT
                                     : MAP_IMAGE;
    }
    av_frame_unref(video->frame);

    if (len > INT32_MAX) {
        out->error = AVERROR(EINVAL);
        return VENTO_VIDEO_REFUSED;
    }
    if (video->dead) {
        out->error = video->dead;
        return VENTO_VIDEO_REFUSED;
    }
    int status = decode_packet(video, data, len, hold, out, format, format_len);
    if (status == HARDWARE_FAILED) {
        // The software decoder in the hardware one's place, given the same
        // packet: a key frame comes out of it at once, and anything else
        // waits for the key frame the caller is told to ask for. A device
        // that failed mid-stream is not the stream's, so it is not tried
        // again.
        memset(out, 0, sizeof(*out));
        int opened = open_decoder(video);
        out->fell_back = opened < 0 ? -1 : 1;
        if (opened < 0) {
            // Not even software: every packet from here on is refused. The
            // context on the device is freed rather than left to fail again,
            // and the software decoder is not retried on later packets.
            avcodec_free_context(&video->context);
            video->method = VENTO_VIDEO_HW_OFF;
            video->hw_format = AV_PIX_FMT_NONE;
            video->dead = opened;
            out->error = opened;
            status = VENTO_VIDEO_REFUSED;
        } else {
            status = decode_packet(video, data, len, hold, out, format, format_len);
        }
    }
    out->decode_us = (uint32_t)(now_us() - start);
    return status;
}

int vento_video_frame_export(vento_video_frame *frame, struct vento_video_export *out,
                             char *error, size_t error_len) {
    memset(out, 0, sizeof(*out));
    if (error_len > 0)
        error[0] = '\0';
    const AVFrame *f = frame->frame;
    const AVHWFramesContext *frames = (const AVHWFramesContext *)f->hw_frames_ctx->data;
    const AVVAAPIDeviceContext *device = frames->device_ctx->hwctx;
    VASurfaceID surface = (VASurfaceID)(uintptr_t)f->data[3];
    VADRMPRIMESurfaceDescriptor d;
    memset(&d, 0, sizeof(d));
    VAStatus exported = vaExportSurfaceHandle(
        device->display, surface, VA_SURFACE_ATTRIB_MEM_TYPE_DRM_PRIME_2,
        VA_EXPORT_SURFACE_READ_ONLY | VA_EXPORT_SURFACE_SEPARATE_LAYERS, &d);
    if (exported != VA_STATUS_SUCCESS) {
        snprintf(error, error_len, "the surface could not be exported (%s)",
                 vaErrorStr(exported));
        return AVERROR(EIO);
    }
    int made = 0;
    if (d.num_layers != 2 || d.num_objects < 1 || d.num_objects > 4) {
        snprintf(error, error_len, "the surface came out as %u layers in %u objects",
                 d.num_layers, d.num_objects);
        made = AVERROR(ENOSYS);
    }
    for (uint32_t i = 0; made == 0 && i < 2; i++) {
        uint32_t object = d.layers[i].object_index[0];
        if (d.layers[i].num_planes != 1 || object >= d.num_objects) {
            snprintf(error, error_len, "layer %u has %u planes", i, d.layers[i].num_planes);
            made = AVERROR(ENOSYS);
            break;
        }
        // A descriptor per layer, though both are usually the one object:
        // each is imported on its own, and an import takes its descriptor.
        int fd = fcntl(d.objects[object].fd, F_DUPFD_CLOEXEC, 0);
        if (fd < 0) {
            snprintf(error, error_len, "the descriptor could not be duplicated (%s)",
                     strerror(errno));
            made = AVERROR(errno);
            break;
        }
        // Y at the surface's size, and the chroma at half of it either way,
        // which is 4:2:0 for both formats held.
        out->layer[i] = (struct vento_video_layer){
            .fd = fd,
            .fourcc = d.layers[i].drm_format,
            .modifier = d.objects[object].drm_format_modifier,
            .offset = d.layers[i].offset[0],
            .pitch = d.layers[i].pitch[0],
            .width = i == 0 ? d.width : (d.width + 1) / 2,
            .height = i == 0 ? d.height : (d.height + 1) / 2,
        };
        out->layers = (int32_t)i + 1;
    }
    for (uint32_t i = 0; i < d.num_objects && i < 4; i++)
        close(d.objects[i].fd);
    if (made < 0) {
        for (int32_t i = 0; i < out->layers; i++)
            close(out->layer[i].fd);
        out->layers = 0;
        return made;
    }
    colour(f, &out->bt709, &out->full_range);
    return 0;
}

int vento_video_frame_map(vento_video_frame *frame, struct vento_video_picture *out) {
    memset(out, 0, sizeof(*out));
    if (!frame->mapped && !(frame->mapped = av_frame_alloc()))
        return 0;
    av_frame_unref(frame->mapped);
    // Into the surface's own format, as a decode maps it; or copied, where
    // the driver will not map it.
    frame->mapped->format = AV_PIX_FMT_NONE;
    int read = av_hwframe_map(frame->mapped, frame->frame, AV_HWFRAME_MAP_READ);
    if (read < 0) {
        av_frame_unref(frame->mapped);
        read = av_hwframe_transfer_data(frame->mapped, frame->frame, 0);
        if (read >= 0)
            read = av_frame_copy_props(frame->mapped, frame->frame);
    }
    if (read < 0 || !describe(frame->mapped, out)) {
        av_frame_unref(frame->mapped);
        return 0;
    }
    out->hardware = 1;
    return 1;
}

void vento_video_frame_unmap(vento_video_frame *frame) {
    if (frame->mapped)
        av_frame_unref(frame->mapped);
}

void vento_video_frame_free(vento_video_frame *frame) {
    if (!frame)
        return;
    av_frame_free(&frame->mapped);
    av_frame_free(&frame->frame);
    free(frame);
}
