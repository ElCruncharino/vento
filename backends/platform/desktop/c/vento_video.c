// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

// vento_video.h over libavcodec. See the header for what crosses; this file
// is the only one that includes FFmpeg's.
//
// The choices that are libavcodec's to make are made here:
//
// - **Low delay, and slice threads only.** A decoder with frame threads holds
//   one frame per thread before the first comes out, and H.264 without
//   `LOW_DELAY` holds frames back to reorder them — a desktop that lags the
//   finger on it, for a stream that has nothing to reorder. dav1d reads the
//   same flag as a frame delay of one.
// - **AV1 by name.** libavcodec's own `av1` decoder is a front for hardware
//   acceleration and decodes nothing without it, so AV1 in software is
//   dav1d's wrapper, which the build links in, rather than a decoder that
//   opens and then fails every frame; `av1` is what decodes on hardware.
// - **Only the last picture.** Each that comes out of one packet is a whole
//   picture, and one the next overwrites before anybody sees it is a
//   conversion for nothing, so the others are counted and dropped.
//
// And the hardware's, when it is asked for:
//
// - **One device per kind, for the process.** Opening a VAAPI device is tens
//   of milliseconds (the driver loads and initialises), and a session makes a
//   decoder on every resize and codec switch, on whichever protocol thread it
//   runs on. So the first decoder to ask opens it, under a lock, and every
//   later one takes a reference to the same; it is kept until the process
//   ends. A device that failed to open is not tried again: a machine without
//   one does not grow one mid-session, and a retry per decoder would be the
//   same delay and the same log line every time.
// - **The stream's profile decides, through `get_format`.** libavcodec offers
//   the hardware's format when it has a hardware decoder for the codec, and
//   when the driver then has no decoder for the profile (iHD has none for
//   H.264 High 10) it asks again without it; either way what comes back is
//   the software format, the reason is kept, and the stream decodes in
//   software from the same context. AV1 is the exception: `av1` decodes only
//   on hardware, so without it the decoder is reopened as dav1d.
// - **Mapped, not copied.** The decoded surface is mapped for reading
//   (`av_hwframe_map`) and described in place, which measured about 40% less
//   time and half the CPU of `av_hwframe_transfer_data`'s copy into a frame
//   of our own; the caller's conversion reads the planes once either way.
//   Where a surface cannot be mapped (Vulkan's images) it is copied.
// - **A failure is a fallback, not an error.** A hardware decode or map that
//   fails mid-stream closes the decoder, opens the software one in its place
//   within the same handle, gives it the packet again and says so
//   (`fell_back`), for the caller to ask for a key frame. The session goes on
//   in software rather than losing its picture.

#include "vento_video.h"

#include <pthread.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

#include <libavcodec/avcodec.h>
#include <libavutil/error.h>
#include <libavutil/hwcontext.h>
#include <libavutil/pixdesc.h>

#include "va_shim.h"

// What one packet's decode came to when the hardware failed it, which the
// caller never sees: the decoder falls back and decodes the packet again.
#define HARDWARE_FAILED (-100)

struct vento_video {
    int codec;
    int width;
    int height;
    AVCodecContext *context;
    AVPacket *packet;
    // The picture last described, and the one a newer is received into
    // while it waits: two, so that only the last of several is read.
    AVFrame *frame;
    AVFrame *newer;
    // A hardware picture's planes, mapped (or copied) for reading.
    AVFrame *mapped;
    int64_t pts;
    // The device decoded on (a `vento_video_hardware`), or OFF, and the
    // format its surfaces come out in.
    int method;
    enum AVPixelFormat hw_format;
    // Copied rather than mapped, once a map has been refused as impossible.
    int copy;
    int fail_next;
    char reason[160];
};

size_t vento_video_picture_size(void) {
    return sizeof(struct vento_video_picture);
}

// The software decoder for the codec, or (`hardware`) the one libavcodec
// decodes on a device with, which is the same one but for AV1.
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

