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

/* How long a parked descriptor sits out. Long enough that a refused UDP
   socket costs a few polls rather than a core's worth, short enough that a
   real error on a real socket is seen almost at once. */
constexpr auto PARK = std::chrono::milliseconds(50);

thread_local Looper *tlLooper = nullptr;

uint32_t epollEvents(int events) {
    return (events & LOOPER_EVENT_INPUT ? EPOLLIN : 0u) | (events & LOOPER_EVENT_OUTPUT ? EPOLLOUT : 0u)
           | (events & LOOPER_EVENT_ERROR ? EPOLLERR : 0u)
           | (events & LOOPER_EVENT_HANGUP ? EPOLLHUP : 0u);
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
    ev.data.fd = wakefd_;
    epoll_ctl(epfd_, EPOLL_CTL_ADD, wakefd_, &ev);
}

int Looper::addFd(int fd, int ident, int events, ALooper_callbackFunc callback, void *data) {
    epoll_event ev{};
    ev.events = epollEvents(events);
    ev.data.fd = fd;
    const std::lock_guard<std::mutex> guard(lock_);
    bool exists = fds_.count(fd) != 0;
    fds_[fd] = {fd, ident, events, callback, data};
    if (parked_.erase(fd)) {
        exists = false; // was out of the set while parked
    }
    if (epoll_ctl(epfd_, exists ? EPOLL_CTL_MOD : EPOLL_CTL_ADD, fd, &ev) < 0) {
        LOGW("looper: epoll_ctl fd %d: %s", fd, strerror(errno));
        return -1;
    }
    return 1;
}

int Looper::removeFd(int fd) {
    const std::lock_guard<std::mutex> guard(lock_);
    if (!fds_.erase(fd)) {
        return 0;
    }
    if (!parked_.erase(fd)) {
        epoll_ctl(epfd_, EPOLL_CTL_DEL, fd, nullptr);
    }
    return 1;
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
            ev.data.fd = it->first;
            epoll_ctl(epfd_, EPOLL_CTL_ADD, it->first, &ev);
        }
        it = parked_.erase(it);
    }
}

void Looper::wake() {
    const uint64_t one = 1;
    if (write(wakefd_, &one, sizeof one) < 0) {
        // EAGAIN: the counter is already saturated, which is already awake.
    }
}

int Looper::pollOnce(int timeoutMs, int *outFd, int *outEvents, void **outData) {
    unparkDue();
    {
        const std::lock_guard<std::mutex> guard(lock_);
        if (!parked_.empty() && (timeoutMs < 0 || timeoutMs > 50)) {
            timeoutMs = 50; // somebody has to put them back
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
        const int fd = evs[i].data.fd;
        if (fd == wakefd_) {
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
            if (it == fds_.end()) {
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
            removeFd(fd);
        } else if (!(events & (LOOPER_EVENT_INPUT | LOOPER_EVENT_OUTPUT))) {
            const std::lock_guard<std::mutex> guard(lock_);
            if (fds_.count(fd) && !parked_.count(fd)
                    && epoll_ctl(epfd_, EPOLL_CTL_DEL, fd, nullptr) == 0) {
                parked_[fd] = std::chrono::steady_clock::now() + PARK;
            }
        }
    }
    return woken ? LOOPER_POLL_WAKE : LOOPER_POLL_CALLBACK;
}

// ---- the symbol table's ------------------------------------------------------

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

/* `pollAll` differs from `pollOnce` in repeating until something other than
   a callback happened; one call is as much as the core has needed. */
int looperPollAll(int timeoutMs, int *outFd, int *outEvents, void **outData) {
    return Looper::forThread()->pollOnce(timeoutMs, outFd, outEvents, outData);
}

} // namespace vento_realvnc
