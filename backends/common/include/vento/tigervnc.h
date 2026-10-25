// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

// TigerVNC's own client as a `vento_session`. The parameters are
// `TigerVncNative.nativeCreate`'s, in its order, and that class's javadoc is
// where each is explained. This is the one backend whose `question` callback
// fires: its certificate check is its own viewer's, known-hosts file and all.

#ifndef VENTO_TIGERVNC_H
#define VENTO_TIGERVNC_H

#include <stdbool.h>

#include <vento/backend.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct vento_tigervnc_config {
    /* As libvnc's: `host`, `host:port`, `host:display`, `host::port`,
       `[literal]` or `[literal]:port`. */
    const char *address;
    const char *user_name;   /* NULL for none */
    const char *password;    /* NULL for none, which prompts */
    bool shared;
    bool anonymous_tls;      /* as libvnc's */
    int encoding;            /* an RFB encoding number, the one to ask for first */
    int compress_level;      /* 0–9, or -1 for the server's own default */
    int quality_level;       /* 0–9, -1 for lossless, or -2 to follow the line speed */
    int color_level;         /* 0 full, 1 for 64 colours, 2 for 8, or -2 to follow */
    bool h264;               /* whether to offer H.264 at all */
    bool audio;              /* whether to ask for the server's sound, for the session */
    int connect_timeout_ms;  /* 0 or less for no deadline */
    /* Whether the caller keeps the pins. False (the phone): a certificate is
       checked the way TigerVNC's viewer checks it, against its known-hosts
       file under the state directory, and asked about through `question`.
       True: that file is neither read nor written, and a certificate that did
       not verify goes to `trust_needed` as its SHA-256 fingerprint, in `openssl
       x509 -fingerprint -sha256`'s form, as libvnc's does — the caller's store
       has the answer, or a person does. */
    bool caller_pins;
} vento_tigervnc_config;

/* "TigerVNC x.y.z" — the one call that proves the whole path. */
const char *vento_tigervnc_version(void);

/* Where the client may keep the certificates a person has accepted: the
   directory TigerVNC's own `tigervnc/` goes under, found the way its viewer
   finds it, through the environment. Process-wide, and before any session —
   the library reads it when the first certificate arrives. Not needed by a
   caller that sets `caller_pins`, for whom nothing is kept there. */
void vento_tigervnc_set_state_dir(const char *path);

/* Starts connecting and returns at once; everything after arrives through
   `callbacks`, which is copied. NULL where the session could not be set up. */
vento_session *vento_tigervnc_create(const vento_tigervnc_config *config,
                                     const vento_callbacks *callbacks);

/* Live, all of it: `SetEncodings` may be sent at any point in a session, and
   the pixel format is this client's to change safely. */
void vento_tigervnc_set_picture(vento_session *s, int encoding, int compress_level,
                                int quality_level, int color_level, bool h264);

#ifdef __cplusplus
}
#endif

#endif
