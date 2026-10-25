// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

// The platform's sound output, as a C backend is handed it: a table of calls
// over `runtime::media::audio::Outputs` (AAudio on the phone, ALSA on Linux),
// filled in by `api::c::audio` and passed in a backend's config. Function
// pointers rather than symbols, for the reason `vento/video.h` gives: a backend
// library links nothing of the process it is loaded into.
//
// What is behind a stream is the same for every backend and both platforms:
// a queue that keeps as little in front of the device as the far end's own
// rhythm of writes needs not to run dry, and a margin that starts higher and
// comes down over a stream's first clean seconds and grows when the far
// end's sound turns out later than that (`runtime::media::audio::Queue`), with
// the device's own buffer behind it; never more than a quarter of a second
// (past that the oldest is dropped, so a burst or a stall costs a skip rather
// than a lag that grows), silence while it fills back up after running dry, the
// session's gain and mute, and a device opened off the caller's thread.
//
// And where the far end's sound arrives compressed, the platform's decoders
// for it are in the same table (AAC and Opus: libavcodec on Linux, MediaCodec
// on the phone, `runtime::media::audio::decode`), so that a backend offers its
// far end what the platform decodes and hands the packets here rather than to a
// decoder of its library's own. So a backend only writes what the far end
// sends, as it arrives, and never waits.

#ifndef VENTO_AUDIO_H
#define VENTO_AUDIO_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* Sample formats. Signed 16-bit, native-endian, a sample for each channel
   in turn: what QEMU's audio and RDP's PCM are, and what the AAC and Opus
   decoders hand back. */
#define VENTO_AUDIO_S16 1

/* Codecs, as `runtime::media::audio::decode::Codec` names them. AAC is raw
   access units, one frame a packet and no ADTS header, which is how RDP's
   sound sends it; Opus is one packet at a time, at 48 kHz. */
#define VENTO_AUDIO_CODEC_AAC 1
#define VENTO_AUDIO_CODEC_OPUS 2

/* What the session's sound switch does (`offer`): silence the far end's
   sound here, where nothing can ask the far end to stop sending it during a
   session (RDP's sound), or ask it to stop and to send again (QEMU's audio
   extension). As `runtime::media::audio::Switch` names them. */
#define VENTO_AUDIO_SWITCH_MUTE 1
#define VENTO_AUDIO_SWITCH_ENABLE 2

/* One stream of the far end's sound. Not safe to call into from two threads
   at once; from one thread after another is fine. */
typedef struct vento_audio_stream vento_audio_stream;

/* One decoder, for one stream's packets, on the same terms as a stream. */
typedef struct vento_audio_decoder vento_audio_decoder;

typedef struct vento_audio {
    /* The output itself, handed to `open`, `decodable`, `decoder_open` and
       `release`. */
    void *user;
    /* A stream of `format` samples at `rate` frames a second, `channels` of
       them a frame (1 to 8), writable at once whether or not the device
       behind it has opened yet. NULL where the format is not one it takes
       (a rate outside 8 to 384 kHz) or the platform has no output. */
    vento_audio_stream *(*open)(void *user, int format, int rate, int channels);
    /* `len` bytes of samples, cut anywhere: a part of a frame at the end is
       kept for the next write. Never waits: the samples are queued and the
       device takes them on a thread of its own. */
    void (*write)(vento_audio_stream *s, const void *data, size_t len);
    /* The stream's own gain, under the session's (which the person sets,
       and which mutes): what a far end that sets its own volume sets, as a
       factor, 1 as sent. */
    void (*set_gain)(vento_audio_stream *s, float gain);
    /* Milliseconds until a sample written now is heard: what is queued and
       the device's own buffer. What RDP's sound confirms to its server. */
    int (*latency)(vento_audio_stream *s);
    /* Drops what is queued and lets the device go, without waiting for
       it. */
    void (*close)(vento_audio_stream *s);
    /* Whether the platform decodes `codec`: what a backend offers its far
       end, which then sends only what it was offered. Cheap to ask again. */
    bool (*decodable)(void *user, int codec);
    /* A decoder for `codec` packets of a stream at `rate` in `channels`, set
       up by `config` — AAC's AudioSpecificConfig, Opus's identification
       header — or, where `config_len` is 0, by those numbers alone: AAC LC,
       and Opus with no pre-skip, which is what RDP's sound sends neither of.
       NULL where the platform will not decode it; why is logged. */
    vento_audio_decoder *(*decoder_open)(void *user, int codec, int rate, int channels,
                                         const uint8_t *config, size_t config_len);
    /* One packet. Returns the frames it made, with the samples — signed
       16-bit, interleaved, as a stream takes them — in `*samples`, which are
       the decoder's until its next call, at `*rate` in `*channels`, which
       are the decoder's answer rather than the stream's (an AAC stream with
       SBR decodes at twice its rate, Opus always at 48 kHz). 0 where the
       packet made none yet; -1 where it could not be decoded, which drops
       that packet and leaves the decoder ready for the next. */
    int (*decode)(vento_audio_decoder *d, const uint8_t *data, size_t len,
                  const int16_t **samples, int *rate, int *channels);
    void (*decoder_close)(vento_audio_decoder *d);
    /* The far end may send sound from now on, so the session shows its sound
       switch: said as early as the protocol says it (a QEMU server's audio
       acknowledgement, an RDP server's list of formats), and again whenever
       it changes. `sw` is VENTO_AUDIO_SWITCH_MUTE, with `enabled` true and no
       `enable`, where the far end cannot be asked to stop; or
       VENTO_AUDIO_SWITCH_ENABLE, `enabled` whether it is asked for its sound
       now, and `enable(ctx, on)` called when the person turns the switch — on
       their thread, a window's, never inside a call into this table, so it
       hands the question to the protocol's thread and returns, and it calls
       nothing here. */
    void (*offer)(void *user, int sw, bool enabled, void (*enable)(void *ctx, bool on),
                  void *ctx);
    /* The far end will send no more sound, or the session is ending: no
       switch. Once this returns `enable` is not called again, so a backend
       withdraws before `ctx` goes away; `release` withdraws too. */
    void (*withdraw)(void *user);
    /* The table is the backend's from the moment it is handed over: this is
       called once, after the last stream and decoder are closed, and also
       when the backend fails to start at all. */
    void (*release)(void *user);
} vento_audio;

#ifdef __cplusplus
}
#endif

#endif
