// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

// Text helpers shared by the session files: decoding strings chosen by a
// server, and clipboard line ends and size. Header-only, since the shared
// layer has no C library of its own.

#ifndef VENTO_TEXT_H
#define VENTO_TEXT_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

#ifdef __cplusplus
extern "C" {
#endif

/* Whether `bytes` is valid UTF-8 up to its terminator. */
static inline bool vento_utf8_valid(const char *bytes) {
    const size_t n = strlen(bytes);
    for (size_t i = 0; i < n;) {
        const uint8_t c = (uint8_t) bytes[i];
        int extra;
        if (c < 0x80) { extra = 0; }
        else if ((c & 0xE0) == 0xC0) { extra = 1; }
        else if ((c & 0xF0) == 0xE0) { extra = 2; }
        else if ((c & 0xF8) == 0xF0) { extra = 3; }
        else { return false; }
        if (i + (size_t) extra >= n) {
            return false;
        }
        for (int k = 1; k <= extra; k++) {
            if (((uint8_t) bytes[i + (size_t) k] & 0xC0) != 0x80) {
                return false;
            }
        }
        i += (size_t) extra + 1;
    }
    return true;
}

/* A string chosen by a server, as UTF-8: valid UTF-8 is copied as is, and
   anything else is read as Latin-1. TigerVNC's reader applies the same rule
   to the desktop name: RFB 3.8 says that field is Latin-1, but servers
   written since send UTF-8, so only the bytes can tell which it is. Returns
   a copy the caller frees, or NULL when out of memory. */
static inline char *vento_utf8_from_bytes(const char *bytes) {
    if (!bytes) {
        bytes = "";
    }
    const size_t n = strlen(bytes);
    if (vento_utf8_valid(bytes)) {
        char *copy = (char *) malloc(n + 1);
        if (copy) {
            memcpy(copy, bytes, n + 1);
        }
        return copy;
    }
    /* One code point per byte, each at most two bytes of UTF-8. */
    char *out = (char *) malloc(2 * n + 1);
    if (!out) {
        return NULL;
    }
    size_t w = 0;
    for (size_t i = 0; i < n; i++) {
        const uint8_t c = (uint8_t) bytes[i];
        if (c < 0x80) {
            out[w++] = (char) c;
        } else {
            out[w++] = (char) (0xC0 | (c >> 6));
            out[w++] = (char) (0x80 | (c & 0x3F));
        }
    }
    out[w] = '\0';
    return out;
}

/* The largest clipboard a session takes from or offers to the far end, in
   bytes of UTF-8 text with LF line ends. Equal to `runtime::MAX_CLIPBOARD`,
   which `api::c` asserts. A larger clipboard is skipped whole rather than
   truncated. */
#define VENTO_MAX_CLIPBOARD (1 << 20)

/* Converts CRLF and lone CR to LF in place, for text from a far end that
   uses Windows line ends. The result is never longer than the input. Same
   as `runtime::text::to_lf`, and tested against the same cases. */
static inline void vento_text_to_lf(char *utf8) {
    if (!utf8) {
        return;
    }
    size_t w = 0;
    for (size_t r = 0; utf8[r]; r++) {
        if (utf8[r] == '\r') {
            utf8[w++] = '\n';
            if (utf8[r + 1] == '\n') {
                r++;
            }
        } else {
            utf8[w++] = utf8[r];
        }
    }
    utf8[w] = '\0';
}

/* Converts every line end to CRLF, for a far end that expects Windows line
   ends on its clipboard. A multi-byte UTF-8 sequence never contains a CR or
   LF byte, so this works byte by byte. Returns a copy the caller frees, or
   NULL when out of memory. Same as `runtime::text::to_crlf`. */
static inline char *vento_text_to_crlf(const char *utf8) {
    if (!utf8) {
        utf8 = "";
    }
    size_t ends = 0, n = 0;
    for (; utf8[n]; n++) {
        if (utf8[n] == '\n' || (utf8[n] == '\r' && utf8[n + 1] != '\n')) {
            ends++;
        }
    }
    /* At most one byte more for every line end, and the terminator. */
    char *out = (char *) malloc(n + ends + 1);
    if (!out) {
        return NULL;
    }
    size_t w = 0;
    for (size_t r = 0; r < n; r++) {
        if (utf8[r] == '\r' || utf8[r] == '\n') {
            if (utf8[r] == '\r' && utf8[r + 1] == '\n') {
                r++;
            }
            out[w++] = '\r';
            out[w++] = '\n';
        } else {
            out[w++] = utf8[r];
        }
    }
    out[w] = '\0';
    return out;
}

#ifdef __cplusplus
}
#endif

#endif
