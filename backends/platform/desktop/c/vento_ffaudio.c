// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

// See vento_ffaudio.h. What is libavcodec's here and worth a note:
//
// - The setup is given as the stream's extradata, which for AAC is what
//   makes a raw frame mean anything (without it libavcodec guesses a
//   configuration from the context's rate and channels, which is what
//   FreeRDP's own decoding relied on), and for Opus is where the pre-skip is,
//   which libavcodec then drops from the start of the stream itself.
// - libavcodec reads past the end of a packet, up to its padding, so every
//   packet is copied into a buffer of ours that has it: a protocol's packet
//   sits in the middle of its message.
// - One thread: a packet of sound is a millisecond of work at most, and a
//   decoder with threads delays its output by a frame per thread.

#include "vento_ffaudio.h"

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include <libavcodec/avcodec.h>
#include <libavutil/channel_layout.h>
#include <libavutil/mathematics.h>
#include <libavutil/mem.h>

struct vento_ffaudio {
    AVCodecContext *context;
    AVPacket *packet;
    AVFrame *frame;
    // The packet, padded (see above).
    uint8_t *in;
    size_t in_cap;
    // What the last packet made.
    int16_t *out;
    size_t out_cap; // in samples
    // Where the next packet starts, in the stream's samples: a protocol
    // sends no timestamps, and libavcodec's Opus decoder warns at every
    // packet when it has none to move past its pre-skip.
    int64_t next_pts;
};

static const AVCodec *find(int codec) {
    switch (codec) {
    case VENTO_FFAUDIO_AAC:
        return avcodec_find_decoder(AV_CODEC_ID_AAC);
    case VENTO_FFAUDIO_OPUS:
        return avcodec_find_decoder(AV_CODEC_ID_OPUS);
    default:
        return NULL;
    }
}

int vento_ffaudio_decodable(int codec) {
    return find(codec) != NULL;
}

static void say_error(int error, char *buf, size_t len) {
    if (len == 0)
        return;
    if (av_strerror(error, buf, len) < 0)
        snprintf(buf, len, "error %d", error);
}

void vento_ffaudio_close(vento_ffaudio *audio) {
    if (!audio)
        return;
    av_frame_free(&audio->frame);
    av_packet_free(&audio->packet);
    avcodec_free_context(&audio->context);
    av_free(audio->in);
    free(audio->out);
    free(audio);
}

vento_ffaudio *vento_ffaudio_open(int codec, int rate, int channels, const uint8_t *config,
                                  size_t config_len, char *error, size_t error_len) {
    if (error_len > 0)
        error[0] = '\0';
    const AVCodec *decoder = find(codec);
    if (!decoder) {
        snprintf(error, error_len, "no decoder for it in this libavcodec");
        return NULL;
    }
    if (rate <= 0 || channels <= 0 || config_len > (1 << 16)) {
        snprintf(error, error_len, "%d Hz in %d channels is no stream", rate, channels);
        return NULL;
    }
    vento_ffaudio *audio = calloc(1, sizeof(*audio));
    if (!audio) {
        snprintf(error, error_len, "out of memory");
        return NULL;
    }
    audio->context = avcodec_alloc_context3(decoder);
    audio->packet = av_packet_alloc();
    audio->frame = av_frame_alloc();
    if (!audio->context || !audio->packet || !audio->frame) {
        snprintf(error, error_len, "out of memory");
        vento_ffaudio_close(audio);
        return NULL;
    }
    AVCodecContext *c = audio->context;
    c->sample_rate = rate;
    c->pkt_timebase = (AVRational){1, rate};
    av_channel_layout_default(&c->ch_layout, channels);
    c->thread_count = 1;
    if (config_len > 0) {
        c->extradata = av_mallocz(config_len + AV_INPUT_BUFFER_PADDING_SIZE);
        if (!c->extradata) {
            snprintf(error, error_len, "out of memory");
            vento_ffaudio_close(audio);
            return NULL;
        }
        memcpy(c->extradata, config, config_len);
        c->extradata_size = (int)config_len;
    }
    int opened = avcodec_open2(c, decoder, NULL);
    if (opened < 0) {
        say_error(opened, error, error_len);
        vento_ffaudio_close(audio);
        return NULL;
    }
    return audio;
}

const char *vento_ffaudio_name(const vento_ffaudio *audio) {
    return audio->context->codec->name;
}

