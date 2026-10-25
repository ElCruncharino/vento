// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

#include "Looper.h"

#include <errno.h>
#include <stdint.h>
#include <string.h>
#include <sys/epoll.h>
#include <sys/eventfd.h>
#include <unistd.h>

#include "Host.h"

namespace vento_realvnc {

namespace {

/* How long a parked descriptor stays out of the epoll set. Long enough that
   a refused UDP socket costs a few polls rather than a whole CPU core, short
   enough that a real error on a real socket is seen almost at once. */
constexpr auto PARK = std::chrono::milliseconds(50);

thread_local Looper *tlLooper = nullptr;

uint32_t epollEvents(int events) {
    return (events & LOOPER_EVENT_INPUT ? EPOLLIN : 0u) | (events & LOOPER_EVENT_OUTPUT ? EPOLLOUT : 0u)
           | (events & LOOPER_EVENT_ERROR ? EPOLLERR : 0u)
           | (events & LOOPER_EVENT_HANGUP ? EPOLLHUP : 0u);
}

/* An event's data: the registration's number in the upper 32 bits, the
   descriptor in the lower. The wake descriptor's number is 0, which no
   registration has. */
uint64_t tag(int fd, uint32_t seq) {
    return ((uint64_t) seq << 32) | (uint32_t) fd;
}

int looperEvents(uint32_t events) {
    return (events & EPOLLIN ? LOOPER_EVENT_INPUT : 0) | (events & EPOLLOUT ? LOOPER_EVENT_OUTPUT : 0)
           | (events & EPOLLERR ? LOOPER_EVENT_ERROR : 0)
           | (events & EPOLLHUP ? LOOPER_EVENT_HANGUP : 0);
}

} // namespace

Looper *Looper::forThread() {
    if (!tlLooper) {
        tlLooper = new Looper;
    }
    return tlLooper;
}

Looper::Looper() : epfd_(epoll_create1(EPOLL_CLOEXEC)), wakefd_(eventfd(0, EFD_CLOEXEC | EFD_NONBLOCK)) {
    epoll_event ev{};
    ev.events = EPOLLIN;
    ev.data.u64 = tag(wakefd_, 0);
    epoll_ctl(epfd_, EPOLL_CTL_ADD, wakefd_, &ev);
}

int Looper::addFd(int fd, int ident, int events, ALooper_callbackFunc callback, void *data) {
    const std::lock_guard<std::mutex> guard(lock_);
    const uint32_t seq = nextSeq_++;
    if (nextSeq_ == 0) {
        nextSeq_ = 1;
    }
    epoll_event ev{};
    ev.events = epollEvents(events);
    ev.data.u64 = tag(fd, seq);
    bool exists = fds_.count(fd) != 0;
    if (parked_.erase(fd)) {
        exists = false; // was out of the set while parked
    }
    int r = epoll_ctl(epfd_, exists ? EPOLL_CTL_MOD : EPOLL_CTL_ADD, fd, &ev);
    if (r < 0 && exists && errno == ENOENT) {
        // As Android tolerates it: the descriptor registered under this number
        // was closed without being removed, which takes it out of the epoll
        // set, and this is a new one that happens to have its number.
        r = epoll_ctl(epfd_, EPOLL_CTL_ADD, fd, &ev);
    }
    if (r < 0) {
        LOGW("looper: epoll_ctl fd %d: %s", fd, strerror(errno));
        return -1;
    }
    fds_[fd] = {fd, ident, events, callback, data, seq};
    return 1;
}

int Looper::removeFd(int fd) {
    const std::lock_guard<std::mutex> guard(lock_);
    if (!fds_.erase(fd)) {
        return 0;
    }
    if (!parked_.erase(fd)) {
        // Fails harmlessly for a descriptor already closed.
        epoll_ctl(epfd_, EPOLL_CTL_DEL, fd, nullptr);
    }
    return 1;
}

void Looper::removeSeq(int fd, uint32_t seq) {
    {
        const std::lock_guard<std::mutex> guard(lock_);
        const auto it = fds_.find(fd);
        if (it == fds_.end() || it->second.seq != seq) {
            return; // re-registered by the callback, which is not a removal
        }
    }
    removeFd(fd);
}

void Looper::unparkDue() {
    const std::lock_guard<std::mutex> guard(lock_);
    const auto now = std::chrono::steady_clock::now();
    for (auto it = parked_.begin(); it != parked_.end();) {
        if (it->second > now) {
            ++it;
            continue;
        }
        const auto e = fds_.find(it->first);
        if (e != fds_.end()) {
            epoll_event ev{};
            ev.events = epollEvents(e->second.events);
            ev.data.u64 = tag(it->first, e->second.seq);
            epoll_ctl(epfd_, EPOLL_CTL_ADD, it->first, &ev);
        }
        it = parked_.erase(it);
    }
}

void Looper::wake() {
    const uint64_t one = 1;
    if (write(wakefd_, &one, sizeof one) < 0) {
        // EAGAIN: the counter is saturated, so the looper is already woken.
    }
}

int Looper::pollOnce(int timeoutMs, int *outFd, int *outEvents, void **outData) {
    unparkDue();
    {
        const std::lock_guard<std::mutex> guard(lock_);
        if (!parked_.empty() && (timeoutMs < 0 || timeoutMs > 50)) {
            timeoutMs = 50; // so that the parked descriptors are put back
        }
    }
    epoll_event evs[16];
    const int n = epoll_wait(epfd_, evs, 16, timeoutMs);
    if (n < 0) {
        return errno == EINTR ? LOOPER_POLL_WAKE : LOOPER_POLL_ERROR;
    }
    if (n == 0) {
        return LOOPER_POLL_TIMEOUT;
    }
    bool woken = false;
    for (int i = 0; i < n; i++) {
        const int fd = (int) (uint32_t) evs[i].data.u64;
        const uint32_t seq = (uint32_t) (evs[i].data.u64 >> 32);
        if (seq == 0 && fd == wakefd_) {
            uint64_t count;
            while (read(wakefd_, &count, sizeof count) > 0) {
            }
            woken = true;
            continue;
        }
        Entry e;
        {
            const std::lock_guard<std::mutex> guard(lock_);
            const auto it = fds_.find(fd);
            if (it == fds_.end() || it->second.seq != seq) {
                // Removed, or removed and its number given to another
                // descriptor, by a callback earlier in this batch: the event
                // belongs to the old registration and is dropped.
                continue;
            }
            e = it->second;
        }
        const int events = looperEvents(evs[i].events);
        if (!e.callback) {
            if (outFd) *outFd = fd;
            if (outEvents) *outEvents = events;
            if (outData) *outData = e.data;
            return e.ident;
        }
        if (e.callback(fd, events, e.data) == 0) {
            removeSeq(fd, seq);
        } else if (!(events & (LOOPER_EVENT_INPUT | LOOPER_EVENT_OUTPUT))) {
            const std::lock_guard<std::mutex> guard(lock_);
            const auto it = fds_.find(fd);
            if (it != fds_.end() && it->second.seq == seq && !parked_.count(fd)
                    && epoll_ctl(epfd_, EPOLL_CTL_DEL, fd, nullptr) == 0) {
                parked_[fd] = std::chrono::steady_clock::now() + PARK;
            }
        }
    }
    return woken ? LOOPER_POLL_WAKE : LOOPER_POLL_CALLBACK;
}

// ---- libandroid.so symbol table entries --------------------------------------

void *looperForThread() {
    return Looper::forThread();
}

void *looperPrepare(int /*opts*/) {
    return Looper::forThread();
}

int looperAddFd(void *looper, int fd, int ident, int events, ALooper_callbackFunc callback,
                void *data) {
    return static_cast<Looper *>(looper)->addFd(fd, ident, events, callback, data);
}

int looperRemoveFd(void *looper, int fd) {
    return static_cast<Looper *>(looper)->removeFd(fd);
}

int looperPollOnce(int timeoutMs, int *outFd, int *outEvents, void **outData) {
    return Looper::forThread()->pollOnce(timeoutMs, outFd, outEvents, outData);
}

/* `pollAll` differs from `pollOnce` by repeating until something other than
   a callback happens; a single call has been enough for the core. */
int looperPollAll(int timeoutMs, int *outFd, int *outEvents, void **outData) {
    return Looper::forThread()->pollOnce(timeoutMs, outFd, outEvents, outData);
}

} // namespace vento_realvnc
