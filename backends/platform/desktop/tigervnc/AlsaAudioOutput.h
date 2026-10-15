// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

// The far end's sound, played on a desktop: the sink for TigerVNC's QEMU audio
// extension on ALSA, behind the shim's own `AudioOutput`, in the shape of the
// AAudio one under `backends/platform/android/tigervnc`.
//
// ALSA rather than PulseAudio, which is what their viewer plays through, for
// the one property the Android sink is built around: a write that takes what
// fits and returns. The caller is the protocol thread, and a sink that blocked
// it would stall the picture behind the sound. libpulse-simple has only
// blocking calls — a write that waits for room and a drain that waits for
// silence — and the full `pa_stream` API that does not is their viewer's own
// four hundred lines, copied. A PCM opened `SND_PCM_NONBLOCK` is the same
// contract AAudio's write with a timeout of zero is, and on a desktop of this
// decade "default" is PipeWire's or PulseAudio's own ALSA plugin anyway, so
// the sound server is still where it ends up.
//
// So the policy is the Android sink's, numbers included — at most a second of
// audio queued ahead, and 20 ms of silence in front of every stream — and the
// queue is the device's own buffer, sized to that second. A write puts in what
// fits and the rest is dropped on the floor, which is the jitter bound doing
// its job. There is no drain: a drain that waits is the one call that would
// block the protocol thread, and ALSA's non-blocking drain is a promise its
// plugins do not all keep, so a stopped stream is left to play itself out and
// the underrun at its end is the stop. Opened on the first stream the server
// begins rather than with the session, since most servers never send any, and
// opened on a thread of its own: `snd_pcm_open` on a sound server's plugin is
// a conversation with the server, 20 to 110 ms of it on PipeWire, which on the
// protocol thread was that long without a picture. What the server sends
// before the device is ready is dropped, as a full buffer's overflow is. A
// device that will not open is said once in the log, and everything after it
// is swallowed: sound is not worth ending a session over.

#ifndef REMOTEDESKTOP_ALSAAUDIOOUTPUT_H
#define REMOTEDESKTOP_ALSAAUDIOOUTPUT_H

#include <alsa/asoundlib.h>

#include <stddef.h>
#include <stdint.h>

#include <memory>

#include "AudioOutput.h"

class AlsaAudioOutput : public AudioOutput {
public:
    AlsaAudioOutput() = default;

    ~AlsaAudioOutput() override;

    AlsaAudioOutput(const AlsaAudioOutput &) = delete;

    AlsaAudioOutput &operator=(const AlsaAudioOutput &) = delete;

    void start() override;

    void stop() override;

    void play(const uint8_t *data, size_t length) override;

private:
    /** What the opening thread and the sink share; see the .cxx. */
    struct Opening;

    /** The device is open and configured, asking the opening thread once it
        has been asked for; starts that thread on the first call. */
    bool ready();

    /** The stream's start on the device: prepared if need be, pre-rolled. */
    void prime();

    /** `snd_pcm_recover` without its wait: -EAGAIN while the device is still
        coming back from a suspend. */
    int recover(int why);

    void close();

    /** Once, with the reason; false, and silence from then on. */
    bool fail(const char *what, int why);

    void preroll();

    /** Frames, not bytes; what does not fit is dropped. */
    void write(const void *frames, snd_pcm_uframes_t count);

    std::shared_ptr<Opening> opening_;
    snd_pcm_t *pcm_ = nullptr;
    bool failed_ = false;
    bool started_ = false;
    // The stream the server began has been started on the device, which
    // waits for the device to be ready.
    bool primed_ = false;

    // A chunk can end partway through a frame, and the next one then starts
    // partway through it too. Dropping the stub would shift every channel
    // after it, so it is carried over instead.
    uint8_t partial_[4] = {};
    size_t partialLength_ = 0;
};

#endif
