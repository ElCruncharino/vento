// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

// Where the far end's sound goes: QEMU's audio extension, whose protocol half
// is TigerVNC's own (`CConnection` asks for it and hands over what arrives),
// played on the platform's output, which the session is handed in its config
// as `vento/audio.h`'s table — AAudio on the phone, a PipeWire stream (or
// ALSA's default device) on the Linux client, behind the one queue and policy every backend's sound
// shares (`media::audio`). Their viewer's own sinks, PulseAudio and waveOut
// behind an `AudioOutput` in `vncviewer/`, are not compiled here, and neither
// is a sink of this module's any more: how far ahead sound is kept, what is
// dropped when it gets too far ahead, and opening the device off this
// thread are all the table's.
//
// What CConnection asks every server for — 16-bit signed, stereo, 48 kHz — it
// keeps to itself in CConnection.cxx, and their viewer restates it in its own
// header the same way; it is restated below, beside the stream it opens.

#ifndef VENTO_TIGERVNC_AUDIO_H
#define VENTO_TIGERVNC_AUDIO_H

#include <stddef.h>
#include <stdint.h>

#include <vento/audio.h>
#include <vento/log.h>

namespace vento {

/** One session's sound: the platform's table, and the one stream a QEMU
    server's audio is played on. Every call but the destructor comes from the
    protocol thread, and none of them waits on the device. */
class Audio {
public:
    // What CConnection asks for (qemuAudioFormatS16, which is native-endian
    // on both ends and VENTO_AUDIO_S16 here), restated; if theirs ever
    // change, these have to follow.
    static constexpr int FORMAT = VENTO_AUDIO_S16;
    static constexpr int RATE = 48000;
    static constexpr int CHANNELS = 2;

    /** A copy of `table`, which the session owns from here and releases when
        it ends: NULL for none. A table it cannot use is released at once. */
    explicit Audio(const vento_audio *table) {
        if (table != nullptr && table->open != nullptr && table->write != nullptr
                && table->close != nullptr) {
            table_ = *table;
            has_ = true;
        } else if (table != nullptr && table->release != nullptr) {
            table->release(table->user);
        }
    }

    /** After the connection is gone, on whichever thread ends the session:
        the stream is closed first, as the header asks. */
    ~Audio() {
        close();
        if (has_ && table_.release != nullptr) {
            table_.release(table_.user);
        }
    }

    Audio(const Audio &) = delete;
    Audio &operator=(const Audio &) = delete;

    bool available() const { return has_; }

    bool isOpen() const { return stream_ != nullptr; }

    /** The stream, opened if it is not: the server has begun sending, or
        sends again after a pause closed it. A platform that will not open
        one is said once, and the server's sound is dropped from then on. */
    void open() {
        if (stream_ != nullptr || !has_ || failed_) {
            return;
        }
        stream_ = table_.open(table_.user, FORMAT, RATE, CHANNELS);
        if (stream_ == nullptr) {
            failed_ = true;
            vento_log(VENTO_LOG_WARN, "TigerVnc",
                      "audio: this platform has no sound output; the server's sound is dropped");
        }
    }

    /** Samples as the server sent them, cut anywhere: the table keeps a part
        of a frame for the next write. */
    void write(const uint8_t *data, size_t length) {
        open();
        if (stream_ != nullptr) {
            table_.write(stream_, data, length);
        }
    }

    /** Milliseconds until the last sample written is heard; 0 with no
        stream. */
    int tail() const {
        return stream_ != nullptr && table_.latency != nullptr ? table_.latency(stream_) : 0;
    }

    /** The session's sound switch, as an Enable switch: the server may send
        sound, and is asked for it (`on`) or not; `enable(ctx, on)` is called
        on a window's thread when the person turns it, until `withdraw`. */
    void offerEnable(bool on, void (*enable)(void *ctx, bool on), void *ctx) {
        if (has_ && table_.offer != nullptr) {
            table_.offer(table_.user, VENTO_AUDIO_SWITCH_ENABLE, on, enable, ctx);
        }
    }

    /** No switch any more; once this returns `enable` is not called again. */
    void withdraw() {
        if (has_ && table_.withdraw != nullptr) {
            table_.withdraw(table_.user);
        }
    }

    /** Drops what is queued and lets the device go, without waiting. */
    void close() {
        if (stream_ != nullptr) {
            table_.close(stream_);
            stream_ = nullptr;
        }
    }

private:
    vento_audio table_{};
    bool has_ = false;
    bool failed_ = false;
    vento_audio_stream *stream_ = nullptr;
};

} // namespace vento

#endif