static inline int16_t from_float(float s) {
    float scaled = s * 32768.0f;
    // NaN fails both comparisons and is silence.
    if (!(scaled > -32768.0f))
        return scaled == scaled ? INT16_MIN : 0;
    if (!(scaled < 32767.0f))
        return INT16_MAX;
    return (int16_t)(scaled >= 0 ? scaled + 0.5f : scaled - 0.5f);
}

// One frame's samples appended at `at`, interleaved. False for a sample
// format neither decoder produces.
static int append(const AVFrame *f, int16_t *at) {
    const int ch = f->ch_layout.nb_channels;
    const int n = f->nb_samples;
    switch (f->format) {
    case AV_SAMPLE_FMT_FLTP:
        for (int c = 0; c < ch; c++) {
            const float *plane = (const float *)f->extended_data[c];
            for (int i = 0; i < n; i++)
                at[i * ch + c] = from_float(plane[i]);
        }
        return 1;
    case AV_SAMPLE_FMT_FLT: {
        const float *s = (const float *)f->extended_data[0];
        for (int i = 0; i < n * ch; i++)
            at[i] = from_float(s[i]);
        return 1;
    }
    case AV_SAMPLE_FMT_S16P:
        for (int c = 0; c < ch; c++) {
            const int16_t *plane = (const int16_t *)f->extended_data[c];
            for (int i = 0; i < n; i++)
                at[i * ch + c] = plane[i];
        }
        return 1;
    case AV_SAMPLE_FMT_S16:
        memcpy(at, f->extended_data[0], (size_t)n * (size_t)ch * sizeof(int16_t));
        return 1;
    default:
        return 0;
    }
}

int vento_ffaudio_decode(vento_ffaudio *audio, const uint8_t *data, size_t len,
                         const int16_t **samples, int *rate, int *channels, char *error,
                         size_t error_len) {
    if (error_len > 0)
        error[0] = '\0';
    *samples = audio->out;
    if (len == 0 || len > INT32_MAX - AV_INPUT_BUFFER_PADDING_SIZE)
        return 0;
    if (audio->in_cap < len + AV_INPUT_BUFFER_PADDING_SIZE) {
        av_free(audio->in);
        audio->in_cap = len + AV_INPUT_BUFFER_PADDING_SIZE;
        audio->in = av_malloc(audio->in_cap);
        if (!audio->in) {
            audio->in_cap = 0;
            snprintf(error, error_len, "out of memory");
            return -1;
        }
    }
    memcpy(audio->in, data, len);
    memset(audio->in + len, 0, AV_INPUT_BUFFER_PADDING_SIZE);
    audio->packet->data = audio->in;
    audio->packet->size = (int)len;
    audio->packet->pts = audio->packet->dts = audio->next_pts;

    int sent = avcodec_send_packet(audio->context, audio->packet);
    av_packet_unref(audio->packet);
    if (sent < 0) {
        say_error(sent, error, error_len);
        return -1;
    }
    size_t used = 0;
    int frames = 0;
    for (;;) {
        int got = avcodec_receive_frame(audio->context, audio->frame);
        if (got == AVERROR(EAGAIN) || got == AVERROR_EOF)
            break;
        if (got < 0) {
            say_error(got, error, error_len);
            return frames > 0 ? frames : -1;
        }
        const AVFrame *f = audio->frame;
        const int ch = f->ch_layout.nb_channels;
        // Every frame of a packet is at one rate and layout; one that is not
        // is a stream that changed between them, and what came before the
        // change is what this packet returns.
        if (ch <= 0 || (frames > 0 && (ch != *channels || f->sample_rate != *rate))) {
            av_frame_unref(audio->frame);
            break;
        }
        const size_t need = used + (size_t)f->nb_samples * (size_t)ch;
        if (need > audio->out_cap) {
            int16_t *grown = realloc(audio->out, need * sizeof(int16_t));
            if (!grown) {
                av_frame_unref(audio->frame);
                snprintf(error, error_len, "out of memory");
                return -1;
            }
            audio->out = grown;
            audio->out_cap = need;
        }
        if (!append(f, audio->out + used)) {
            snprintf(error, error_len, "the decoder made samples in %s, which this does not convert",
                     av_get_sample_fmt_name(f->format) ? av_get_sample_fmt_name(f->format) : "?");
            av_frame_unref(audio->frame);
            return -1;
        }
        used = need;
        frames += f->nb_samples;
        if (f->sample_rate > 0)
            audio->next_pts += av_rescale(f->nb_samples, audio->context->pkt_timebase.den, f->sample_rate);
        *rate = f->sample_rate;
        *channels = ch;
        av_frame_unref(audio->frame);
    }
    *samples = audio->out;
    return frames;
}
