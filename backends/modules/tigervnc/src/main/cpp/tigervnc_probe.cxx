// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

// The TigerVNC backend's probe: `vento_tigervnc_config` from the command
// line, over the body in `backends/platform/desktop/c`. C++
// only so that the link is: the body and this are C, the library behind
// them is not.
//
//   vento_tigervnc_probe HOST:DISPLAY --password vncpass --out shot.ppm
//
// H.264 is not among what it can show: the session decodes it with the
// platform's decoders, handed over by a Rust caller, and this is C with none
// to hand, so `--h264` is passed on and the session declines to offer it.
// The Linux client's `vento tigervnc ... --probe` is the probe for H.264.

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include <vento/tigervnc.h>

#include "probe.h"

namespace {

vento_session *create(void *config, const vento_callbacks *callbacks) {
    return vento_tigervnc_create(static_cast<vento_tigervnc_config *>(config), callbacks);
}

[[noreturn]] void usage() {
    fprintf(stderr,
            "usage: vento_tigervnc_probe ADDRESS [options]\n"
            "  ADDRESS              host, host:display, host::port, [literal]:port\n"
            "  --shared             do not disconnect other viewers\n"
            "  --anonymous-tls      allow VeNCrypt without a certificate\n"
            "  --encoding N         the RFB encoding to ask for first (7, Tight)\n"
            "  --compress N         0-9, or -1 for the server's default (-1)\n"
            "  --quality N          0-9 for JPEG, -1 lossless, -2 by line speed (-1)\n"
            "  --color N            0 full, 1 for 64 colours, 2 for 8, -2 by speed (0)\n"
            "  --h264               offer H.264 as well (declined here: no decoders)\n"
            "  --audio              ask for the server's sound\n"
            "  --timeout MS         connect timeout, or 0 for none (0)\n"
            "%s",
            vento_probe_usage);
    exit(2);
}

} // namespace

int main(int argc, char **argv) {
    vento_probe probe;
    vento_probe_init(&probe);
    vento_tigervnc_config config = {};
    config.encoding = 7; // Tight
    config.compress_level = -1;
    config.quality_level = -1;
    for (int i = 1; i < argc;) {
        const char *a = argv[i];
        if (vento_probe_option(&probe, argc, argv, &i)) {
            continue;
        } else if (strcmp(a, "--shared") == 0) {
            config.shared = true;
            i++;
        } else if (strcmp(a, "--anonymous-tls") == 0) {
            config.anonymous_tls = true;
            i++;
        } else if (strcmp(a, "--encoding") == 0) {
            config.encoding = atoi(vento_probe_value(argc, argv, &i));
        } else if (strcmp(a, "--compress") == 0) {
            config.compress_level = atoi(vento_probe_value(argc, argv, &i));
        } else if (strcmp(a, "--quality") == 0) {
            config.quality_level = atoi(vento_probe_value(argc, argv, &i));
        } else if (strcmp(a, "--color") == 0) {
            config.color_level = atoi(vento_probe_value(argc, argv, &i));
        } else if (strcmp(a, "--h264") == 0) {
            config.h264 = true;
            i++;
        } else if (strcmp(a, "--audio") == 0) {
            config.audio = true;
            i++;
        } else if (strcmp(a, "--timeout") == 0) {
            config.connect_timeout_ms = atoi(vento_probe_value(argc, argv, &i));
        } else if (a[0] != '-' && !config.address) {
            config.address = a;
            i++;
        } else {
            usage();
        }
    }
    if (!config.address) {
        usage();
    }
    /* The user name and the password are asked for rather than given, so
       that the probe exercises the question every real session asks. */
    printf("%s\n", vento_tigervnc_version());
    return vento_probe_run(&probe, create, &config);
}
