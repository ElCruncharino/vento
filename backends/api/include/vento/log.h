// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

// Logging for the C session files, implemented by the platform: logcat on
// Android, stderr on a desktop. The session files log only through this so
// that none of them is platform-specific. The library loggers that need
// bridging (libvncclient's two hooks, TigerVNC's `core::Logger`) are bridged
// onto this in the session files.

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
   name; the session scripts filter the log by it. */
void vento_log(int level, const char *tag, const char *fmt, ...)
#if defined(__GNUC__) || defined(__clang__)
        __attribute__((format(printf, 3, 4)))
#endif
        ;

#ifdef __cplusplus
}
#endif

#endif
