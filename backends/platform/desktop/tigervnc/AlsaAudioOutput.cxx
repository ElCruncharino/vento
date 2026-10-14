// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

#include "AlsaAudioOutput.h"

#include <string.h>

#include <rfb/qemuTypes.h>

#include <vento/log.h>

#define TAG "TigerVnc"
#define LOGW(...) vento_log(VENTO_LOG_WARN, TAG, __VA_ARGS__)

namespace {

// What CConnection asks every server for. It keeps the three to itself, in
// CConnection.cxx, and their viewer restates them in its own header the same
// way; if theirs ever change, these have to follow.
constexpr uint8_t SAMPLE_FORMAT = rfb::qemuAudioFormatS16;
constexpr unsigned CHANNELS = 2;
constexpr unsigned FREQUENCY = 48000;

// A frame is one sample for each channel. Native-endian on both ends, which is
// what ALSA's plain S16 is.
constexpr size_t FRAME_BYTES = (size_t) CHANNELS << (SAMPLE_FORMAT >> 1);
static_assert(SAMPLE_FORMAT == rfb::qemuAudioFormatS16 && FRAME_BYTES == 4,
              "the device below is opened as 16-bit stereo");

// Their viewer's two numbers.
constexpr unsigned MAX_JITTER_MS = 1000;
constexpr unsigned MIN_STREAM_DELAY_MS = 20;

constexpr snd_pcm_uframes_t PREROLL_FRAMES = MIN_STREAM_DELAY_MS * FREQUENCY / 1000;

// What ALSA calls the device the sound server stands behind, on every
// desktop that has one.
constexpr const char *DEVICE = "default";

} // namespace

AudioOutput *vento_tigervnc_audio_output() {
    return new AlsaAudioOutput();
}

AlsaAudioOutput::~AlsaAudioOutput() {
    // The session is ending, so whatever is still queued goes with it.
    close();
}

bool AlsaAudioOutput::open() {
    if (pcm_ != nullptr) {
        return true;
    }
    if (failed_) {
        return false;
    }
    // Non-blocking from the open onwards: a device somebody else holds is a
    // refusal now rather than a wait, and every write below takes what fits.
    int err = snd_pcm_open(&pcm_, DEVICE, SND_PCM_STREAM_PLAYBACK, SND_PCM_NONBLOCK);
    if (err < 0) {
        pcm_ = nullptr;
        return fail("could not open an output device", err);
    }
    // The buffer is the whole second, which is a request: what the device
    // grants may be less, and that is then the line the jitter bound is drawn
    // at. Resampling is allowed so that a device without a 48 kHz mode is not
    // a device without sound.
    err = snd_pcm_set_params(pcm_, SND_PCM_FORMAT_S16, SND_PCM_ACCESS_RW_INTERLEAVED, CHANNELS,
                             FREQUENCY, 1, MAX_JITTER_MS * 1000);
    if (err < 0) {
        return fail("could not set the output format", err);
    }
    // set_params starts the device only once the buffer is nearly full,
    // which for a buffer of a second is a second of silence in front of
    // every stream. The pre-roll is where the stream should start instead:
    // 20 ms queued, and the device runs.
    snd_pcm_sw_params_t *sw;
    snd_pcm_sw_params_alloca(&sw);
    if ((err = snd_pcm_sw_params_current(pcm_, sw)) < 0
        || (err = snd_pcm_sw_params_set_start_threshold(pcm_, sw, PREROLL_FRAMES)) < 0
        || (err = snd_pcm_sw_params(pcm_, sw)) < 0) {
        return fail("could not set the start threshold", err);
    }
    return true;
}

void AlsaAudioOutput::close() {
    if (pcm_ != nullptr) {
        snd_pcm_close(pcm_);
        pcm_ = nullptr;
    }
    started_ = false;
}

bool AlsaAudioOutput::fail(const char *what, int why) {
    if (!failed_) {
        LOGW("audio: %s (%s); the server's sound is dropped", what, snd_strerror(why));
    }
    close();
    failed_ = true;
    return false;
}

void AlsaAudioOutput::start() {
    partialLength_ = 0;
    if (!open()) {
        return;
    }
    // A stream the server begins again before the last one has played out
    // carries straight on: the device is still running, and the pre-roll
    // goes in behind what is queued. One that ran dry — which is how every
    // stream ends here — is prepared again first.
    switch (snd_pcm_state(pcm_)) {
        case SND_PCM_STATE_RUNNING:
        case SND_PCM_STATE_PREPARED:
            break;
        default: {
            const int err = snd_pcm_prepare(pcm_);
            if (err < 0 && !fail("could not restart the output device", err)) {
                return;
            }
        }
    }
    started_ = true;
    preroll();
}

// Straight to the device rather than through write(): the silence is what
// write() recovers *with*, and must not itself recover.
void AlsaAudioOutput::preroll() {
    static const int16_t silence[PREROLL_FRAMES * CHANNELS] = {};
    snd_pcm_writei(pcm_, silence, PREROLL_FRAMES);
}

void AlsaAudioOutput::stop() {
    // What is queued plays out on its own, and the device stops itself when
    // it runs dry; see the header for why that is the drain.
    started_ = false;
}

void AlsaAudioOutput::play(const uint8_t *data, size_t length) {
    if (pcm_ == nullptr || !started_ || length == 0) {
        return;
    }
    if (partialLength_ > 0) {
        const size_t take = FRAME_BYTES - partialLength_ < length
                ? FRAME_BYTES - partialLength_ : length;
        memcpy(partial_ + partialLength_, data, take);
        partialLength_ += take;
        data += take;
        length -= take;
        if (partialLength_ < FRAME_BYTES) {
            return;
        }
        write(partial_, 1);
        partialLength_ = 0;
    }
    const size_t whole = length / FRAME_BYTES;
    if (whole > 0) {
        write(data, whole);
    }
    partialLength_ = length - whole * FRAME_BYTES;
    memcpy(partial_, data + whole * FRAME_BYTES, partialLength_);
}

void AlsaAudioOutput::write(const void *frames, snd_pcm_uframes_t count) {
    if (pcm_ == nullptr) {
        return;
    }
    // Non-blocking is what makes this safe on the protocol thread: it takes
    // what fits and returns, and a full buffer (-EAGAIN) is the jitter bound
    // dropping the samples that could never have been played in time.
    snd_pcm_sframes_t result = snd_pcm_writei(pcm_, frames, count);
    if (result == -EPIPE || result == -ESTRPIPE) {
        // The device ran dry mid-stream, or the machine slept. Prepared again
        // and the chunk goes in behind a fresh pre-roll, which is what keeps
        // it from being played straight into the next underrun. Once per
        // chunk at most, and a device that will not come back is given up on.
        const int err = snd_pcm_recover(pcm_, (int) result, 1);
        if (err < 0) {
            fail("the output device stopped working", err);
            return;
        }
        preroll();
        result = snd_pcm_writei(pcm_, frames, count);
    }
    if (result < 0 && result != -EAGAIN) {
        fail("the output device stopped working", (int) result);
    }
}
