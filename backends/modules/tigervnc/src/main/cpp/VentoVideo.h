// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

// What the session and its H.264 decoder contexts share, and the one thing
// the contexts need of the framebuffer they are handed.
//
// TigerVNC makes a decoder context per H.264 rectangle with nothing but the
// rectangle (`H264DecoderContext::createContext`, static, on a worker
// thread), so a context cannot be told which session it decodes for. What
// it is handed, at every decode, is the framebuffer — the session's own
// class — and that is where a context finds the platform's decoders, a cap
// on how many contexts may hold a hardware decoder, and somewhere to leave
// what the facts and the overlay say about the last picture.

#ifndef VENTO_TIGERVNC_VIDEO_H
#define VENTO_TIGERVNC_VIDEO_H

#include <stdint.h>
#include <string.h>

#include <atomic>
#include <mutex>
#include <string>

#include <rfb/PixelBuffer.h>

#include <vento/video.h>

namespace vento {

/** One session's video decoding: the platform's table, and what is known
    about the picture it last decoded. Every member is safe from any thread. */
class Video {
public:
    /** TigerVNC keeps up to 64 contexts, one per rectangle a server encodes on
        its own, and a GPU has a handful of decode sessions to give before it
        refuses or slows everything down. A server sends one rectangle per
        screen, so four covers any desktop that is not a wall of screens;
        a context past them decodes in software. */
    static constexpr int MAX_HARDWARE = 4;

    /** A copy of `table`, which the session owns from here: NULL for none. */
    explicit Video(const vento_video_decoders *table) {
        if (table != nullptr && table->open != nullptr && table->decode != nullptr
                && table->close != nullptr) {
            table_ = *table;
            has_ = true;
        } else if (table != nullptr && table->release != nullptr) {
            table->release(table->user);
        }
    }

    /** After every context is gone, which is after the connection is. */
    ~Video() {
        if (has_ && table_.release != nullptr) {
            table_.release(table_.user);
        }
    }

    Video(const Video &) = delete;
    Video &operator=(const Video &) = delete;

    bool available() const { return has_; }

    const vento_video_decoders &table() const { return table_; }

    /** How a decoder opened now would decode, as the header numbers it. */
    int mode() const { return has_ && table_.mode != nullptr ? table_.mode(table_.user) : 0; }

    /** One of the hardware places, or false where they are all taken. */
    bool claimHardware() {
        int held = hardwareHeld_.load();
        while (held < MAX_HARDWARE) {
            if (hardwareHeld_.compare_exchange_weak(held, held + 1)) {
                return true;
            }
        }
        return false;
    }

    void releaseHardware() { hardwareHeld_.fetch_sub(1); }

    /** From the context that just decoded a picture: whether it was
        hardware (1, 0, or -1 unknown), and its status line where it was
        asked for one this time. */
    void report(int hardware, const char *status) {
        hardware_ = hardware;
        if (status != nullptr) {
            const std::lock_guard<std::mutex> guard(lock_);
            status_ = status;
        }
    }

    /** As `report` last said: -1 before any picture. */
    int hardware() const { return hardware_; }

    bool status(char *out, size_t len) const {
        const std::lock_guard<std::mutex> guard(lock_);
        if (status_.empty() || out == nullptr || len < 2) {
            return false;
        }
        const size_t n = status_.size() < len - 1 ? status_.size() : len - 1;
        memcpy(out, status_.data(), n);
        out[n] = '\0';
        return true;
    }

    /** A context's decoder has lost the pictures the stream refers back to:
        the protocol thread asks for a whole new picture at the end of the
        update, which is the nearest thing RFB has to a key frame request. */
    void askRefresh() { refresh_ = true; }

    bool takeRefresh() { return refresh_.exchange(false); }

private:
    vento_video_decoders table_{};
    bool has_ = false;
    std::atomic<int> hardwareHeld_{0};
    std::atomic<int> hardware_{-1};
    mutable std::mutex lock_;
    std::string status_;
    std::atomic<bool> refresh_{false};
};

/**
 * The session's framebuffer, as far as an H.264 context is concerned: the
 * session's `Video`, its pixels as one block, and a count of the writes into
 * it, which is how a context that decodes straight into it knows whether
 * anything but itself has drawn there since its last picture.
 */
class VideoPixelBuffer : public rfb::ManagedPixelBuffer {
public:
    VideoPixelBuffer(const rfb::PixelFormat &pf, int w, int h, Video *video)
            : rfb::ManagedPixelBuffer(pf, w, h), video_(video), id_(nextId()) {}

    Video *video() const { return video_; }

    /** Never the same for two buffers, so that a count read from one is not
        mistaken for one of the next. */
    uint64_t id() const { return id_; }

    /** How many writes have been committed. */
    uint64_t writes() const { return writes_.load(); }

    /** The whole picture, rows `stride` pixels apart: one allocation, which
        is what lets a context whose rectangle is the whole screen decode
        into it directly. Every write is still committed rectangle by
        rectangle, through commitBufferRW. */
    uint8_t *pixels(int *stride) {
        return rfb::ManagedPixelBuffer::getBufferRW(getRect(), stride);
    }

    void commitBufferRW(const core::Rect &r) override {
        rfb::ManagedPixelBuffer::commitBufferRW(r);
        writes_.fetch_add(1);
    }

private:
    static uint64_t nextId() {
        static std::atomic<uint64_t> next{1};
        return next.fetch_add(1);
    }

    Video *const video_;
    const uint64_t id_;
    std::atomic<uint64_t> writes_{0};
};

} // namespace vento

#endif
