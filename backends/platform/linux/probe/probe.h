// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

// A probe for a C backend: connect, print every callback as it arrives, answer
// the questions from the command line, poke the far end once, and after a
// while write the framebuffer to a file and exit. One body, written against
// the vtable in `vento/backend.h` alone, under a `main` per backend that parses
// that backend's configuration and passes its `create`: the same split as the
// Rust probes, where `runtime` is the body and a protocol crate is the `main`.
//
// A session that reaches `connected` through this has reached it through the
// neutral C API alone, with no Android under it, and the PPM it writes is the
// picture the phone would have drawn.

#ifndef VENTO_PROBE_H
#define VENTO_PROBE_H

#include <stdbool.h>

#include <vento/backend.h>

#ifdef __cplusplus
extern "C" {
#endif

/* The options every backend's probe takes, filled in from the command line
   by `vento_probe_option`. */
typedef struct vento_probe {
    int seconds;              /* --seconds N: how long to watch after `connected` */
    int connect_timeout_s;    /* --connect-timeout N: how long to wait for it */
    bool trust;               /* --trust: yes to every identity question */
    const char *user;         /* --user, --domain, --password: the answer to */
    const char *domain;       /*   credentials_needed; without a password the */
    const char *password;     /*   answer is a cancel */
    const char *clipboard;    /* --clipboard TEXT: sent a second after connecting */
    long keysym;              /* --key KEYSYM: pressed and released then, or -1 */
    int click_x, click_y;     /* --click X,Y: a left click there first, or -1 */
    const char *out;          /* --out FILE.ppm: the framebuffer; FILE-thumb.ppm */
    int thumb_step;           /* --thumb-step N: the thumbnail's 1/N */
} vento_probe;

/* The defaults: five seconds, no answers, no output. */
void vento_probe_init(vento_probe *p);

/* The common options, one per line, for a backend's usage text. */
extern const char vento_probe_usage[];

/* Whether `argv[*i]` is one of the common options; takes it, and whatever it
   needs after it, and moves `*i` past them. A malformed one is reported and
   exits. */
bool vento_probe_option(vento_probe *p, int argc, char **argv, int *i);

/* The value after an option that takes one, moving `*i` past both; a missing
   one is reported and exits. For the backends' own options. */
const char *vento_probe_value(int argc, char **argv, int *i);

/* The function a backend's `main` passes: its `vento_<name>_create`, taking
   its own configuration. */
typedef vento_session *(*vento_probe_create)(void *config, const vento_callbacks *callbacks);

/* Runs one session to its end and returns the process's exit status: zero
   where it connected and everything asked for was done. */
int vento_probe_run(const vento_probe *p, vento_probe_create create, void *config);

#ifdef __cplusplus
}
#endif

#endif
