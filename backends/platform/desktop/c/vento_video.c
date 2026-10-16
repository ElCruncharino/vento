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
//   acceleration and decodes nothing without it, so AV1 is dav1d's wrapper,
//   which the build links in, rather than a decoder that opens and then fails
//   every frame.
// - **Only the last picture.** Each that comes out of one packet is a whole
//   picture, and one the next overwrites before anybody sees it is a
//   conversion for nothing, so the others are counted and dropped.

#include "vento_video.h"

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include <libavcodec/avcodec.h>
#include <libavutil/error.h>
#include <libavutil/pixdesc.h>

struct vento_video {
    AVCodecContext *context;
    AVPacket *packet;
    // The picture last described, and the one a newer is received into
    // while it waits: two, so that only the last of several is read.
    AVFrame *frame;
    AVFrame *newer;
    int64_t pts;
};

size_t vento_video_picture_size(void) {
    return sizeof(struct vento_video_picture);
}

static const AVCodec *find(int codec) {
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
        return avcodec_find_decoder_by_name("libdav1d");
    default:
        return NULL;
    }
}

int vento_video_decodable(int codec) {
    return find(codec) != NULL;
}

void vento_video_strerror(int error, char *buf, size_t len) {
    if (len == 0)
        return;
    if (av_strerror(error, buf, len) < 0)
        snprintf(buf, len, "error %d", error);
}

void vento_video_close(vento_video *video) {
    if (!video)
        return;
    av_frame_free(&video->newer);
    av_frame_free(&video->frame);
    av_packet_free(&video->packet);
    avcodec_free_context(&video->context);
    free(video);
}

vento_video *vento_video_open(int codec, int width, int height, char *error, size_t error_len) {
    if (error_len > 0)
        error[0] = '\0';
    const AVCodec *found = find(codec);
    if (!found) {
        snprintf(error, error_len, "no decoder for it in this libavcodec");
        return NULL;
    }
    vento_video *video = calloc(1, sizeof(*video));
    if (!video) {
        snprintf(error, error_len, "out of memory");
        return NULL;
    }
    video->context = avcodec_alloc_context3(found);
    video->packet = av_packet_alloc();
    video->frame = av_frame_alloc();
    video->newer = av_frame_alloc();
    if (!video->context || !video->packet || !video->frame || !video->newer) {
        snprintf(error, error_len, "out of memory");
        vento_video_close(video);
        return NULL;
    }
    // A hint rather than a promise: the stream says what size it is, and
    // what is read is the frame's own.
    video->context->width = width;
    video->context->height = height;
    video->context->flags |= AV_CODEC_FLAG_LOW_DELAY;
    video->context->thread_type = FF_THREAD_SLICE;
    video->context->thread_count = 0; // as many as the machine has
    int opened = avcodec_open2(video->context, found, NULL);
    if (opened < 0) {
        vento_video_strerror(opened, error, error_len);
        vento_video_close(video);
        return NULL;
    }
    return video;
}

const char *vento_video_name(const vento_video *video) {
    return video->context->codec->name;
}