// The codec as a person reads it, for a reason.
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
    case VENTO_VIDEO_HW_VULKAN:
        return "vulkan";
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

// The shared devices, by `vento_video_hardware` (VAAPI and VULKAN).
static struct {
    AVBufferRef *ref;
    // 0 not tried, 1 open, -1 failed (why says why).
    int state;
    char why[128];
} devices[3];
static pthread_mutex_t devices_lock = PTHREAD_MUTEX_INITIALIZER;

int vento_video_device_state(int method) {
    if (method != VENTO_VIDEO_HW_VAAPI && method != VENTO_VIDEO_HW_VULKAN)
        return 0;
    pthread_mutex_lock(&devices_lock);
    int state = devices[method].state;
    pthread_mutex_unlock(&devices_lock);
    return state;
}

// A reference to the device of `method`, opening it if nobody has, or NULL
// with "<method>: <why>" in `why`.
static AVBufferRef *device(int method, char *why, size_t why_len) {
    static const enum AVHWDeviceType types[] = {
        [VENTO_VIDEO_HW_VAAPI] = AV_HWDEVICE_TYPE_VAAPI,
        [VENTO_VIDEO_HW_VULKAN] = AV_HWDEVICE_TYPE_VULKAN,
    };
    pthread_mutex_lock(&devices_lock);
    if (devices[method].state == 0) {
        // The default device of each: VAAPI's first DRM render node,
        // Vulkan's first physical device.
        int made = av_hwdevice_ctx_create(&devices[method].ref, types[method], NULL, NULL, 0);
        if (made >= 0) {
            devices[method].state = 1;
        } else {
            devices[method].state = -1;
            const char *missing = NULL;
            if (method == VENTO_VIDEO_HW_VAAPI && !vento_va_loaded(&missing)) {
                snprintf(devices[method].why, sizeof(devices[method].why), "no libva (%s)",
                         missing);
            } else if (made == AVERROR_EXTERNAL) {
                // Which is all libavcodec says when there is no render node
                // or the driver would not initialise; what it logged said
                // which.
                snprintf(devices[method].why, sizeof(devices[method].why), "no device");
            } else {
                char error[64];
                vento_video_strerror(made, error, sizeof(error));
                snprintf(devices[method].why, sizeof(devices[method].why), "no device (%s)",
                         error);
            }
        }
    }
    AVBufferRef *ref = devices[method].ref ? av_buffer_ref(devices[method].ref) : NULL;
    if (!ref)
        snprintf(why, why_len, "%s: %s", vento_video_method_name(method),
                 devices[method].ref ? "out of memory" : devices[method].why);
    pthread_mutex_unlock(&devices_lock);
    return ref;
}

// libavcodec's choice of format, made the hardware's where it is offered and
// the software's where it is not, which is also where a hardware decoder that
// failed to start for this stream's profile ends up: libavcodec asks again
// without it. Called again on every change of the stream's parameters.
static enum AVPixelFormat get_format(AVCodecContext *context, const enum AVPixelFormat *formats) {
    vento_video *video = context->opaque;
    for (const enum AVPixelFormat *f = formats; *f != AV_PIX_FMT_NONE; f++) {
        if (*f == video->hw_format) {
            video->reason[0] = '\0';
            return *f;
        }
    }
    const char *profile = avcodec_profile_name(context->codec_id, context->profile);
    const char *sw = av_get_pix_fmt_name(context->sw_pix_fmt);
    if (profile)
        snprintf(video->reason, sizeof(video->reason), "%s: %s %s is not supported",
                 vento_video_method_name(video->method), codec_name(video->codec), profile);
    else
        snprintf(video->reason, sizeof(video->reason), "%s: %s in %s is not supported",
                 vento_video_method_name(video->method), codec_name(video->codec),
                 sw ? sw : "this format");
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
    video->hw_format = !device                         ? AV_PIX_FMT_NONE
                       : method == VENTO_VIDEO_HW_VAAPI ? AV_PIX_FMT_VAAPI
                                                        : AV_PIX_FMT_VULKAN;
    video->copy = 0;
    return 0;
}

