// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

// The libvncclient backend's probe: `vento_libvnc_config` from the command
// line, over the body in `backends/platform/desktop/c`.
//
//   vento_libvnc_probe HOST:DISPLAY --password vncpass --out shot.ppm

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include <vento/libvnc.h>

#include "probe.h"

static vento_session *create(void *config, const vento_callbacks *callbacks) {
    return vento_libvnc_create(config, callbacks);
}

static void usage(void) {
    fprintf(stderr,
            "usage: vento_libvnc_probe ADDRESS [options]\n"
            "  ADDRESS              host, host:display, host::port, [literal]:port\n"
            "  --shared             do not disconnect other viewers\n"
            "  --anonymous-tls      allow VeNCrypt without a certificate\n"
            "  --encoding LIST      libvncclient encodings, best first\n"
            "  --compress N         0-9, or -1 for the server's default (-1)\n"
            "  --quality N          0-9 for JPEG, or -1 for lossless (-1)\n"
            "  --color N            0 full, 1 for 64 colours, 2 for 8 (0)\n"
            "  --timeout MS         connect timeout, or 0 for the library's (0)\n"
            "%s",
            vento_probe_usage);
    exit(2);
}

int main(int argc, char **argv) {
    vento_probe probe;
    vento_probe_init(&probe);
    vento_libvnc_config config = {
            .encoding = "tight zrle ultra copyrect hextile zlib corre rre raw",
            .compress_level = -1,
            .quality_level = -1,
    };
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
            config.encoding = vento_probe_value(argc, argv, &i);
        } else if (strcmp(a, "--compress") == 0) {
            config.compress_level = atoi(vento_probe_value(argc, argv, &i));
        } else if (strcmp(a, "--quality") == 0) {
            config.quality_level = atoi(vento_probe_value(argc, argv, &i));
        } else if (strcmp(a, "--color") == 0) {
            config.color_level = atoi(vento_probe_value(argc, argv, &i));
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
    printf("%s\n", vento_libvnc_version());
    return vento_probe_run(&probe, create, &config);
}
