// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

// The one text rule the session files share: what to make of bytes a server
// chose. Header-only, since the neutral layer has no library of its own in C.

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

/* Whether `bytes` decodes as UTF-8 all the way to its terminator. Only
   answerable by decoding it, which this does and throws away. */
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

/* A string a *server* chose, as UTF-8: valid UTF-8 as itself, and anything
   else as Latin-1, which is the rule TigerVNC's reader applies to the desktop
   name — RFB 3.8 says that field is Latin-1 and every server written since
   sends UTF-8, so the bytes alone have to say which they are. A server's own
   bytes are whatever it decided, and a name that is not UTF-8 is still a
   name. Returns a copy the caller frees, or NULL out of memory. */
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
    /* One code point a byte, which is at most two bytes of UTF-8 each. */
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

#ifdef __cplusplus
}
#endif

#endif
