// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

// The text rules the session files share: what to make of bytes a server
// chose, and a clipboard's line ends and size. Header-only, since the neutral
// layer has no library of its own in C.

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

/* The most of a clipboard a session takes from the far end or offers it, in
   bytes of its text as UTF-8 with LF line ends: `common::MAX_CLIPBOARD`,
   which the Rust side holds this equal to (`session::c`). Past it a clipboard
   is skipped whole rather than cut short. */
#define VENTO_MAX_CLIPBOARD (1 << 20)

/* CRLF, and a lone CR, to LF, in place: what text is on this side of the
   seam, from a protocol whose far end writes Windows' line ends. Never
   longer than it was. `common::text::to_lf`, held to the same cases. */
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

/* Every line end, whichever it was, to CRLF, for a protocol whose far end
   expects Windows' line ends on its clipboard. Bytes of UTF-8 are never CR
   or LF but as themselves, so this works byte by byte. Returns a copy the
   caller frees, or NULL out of memory. `common::text::to_crlf`. */
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
