// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

// `vento_log` for a desktop process: one line on stderr, with the level and
// the tag in front, where the Android adapter has logcat. This is the whole
// of what the Linux platform has to be for a C backend's session file to
// link (`vento_linux`, in `cmake/vento_linux.cmake`); the rest of `cpp/` is
// the C the Rust decoders call, and the probe's body is in `probe/`.

#include <stdarg.h>
#include <stdbool.h>
#include <stdio.h>

#include <vento/log.h>

void vento_log(int level, const char *tag, const char *fmt, ...) {
    /* One write per line, so that two protocol threads logging at once do
       not interleave halfway through a line: stderr is unbuffered, so the
       line is formatted first and then written whole. */
    char line[1024];
    va_list ap;
    va_start(ap, fmt);
    int n = vsnprintf(line, sizeof(line), fmt, ap);
    va_end(ap);
    const bool cut = n >= (int) sizeof(line);
    if (cut) {
        n = (int) sizeof(line) - 1;
    }
    /* libvncclient's lines arrive with their newline on, as logcat would
       swallow; one line is one line here too. */
    while (n > 0 && line[n - 1] == '\n') {
        line[--n] = '\0';
    }
    fprintf(stderr, "%s %s: %s%s\n", level == VENTO_LOG_WARN ? "W" : "I", tag, line,
            cut ? "..." : "");
}
