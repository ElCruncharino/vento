// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

// LibVNCServer's libvncclient as a `vento_session`. The config is filled in
// from the backend's options by `backends/modules/libvnc`, the adapter the
// phone holds it through, and each field is explained below.

#ifndef VENTO_LIBVNC_H
#define VENTO_LIBVNC_H

#include <stdbool.h>

#include <vento/backend.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct vento_libvnc_config {
    /* `host`, `host:port`, `host:display`, `host::port`, `[literal]` or
       `[literal]:port`; a bare IPv6 literal is refused in words. */
    const char *address;
    const char *user_name;   /* NULL for none */
    const char *password;    /* NULL for none, which prompts */
    bool shared;
    /* Whether the VeNCrypt sub-types with no certificate in them may be used.
       They are always offered; a connection that says no here ends when one
       is agreed, before the tunnel is built, and says why. */
    bool anonymous_tls;
    const char *encoding;    /* a libvncclient encodings string, best first */
    int compress_level;      /* 0–9, or -1 for the server's own default */
    int quality_level;       /* 0–9 for Tight's JPEG, or -1 for lossless */
    int color_level;         /* 0 for 32-bit, 1 for 64 colours, 2 for 8 */
    int connect_timeout_ms;  /* 0 or less for the library's default */
} vento_libvnc_config;

/* "LibVNCServer x.y.z" — the one call that proves the whole path. */
const char *vento_libvnc_version(void);

/* Starts connecting and returns at once; everything after arrives through
   `callbacks`, which is copied. NULL out of memory, with nothing started. */
vento_session *vento_libvnc_create(const vento_libvnc_config *config,
                                   const vento_callbacks *callbacks);

/* Live: `SetEncodings` may be sent at any point in a session. */
void vento_libvnc_set_encodings(vento_session *s, const char *encoding, int compress_level,
                                int quality_level);

#ifdef __cplusplus
}
#endif

#endif
