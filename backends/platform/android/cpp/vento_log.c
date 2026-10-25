// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

// `vento_log` for the phone: logcat, under the backend's tag. It is in C
// because it is variadic, which the Rust half linked beside it cannot
// implement, and it is the only C in this platform;
// `platform/linux/cpp/vento_log.c` is the stderr equivalent.

#include <stdarg.h>

#include <android/log.h>

#include <vento/log.h>

void vento_log(int level, const char *tag, const char *fmt, ...) {
    va_list ap;
    va_start(ap, fmt);
    __android_log_vprint(level == VENTO_LOG_WARN ? ANDROID_LOG_WARN : ANDROID_LOG_INFO, tag, fmt, ap);
    va_end(ap);
}