void vento_video_close(vento_video *video) {
    if (!video)
        return;
    av_frame_free(&video->mapped);
    av_frame_free(&video->newer);
    av_frame_free(&video->frame);
    av_packet_free(&video->packet);
    avcodec_free_context(&video->context);
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
    video->packet = av_packet_alloc();
    video->frame = av_frame_alloc();
    video->newer = av_frame_alloc();
    video->mapped = av_frame_alloc();
    if (!video->packet || !video->frame || !video->newer || !video->mapped) {
        snprintf(error, error_len, "out of memory");
        vento_video_close(video);
        return NULL;
    }

    // The first device of those asked for that opens, and a decoder on it
    // that opens; anything else is the software decoder, with why.
    int methods[2] = {VENTO_VIDEO_HW_OFF, VENTO_VIDEO_HW_OFF};
    if (hardware == VENTO_VIDEO_HW_VAAPI || hardware == VENTO_VIDEO_HW_VULKAN) {
        methods[0] = hardware;
    } else if (hardware == VENTO_VIDEO_HW_AUTO) {
        methods[0] = VENTO_VIDEO_HW_VAAPI;
        methods[1] = VENTO_VIDEO_HW_VULKAN;
    }
    for (int i = 0; i < 2 && methods[i] != VENTO_VIDEO_HW_OFF && !video->context; i++) {
        char why[sizeof(video->reason)];
        AVBufferRef *ref = device(methods[i], why, sizeof(why));
        int opened = ref ? open_context(video, methods[i], ref) : -1;
        if (ref && opened < 0) {
            char e[64];
            vento_video_strerror(opened, e, sizeof(e));
            snprintf(why, sizeof(why), "%s: the decoder did not open (%s)",
                     vento_video_method_name(methods[i]), e);
        }
        // The first method's reason is the one worth reading: `auto`'s
        // second is a stand-in.
        if (!video->context && !video->reason[0])
            snprintf(video->reason, sizeof(video->reason), "%s", why);
    }
    if (video->context)
        video->reason[0] = '\0';
    else {
        int opened = open_context(video, VENTO_VIDEO_HW_OFF, NULL);
        if (opened < 0) {
            vento_video_strerror(opened, error, error_len);
            vento_video_close(video);
            return NULL;
        }
    }
    return video;
}

const char *vento_video_name(const vento_video *video) {
    return video->context->codec->name;
}

int vento_video_method(const vento_video *video) {
    return video->method;
}

const char *vento_video_software_reason(const vento_video *video) {
    return video->reason;
}

void vento_video_fail_next(vento_video *video) {
    video->fail_next = 1;
}

