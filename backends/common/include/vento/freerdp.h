// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

// FreeRDP's own client as a `vento_session`. The config is filled in from the
// backend's options by `backends/modules/freerdp/src/main/rust`, the adapter
// both clients hold it through, and each field is explained below. Its
// credentials carry a domain, which is the one
// place the shape of what is sent *back* differs between the protocols.

#ifndef VENTO_FREERDP_H
#define VENTO_FREERDP_H

#include <stdbool.h>

#include <vento/backend.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct vento_freerdp_config {
    /* `host`, `host:port`, `[literal]` or `[literal]:port`; no display
       numbers, which RDP has never had. */
    const char *address;
    const char *user_name;   /* NULL for none */
    const char *domain;      /* NULL for none */
    const char *password;    /* NULL for none, which prompts */
    const char *nla;         /* "prefer", "require" or "off" */
    bool legacy_tls;         /* TLS down to 1.0 at OpenSSL's security level 0 */
    bool compression;
    const char *graphics;    /* "gfx", "gfx-novideo", "rfx" or "bitmap" */
    const char *experience;  /* "full", "balanced" or "plain" */
    const char *sound;       /* "off", "local" or "remote" */
    int scale;               /* 100, 140 or 180; anything else is taken as 100 */
    int width, height;       /* the desktop asked for, per monitor */
    int monitors;
    int keyboard_layout;
    const char *client_name;
    /* A writable directory: the library sets its certificate store up there,
       and fails to connect without one, though the store is never read or
       written — every certificate goes to `trust_needed` and nowhere else.
       Also becomes the process's HOME, which WinPR reads and an app has none
       of. */
    const char *config_path;
    int connect_timeout_ms;  /* 0 or less for the library's default */
} vento_freerdp_config;

/* "FreeRDP x.y.z" — the one call that proves the whole path. */
const char *vento_freerdp_version(void);

/* Starts connecting and returns at once; everything after arrives through
   `callbacks`, which is copied. NULL where the session could not be set up. */
vento_session *vento_freerdp_create(const vento_freerdp_config *config,
                                    const vento_callbacks *callbacks);

/* How many monitors the *next* layout asks for; a desktop somebody is working
   on is not rearranged as a side effect of a settings row moving. */
void vento_freerdp_set_monitor_count(vento_session *s, int count);

#ifdef __cplusplus
}
#endif

#endif
