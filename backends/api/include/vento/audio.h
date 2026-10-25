// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

// The platform's sound output for a C backend: a table of calls over
// `runtime::media::audio::Outputs` (AAudio on the phone; on the Linux client
// PipeWire's own streams, or ALSA's `default` device when no PipeWire daemon
// answers), filled in by `api::c::audio` and passed in a backend's config.
// Function pointers rather than symbols, for the reason `vento/video.h`
// gives: a backend library links nothing from the process it is loaded into.
//
// Every backend on both platforms gets the same stream handling: a queue
// that keeps only as much ahead of the device as the far end's write timing
// needs to avoid running dry, plus a margin that starts higher, shrinks over
// a stream's first clean seconds and grows when the far end's sound arrives
// later than that (`runtime::media::audio::Queue`), with the device's own
// buffer behind it. The queue never holds more than a quarter of a second
// (past that the oldest samples are dropped, so a burst or a stall costs a
// skip rather than a growing lag). It also plays silence while refilling
// after running dry, applies the session's gain and mute, and opens the
// device off the caller's thread.
//
// For compressed sound, the platform's decoders are in the same table (AAC
// and Opus: libavcodec on the Linux client, MediaCodec on the phone,
// `runtime::media::audio::decode`), so a backend offers its far end the
// codecs the platform decodes and hands the packets here rather than to its
// library's own decoder. A backend only writes what the far end sends, as it
// arrives, and never waits.

#ifndef VENTO_AUDIO_H
#define VENTO_AUDIO_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* Sample formats. Signed 16-bit, native-endian, interleaved: the format of
   QEMU's audio and RDP's PCM, and of the AAC and Opus decoders' output. */
#define VENTO_AUDIO_S16 1

/* Codecs, as `runtime::media::audio::decode::Codec` names them. AAC is raw
   access units, one frame per packet and no ADTS header, as RDP's sound
   sends it; Opus is one packet at a time, at 48 kHz. */
#define VENTO_AUDIO_CODEC_AAC 1
#define VENTO_AUDIO_CODEC_OPUS 2

/* What the session's sound switch does (`offer`): mute the far end's sound
   locally, where the protocol cannot ask the far end to stop sending it
   during a session (RDP's sound), or ask it to stop and start again (QEMU's
   audio extension). As `runtime::media::audio::Switch` names them. */
#define VENTO_AUDIO_SWITCH_MUTE 1
#define VENTO_AUDIO_SWITCH_ENABLE 2

/* One stream of the far end's sound. Not safe to call from two threads at
   once, but may be called from different threads in turn. */
typedef struct vento_audio_stream vento_audio_stream;

/* One decoder, for one stream's packets, with the same threading rules as
   a stream. */
typedef struct vento_audio_decoder vento_audio_decoder;

typedef struct vento_audio {
    /* Passed to `open`, `decodable`, `decoder_open` and `release`. */
    void *user;
    /* A stream of `format` samples at `rate` frames per second with
       `channels` channels (1 to 8), writable immediately whether or not its
       device has opened yet. NULL for an unsupported format (a rate outside
       8 to 384 kHz) or when the platform has no output. */
    vento_audio_stream *(*open)(void *user, int format, int rate, int channels);
    /* `len` bytes of samples, split anywhere: a partial frame at the end is
       kept for the next write. Never waits: the samples are queued and the
       device reads them on its own thread. */
    void (*write)(vento_audio_stream *s, const void *data, size_t len);
    /* The stream's own gain as a factor, 1 for unchanged, applied under the
       session's gain and mute (set by the user). For a far end that sets its
       own volume. */
    void (*set_gain)(vento_audio_stream *s, float gain);
    /* Milliseconds until a sample written now is heard: the queue plus the
       device's own buffer. RDP's sound reports this to its server. */
    int (*latency)(vento_audio_stream *s);
    /* Drops the queued samples and releases the device without waiting for
       it. */
    void (*close)(vento_audio_stream *s);
    /* Whether the platform decodes `codec`. A backend offers its far end only
       these codecs, and the far end sends only what it was offered. Cheap to
       call repeatedly. */
    bool (*decodable)(void *user, int codec);
    /* A decoder for `codec` packets of a stream at `rate` in `channels`, set
       up by `config` (AAC's AudioSpecificConfig, Opus's identification
       header) or, when `config_len` is 0, by `rate` and `channels` alone: AAC
       LC, and Opus with no pre-skip, since RDP's sound sends neither header.
       NULL when the platform cannot decode it; the reason is logged. */
    vento_audio_decoder *(*decoder_open)(void *user, int codec, int rate, int channels,
                                         const uint8_t *config, size_t config_len);
    /* One packet. Returns the number of frames decoded, with the samples
       (signed 16-bit, interleaved, as a stream takes them) in `*samples`,
       owned by the decoder until its next call, at `*rate` in `*channels`.
       The rate and channels come from the decoder rather than the stream (an
       AAC stream with SBR decodes at twice its rate, Opus always at 48 kHz).
       0 when the packet produced no frames yet; -1 when it could not be
       decoded, which drops that packet and leaves the decoder ready for the
       next. */
    int (*decode)(vento_audio_decoder *d, const uint8_t *data, size_t len,
                  const int16_t **samples, int *rate, int *channels);
    void (*decoder_close)(vento_audio_decoder *d);
    /* The far end may send sound from now on, so the session shows its sound
       switch. Called as early as the protocol allows (a QEMU server's audio
       acknowledgement, an RDP server's list of formats), and again whenever
       it changes. `sw` is VENTO_AUDIO_SWITCH_MUTE, with `enabled` true and no
       `enable`, when the far end cannot be asked to stop; or
       VENTO_AUDIO_SWITCH_ENABLE, with `enabled` saying whether sound is
       currently requested, and `enable(ctx, on)` called when the user turns
       the switch. `enable` runs on the UI thread (a window's), never inside
       a call into this table, so it passes the request to the protocol's
       thread, returns, and calls nothing here. */
    void (*offer)(void *user, int sw, bool enabled, void (*enable)(void *ctx, bool on),
                  void *ctx);
    /* The far end will send no more sound, or the session is ending: hides
       the switch. Once this returns, `enable` is not called again, so a
       backend withdraws before `ctx` goes away; `release` withdraws too. */
    void (*withdraw)(void *user);
    /* The backend owns the table from the moment it is handed over. Called
       once, after the last stream and decoder are closed, and also when the
       backend fails to start. */
    void (*release)(void *user);
} vento_audio;

#ifdef __cplusplus
}
#endif

#endif