// The received frame described into `out`, or false for a format that is not
// YUV of eight to sixteen bits a sample with Y in a plane of its own and U and
// V in planes of their own or interleaved in one (or packed as XV30): RGB, a
// palette, big-endian samples (which no decoder on a little-endian machine
// hands back), hardware surfaces, packed formats but that one, or chroma
// subsampled other than 4:2:0, 4:2:2, 4:4:0 or 4:4:4. An alpha plane after
// the three is not read.
static int describe(const AVFrame *f, struct vento_video_picture *out) {
    const AVPixFmtDescriptor *desc = av_pix_fmt_desc_get((enum AVPixelFormat)f->format);
    if (!desc || f->width <= 0 || f->height <= 0)
        return 0;

    // BT.601 for BT.601, and for a stream that says nothing — which is what
    // the encoders met so far produce untagged, and what libavcodec's own
    // scaler takes silence for — and BT.709 for anything else that names
    // itself.
    out->bt709 = !(f->colorspace == AVCOL_SPC_BT470BG || f->colorspace == AVCOL_SPC_SMPTE170M ||
                   f->colorspace == AVCOL_SPC_UNSPECIFIED);
    // The YUVJ formats are full range whatever the frame's range says.
    out->full_range = f->color_range == AVCOL_RANGE_JPEG || f->format == AV_PIX_FMT_YUVJ420P ||
                      f->format == AV_PIX_FMT_YUVJ422P || f->format == AV_PIX_FMT_YUVJ444P;
    out->width = f->width;
    out->height = f->height;

    // The one packed format read, which VAAPI decodes HEVC 4:4:4 at ten bits
    // into, and which no descriptor of planes can say.
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

// `video->reason` as "<method>: <what> (<error>)", for a hardware failure —
// unless `get_format` has already said why the stream is not the hardware's,
// which is the better reason: `av1` fails its first frame for want of a
// device that can take the stream, and that is what is worth reading.
static int hardware_failed(vento_video *video, const char *what, int error) {
    if (video->reason[0])
        return HARDWARE_FAILED;
    char e[64];
    vento_video_strerror(error, e, sizeof(e));
    snprintf(video->reason, sizeof(video->reason), "%s: %s (%s)",
             vento_video_method_name(video->method), what, e);
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

// One decode of the packet into `out`: a status of the header's, or
// HARDWARE_FAILED with the reason kept.
static int decode_packet(vento_video *video, const uint8_t *data, size_t len,
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
        if (got)
            out->skipped++;
        av_frame_unref(video->frame);
        av_frame_move_ref(video->frame, video->newer);
        got = 1;
    }
    if (!got)
        return VENTO_VIDEO_NONE;

    const AVFrame *picture = video->frame;
    if (video->hw_format != AV_PIX_FMT_NONE && picture->format == video->hw_format) {
        uint64_t mapping = now_us();
        int read = 0;
        if (video->fail_next) {
            video->fail_next = 0;
            read = AVERROR(EIO);
        } else {
            if (!video->copy) {
                // Into the surfaces' own software format (NV12, P010, ...),
                // which is what a format of NONE asks for.
                video->mapped->format = AV_PIX_FMT_NONE;
                read = av_hwframe_map(video->mapped, picture, AV_HWFRAME_MAP_READ);
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
        out->map_us = (uint32_t)(now_us() - mapping);
        if (read < 0) {
            av_frame_unref(video->frame);
            return hardware_failed(video, video->copy ? "copy failed" : "map failed", read);
        }
        picture = video->mapped;
        out->hardware = video->copy ? 2 : 1;
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

int vento_video_decode(vento_video *video, const uint8_t *data, size_t len,
                       struct vento_video_picture *out, char *format, size_t format_len) {
    uint64_t start = now_us();
    memset(out, 0, sizeof(*out));
    if (format_len > 0)
        format[0] = '\0';
    // The last picture described is the caller's until now, and so is the
    // mapping of it, which holds the surface it was decoded into.
    av_frame_unref(video->mapped);
    av_frame_unref(video->frame);

    if (len > INT32_MAX) {
        out->error = AVERROR(EINVAL);
        return VENTO_VIDEO_REFUSED;
    }
    int status = decode_packet(video, data, len, out, format, format_len);
    if (status == HARDWARE_FAILED) {
        // The software decoder in the hardware one's place, given the same
        // packet: a key frame comes out of it at once, and anything else
        // waits for the key frame the caller is told to ask for.
        memset(out, 0, sizeof(*out));
        int opened = open_context(video, VENTO_VIDEO_HW_OFF, NULL);
        out->fell_back = 1;
        if (opened < 0) {
            // Not even software: the decoder is no decoder now, and every
            // packet from here on is refused.
            out->error = opened;
            status = VENTO_VIDEO_REFUSED;
        } else {
            status = decode_packet(video, data, len, out, format, format_len);
        }
    }
    out->decode_us = (uint32_t)(now_us() - start);
    return status;
}
