// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

// A desktop sound decoder: libavcodec's AAC and Opus behind a few calls, the
// only FFmpeg interface `platform_linux::ffmpeg_audio` uses, narrow for the
// same reason as `vento_video.h`: the Rust half declares these by hand and
// needs no FFmpeg headers.
//
// A packet goes in and all of its samples come out at once, signed 16-bit and
// interleaved, converted here from the decoder's format (planar floating point
// for both): the format vento's queue takes. There is no resampler, since the
// rate is the stream's and the platform's sound server resamples properly
// where the device's differs.
#ifndef VENTO_FFAUDIO_H
#define VENTO_FFAUDIO_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

// The codecs, in the order `runtime::media::audio::decode::Codec` declares
// them, from one.
enum vento_ffaudio_codec {
    VENTO_FFAUDIO_AAC = 1,
    VENTO_FFAUDIO_OPUS = 2,
};

typedef struct vento_ffaudio vento_ffaudio;

// Whether this libavcodec has the codec's decoder.
int vento_ffaudio_decodable(int codec);

// A decoder for packets of a stream of `rate` and `channels`, set up by
// `config` (AAC's AudioSpecificConfig, Opus's identification header), which
// the caller always gives. NULL, with the reason in `error`, if it does not
// open.
vento_ffaudio *vento_ffaudio_open(int codec, int rate, int channels, const uint8_t *config,
                                  size_t config_len, char *error, size_t error_len);

// The decoder's name in libavcodec: "aac", "opus".
const char *vento_ffaudio_name(const vento_ffaudio *audio);

// Decodes one packet. Returns the number of frames decoded, with the samples in
// `*samples` (the decoder's buffer, valid until its next call) at `*rate` in
// `*channels`; 0 if it has decoded none yet; and a negative number if the
// packet could not be decoded, with the reason in `error`. A failed packet is
// dropped and the decoder stays ready for the next.
int vento_ffaudio_decode(vento_ffaudio *audio, const uint8_t *data, size_t len,
                         const int16_t **samples, int *rate, int *channels, char *error,
                         size_t error_len);

void vento_ffaudio_close(vento_ffaudio *audio);

#ifdef __cplusplus
}
#endif

#endif