// The received frame described into `out`, or false for a format that is not
// planar YUV of eight to sixteen bits a sample with U and V in planes of
// their own or (at eight bits) interleaved in one: RGB, a palette,
// big-endian samples (which no decoder on a little-endian machine hands
// back), hardware surfaces, or chroma subsampled other than 4:2:0, 4:2:2,
// 4:4:0 or 4:4:4. An alpha plane after the three is not read.
static int describe(const AVFrame *f, struct vento_video_picture *out) {
    const AVPixFmtDescriptor *desc = av_pix_fmt_desc_get((enum AVPixelFormat)f->format);
    if (!desc || f->width <= 0 || f->height <= 0)
        return 0;
    const uint64_t unwanted = AV_PIX_FMT_FLAG_BE | AV_PIX_FMT_FLAG_PAL | AV_PIX_FMT_FLAG_BITSTREAM |
                              AV_PIX_FMT_FLAG_HWACCEL | AV_PIX_FMT_FLAG_RGB | AV_PIX_FMT_FLAG_BAYER |
                              AV_PIX_FMT_FLAG_FLOAT | AV_PIX_FMT_FLAG_XYZ;
    if ((desc->flags & unwanted) || !(desc->flags & AV_PIX_FMT_FLAG_PLANAR) ||
        desc->nb_components < 3 || desc->log2_chroma_w > 1 || desc->log2_chroma_h > 1)
        return 0;

    const AVComponentDescriptor *c = desc->comp;
    int depth = c[0].depth;
    int bytes = depth > 8 ? 2 : 1;
    for (int i = 0; i < 3; i++) {
        if (c[i].depth != depth || c[i].shift != 0)
            return 0;
    }
    if (depth < 8 || depth > 16 || c[0].plane != 0 || c[0].step != bytes || c[0].offset != 0)
        return 0;
    // Y, U and V in a plane each; or, at eight bits, U and V side by side in
    // the second (NV12 has U first, NV21 V).
    int separate = c[1].plane == 1 && c[2].plane == 2 && c[1].step == bytes && c[2].step == bytes &&
                   c[1].offset == 0 && c[2].offset == 0;
    int paired = depth == 8 && c[1].plane == 1 && c[2].plane == 1 && c[1].step == 2 && c[2].step == 2 &&
                 ((c[1].offset == 0 && c[2].offset == 1) || (c[1].offset == 1 && c[2].offset == 0));
    if (!separate && !paired)
        return 0;

    int cw = AV_CEIL_RSHIFT(f->width, desc->log2_chroma_w);
    int row_bytes[3] = {f->width * bytes, paired ? cw * 2 : cw * bytes, cw * bytes};
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
    out->width = f->width;
    out->height = f->height;
    out->chroma_shift_x = desc->log2_chroma_w;
    out->chroma_shift_y = desc->log2_chroma_h;
    out->interleaved = paired;
    out->v_first = paired && c[2].offset == 0;
    out->depth = depth;
    // BT.601 for BT.601, and for a stream that says nothing — which is what
    // the encoders met so far produce untagged, and what libavcodec's own
    // scaler takes silence for — and BT.709 for anything else that names
    // itself.
    out->bt709 = !(f->colorspace == AVCOL_SPC_BT470BG || f->colorspace == AVCOL_SPC_SMPTE170M ||
                   f->colorspace == AVCOL_SPC_UNSPECIFIED);
    // The YUVJ formats are full range whatever the frame's range says.
    out->full_range = f->color_range == AVCOL_RANGE_JPEG || f->format == AV_PIX_FMT_YUVJ420P ||
                      f->format == AV_PIX_FMT_YUVJ422P || f->format == AV_PIX_FMT_YUVJ444P;
    return 1;
}

int vento_video_decode(vento_video *video, const uint8_t *data, size_t len,
                       struct vento_video_picture *out, char *format, size_t format_len) {
    memset(out, 0, sizeof(*out));
    if (format_len > 0)
        format[0] = '\0';
    // The last picture described is the caller's until now.
    av_frame_unref(video->frame);

    if (len > INT32_MAX) {
        out->error = AVERROR(EINVAL);
        return VENTO_VIDEO_REFUSED;
    }
    // libavcodec copies a packet it does not own into one with the padding
    // its bitstream readers need, so `data` is lent rather than kept.
    video->packet->data = (uint8_t *)data;
    video->packet->size = (int)len;
    video->packet->pts = video->pts++;
    int sent = avcodec_send_packet(video->context, video->packet);
    video->packet->data = NULL;
    video->packet->size = 0;
    if (sent < 0) {
        out->error = sent;
        return VENTO_VIDEO_REFUSED;
    }

    int got = 0;
    for (;;) {
        int received = avcodec_receive_frame(video->context, video->newer);
        if (received < 0) {
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
    if (!describe(video->frame, out)) {
        const char *name = av_get_pix_fmt_name((enum AVPixelFormat)video->frame->format);
        snprintf(format, format_len, "%s", name ? name : "an unknown format");
        av_frame_unref(video->frame);
        return VENTO_VIDEO_UNSUPPORTED;
    }
    return VENTO_VIDEO_PICTURE;
}
