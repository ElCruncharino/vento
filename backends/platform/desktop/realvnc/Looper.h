// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

// `ALooper`, which is what the core registers its sockets and timers on, as
// an epoll set per thread. The pieces of `libandroid.so` the core imports are
// the six functions below; `pollOnce` is also what the session's thread pumps
// between its own tasks, and `wake` is how a task posted from another thread
// makes that pump return.

#ifndef VENTO_REALVNC_LOOPER_H
#define VENTO_REALVNC_LOOPER_H

#include <chrono>
#include <map>
#include <mutex>

namespace vento_realvnc {

typedef int (*ALooper_callbackFunc)(int fd, int events, void *data);

/* The `ALOOPER_EVENT_*` bits and `ALOOPER_POLL_*` results, as the NDK
   numbers them. */
enum {
    LOOPER_EVENT_INPUT = 1,
    LOOPER_EVENT_OUTPUT = 2,
    LOOPER_EVENT_ERROR = 4,
    LOOPER_EVENT_HANGUP = 8,
    LOOPER_POLL_WAKE = -1,
    LOOPER_POLL_CALLBACK = -2,
    LOOPER_POLL_TIMEOUT = -3,
    LOOPER_POLL_ERROR = -4,
};

class Looper {
public:
    /* This thread's, made on first use, as `ALooper_prepare` makes one. */
    static Looper *forThread();

    int addFd(int fd, int ident, int events, ALooper_callbackFunc callback, void *data);
    int removeFd(int fd);
    int pollOnce(int timeoutMs, int *outFd, int *outEvents, void **outData);

    /* From any thread: the next or current `pollOnce` returns
       `LOOPER_POLL_WAKE`. */
    void wake();

private:
    Looper();

    struct Entry {
        int fd, ident, events;
        ALooper_callbackFunc callback;
        void *data;
    };

    void unparkDue();

    int epfd_;
    int wakefd_;
    std::mutex lock_;
    std::map<int, Entry> fds_;
    /* Descriptors that reported only an error or hangup and were kept by
       their callback. Level-triggered epoll would return them at once, for
       ever — a UDP socket with a pending ECONNREFUSED does this, and spun the
       loop a million times a second until the core's 750 ms TCP fallback
       fired — so they are taken out of the set and put back after a moment. */
    std::map<int, std::chrono::steady_clock::time_point> parked_;
};

/* The `libandroid.so` symbol table's entries. */
void *looperForThread();
void *looperPrepare(int opts);
int looperAddFd(void *looper, int fd, int ident, int events, ALooper_callbackFunc callback,
                void *data);
int looperRemoveFd(void *looper, int fd);
int looperPollOnce(int timeoutMs, int *outFd, int *outEvents, void **outData);
int looperPollAll(int timeoutMs, int *outFd, int *outEvents, void **outData);

} // namespace vento_realvnc

#endif
