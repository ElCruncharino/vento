// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

// The FreeRDP backend's probe: `vento_freerdp_config` from the command
// line, over the body in `backends/platform/desktop/c`.
//
//   vento_freerdp_probe HOST --trust --user proto --password protopass --out shot.ppm

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>

#include <vento/freerdp.h>

#include "probe.h"

static vento_session *create(void *config, const vento_callbacks *callbacks) {
    return vento_freerdp_create(config, callbacks);
}

static void usage(void) {
    fprintf(stderr,
            "usage: vento_freerdp_probe ADDRESS [options]\n"
            "  ADDRESS              host, host:port, [literal]:port\n"
            "  --nla MODE           prefer, require or off (prefer)\n"
            "  --legacy-tls         TLS down to 1.0 at security level 0\n"
            "  --no-compression     bulk compression off\n"
            "  --graphics MODE      gfx, gfx-novideo, rfx or bitmap (gfx)\n"
            "  --experience MODE    full, balanced or plain (balanced)\n"
            "  --sound MODE         off, local or remote (off)\n"
            "  --scale N            100, 140 or 180 (100)\n"
            "  --size WxH           the desktop asked for, per monitor (1280x800)\n"
            "  --monitors N         how many (1)\n"
            "  --layout N           the keyboard layout id, or 0 for the library's (0)\n"
            "  --config-dir DIR     a writable directory the library wants (nothing\n"
            "                       is kept there), which becomes $HOME\n"
            "                       (vento-probe-freerdp, here)\n"
            "  --timeout MS         connect timeout, or 0 for the library's (0)\n"
            "%s",
            vento_probe_usage);
    exit(2);
}

int main(int argc, char **argv) {
    vento_probe probe;
    vento_probe_init(&probe);
    vento_freerdp_config config = {
            .nla = "prefer",
            .compression = true,
            .graphics = "gfx",
            .experience = "balanced",
            .sound = "off",
            .scale = 100,
            .width = 1280,
            .height = 800,
            .monitors = 1,
            .client_name = "vento-probe",
            .platform = "linux",
            .config_path = "vento-probe-freerdp",
    };
    for (int i = 1; i < argc;) {
        const char *a = argv[i];
        if (vento_probe_option(&probe, argc, argv, &i)) {
            continue;
        } else if (strcmp(a, "--nla") == 0) {
            config.nla = vento_probe_value(argc, argv, &i);
        } else if (strcmp(a, "--legacy-tls") == 0) {
            config.legacy_tls = true;
            i++;
        } else if (strcmp(a, "--no-compression") == 0) {
            config.compression = false;
            i++;
        } else if (strcmp(a, "--graphics") == 0) {
            config.graphics = vento_probe_value(argc, argv, &i);
        } else if (strcmp(a, "--experience") == 0) {
            config.experience = vento_probe_value(argc, argv, &i);
        } else if (strcmp(a, "--sound") == 0) {
            config.sound = vento_probe_value(argc, argv, &i);
        } else if (strcmp(a, "--scale") == 0) {
            config.scale = atoi(vento_probe_value(argc, argv, &i));
        } else if (strcmp(a, "--size") == 0) {
            if (sscanf(vento_probe_value(argc, argv, &i), "%dx%d", &config.width,
                       &config.height) != 2) {
                usage();
            }
        } else if (strcmp(a, "--monitors") == 0) {
            config.monitors = atoi(vento_probe_value(argc, argv, &i));
        } else if (strcmp(a, "--layout") == 0) {
            config.keyboard_layout = atoi(vento_probe_value(argc, argv, &i));
        } else if (strcmp(a, "--config-dir") == 0) {
            config.config_path = vento_probe_value(argc, argv, &i);
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
    /* The store has to exist: the library writes known hosts under it, and
       a directory it cannot write is a connection that fails as a TLS error. */
    mkdir(config.config_path, 0700);
    /* The user name and the password are asked for rather than given, so
       that the probe exercises the question every real session asks. */
    printf("%s\n", vento_freerdp_version());
    return vento_probe_run(&probe, create, &config);
}
