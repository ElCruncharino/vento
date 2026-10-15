// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

// `vento_log` for the phone: logcat, under the backend's tag. The one thing a
// C backend's session file asks of Android that the Rust half linked beside
// it cannot answer, because it is variadic — so it is C, and it is all of the
// C this platform has; `platform/desktop/c/vento_log.c` is its twin on stderr.

#include <stdarg.h>

#include <android/log.h>

#include <vento/log.h>

void vento_log(int level, const char *tag, const char *fmt, ...) {
    va_list ap;
    va_start(ap, fmt);
    __android_log_vprint(level == VENTO_LOG_WARN ? ANDROID_LOG_WARN : ANDROID_LOG_INFO, tag, fmt, ap);
    va_end(ap);
}
