// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

// The platform's sound output, as a C backend is handed it: a table of calls
// over `common::audio::Outputs` (AAudio on the phone, ALSA on the Linux
// client), filled in by `session::c::audio` and passed in a backend's config.
// Function pointers rather than symbols, for the reason `vento/video.h` gives:
// a backend library links nothing of the process it is loaded into.
//
// What is behind a stream is the same for every backend and both platforms:
// about a tenth of a second queued ahead of the device and never more than a
// quarter (past that the oldest is dropped, so a burst or a stall costs a
// skip rather than a lag that grows), silence while it fills back up after
// running dry, the session's gain and mute, and a device opened off the
// caller's thread. So a backend only writes what the far end sends, as it
// arrives, and never waits.

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

/* One stream of the far end's sound. Not safe to call into from two threads
   at once; from one thread after another is fine. */
typedef struct vento_audio_stream vento_audio_stream;

typedef struct vento_audio {
    /* The output itself, handed to `open` and `release`. */
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
    /* The table is the backend's from the moment it is handed over: this is
       called once, after the last stream is closed, and also when the
       backend fails to start at all. */
    void (*release)(void *user);
} vento_audio;

#ifdef __cplusplus
}
#endif

#endif
