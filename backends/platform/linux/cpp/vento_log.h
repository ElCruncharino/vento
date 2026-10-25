// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

// The stderr line writer behind `vento_log` on a desktop (`vento_log.c`), for
// the callers that have a whole line already: the Linux client's Rust logger
// and its FFmpeg log callback.

#ifndef VENTO_LINUX_LOG_H
#define VENTO_LINUX_LOG_H

#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

/* Writes `HH:MM:SS.mmm <level> <tag>: <text>` and a newline to stderr in one
   write, the time local and to the millisecond. `level` is the letter: `W`,
   `I`, `D` or `T`. Neither string needs a terminating NUL, and `text` is
   written as given, so it should hold no newline. */
void vento_log_line(char level, const char *tag, size_t tag_len, const char *text,
                    size_t text_len);

#ifdef __cplusplus
}
#endif

#endif
