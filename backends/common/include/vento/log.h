// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

// Where a session file's log lines go, which is the platform's to say: logcat
// under the Android adapter, stderr under a desktop one. The session files
// log through this and nothing else, so that none of them names a platform —
// and the two library loggers that have to be bridged (libvncclient's two
// hooks, TigerVNC's `core::Logger`) are bridged in the session files, onto
// this.

#ifndef VENTO_LOG_H
#define VENTO_LOG_H

#ifdef __cplusplus
extern "C" {
#endif

enum {
    VENTO_LOG_INFO = 1,
    VENTO_LOG_WARN = 2,
};

/* Implemented by the platform, once per process. `tag` is the backend's
   name, which is what the session scripts filter the log by. */
void vento_log(int level, const char *tag, const char *fmt, ...)
#if defined(__GNUC__) || defined(__clang__)
        __attribute__((format(printf, 3, 4)))
#endif
        ;

#ifdef __cplusplus
}
#endif

#endif
