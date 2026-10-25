// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

// The RealVNC backend's probe: `vento_realvnc_config` from the command line,
// over the body in `backends/platform/linux/probe`. The libraries are not in
// this repository; `--libs` points at a copy extracted from the viewer's
// APK, and the session refuses any build other than the pinned one.
//
//   vento_realvnc_probe HOST::PORT --libs DIR --state-dir DIR --trust --password vncpass --out shot.ppm

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include <vento/realvnc.h>

#include "probe.h"

namespace {

vento_session *create(void *config, const vento_callbacks *callbacks) {
    return vento_realvnc_create(static_cast<vento_realvnc_config *>(config), callbacks);
}

[[noreturn]] void usage() {
    fprintf(stderr,
            "usage: vento_realvnc_probe ADDRESS --libs DIR [options]\n"
            "  ADDRESS              host, host:display, host::port\n"
            "  --libs DIR           where libvncviewer.so and libc++_shared.so are\n"
            "  --state-dir DIR      where the core keeps its key, identities and logs\n"
            "                       (build/scratch/realvnc-state)\n"
            "  --quality Q          Auto, High, Medium, Low or Custom (Auto)\n"
            "  --encoding E         the preferred encoding, read at Custom (ZRLE2)\n"
            "  --color C            full, rgb222 or rgb111 (follows the quality)\n"
            "  --view-only          send no input\n"
            "  --not-shared         ask the server to disconnect other viewers\n"
            "  --no-reconnect       ask before re-establishing a dropped connection\n"
            "  --no-udp             TCP only, without their UDP transport first\n"
            "  --encryption E       AlwaysOn, PreferOn, PreferOff or Server (PreferOn)\n"
            "  --identity BLOB      what this server was accepted as last time\n"
            "  --timeout MS         connect timeout (20000)\n"
            "%s",
            vento_probe_usage);
    exit(2);
}

} // namespace

int main(int argc, char **argv) {
    vento_probe probe;
    vento_probe_init(&probe);
    vento_realvnc_config config;
    vento_realvnc_config_init(&config);
    config.state_dir = "build/scratch/realvnc-state";
    for (int i = 1; i < argc;) {
        const char *a = argv[i];
        if (vento_probe_option(&probe, argc, argv, &i)) {
            continue;
        } else if (strcmp(a, "--libs") == 0) {
            config.library_dir = vento_probe_value(argc, argv, &i);
        } else if (strcmp(a, "--state-dir") == 0) {
            config.state_dir = vento_probe_value(argc, argv, &i);
        } else if (strcmp(a, "--quality") == 0) {
            config.quality = vento_probe_value(argc, argv, &i);
        } else if (strcmp(a, "--encoding") == 0) {
            config.preferred_encoding = vento_probe_value(argc, argv, &i);
        } else if (strcmp(a, "--color") == 0) {
            config.color_level = vento_probe_value(argc, argv, &i);
        } else if (strcmp(a, "--view-only") == 0) {
            config.view_only = true;
            i++;
        } else if (strcmp(a, "--not-shared") == 0) {
            config.shared = false;
            i++;
        } else if (strcmp(a, "--no-reconnect") == 0) {
            config.auto_reconnect = false;
            i++;
        } else if (strcmp(a, "--no-udp") == 0) {
            config.enable_udp = false;
            i++;
        } else if (strcmp(a, "--encryption") == 0) {
            config.encryption = vento_probe_value(argc, argv, &i);
        } else if (strcmp(a, "--identity") == 0) {
            config.identity = vento_probe_value(argc, argv, &i);
        } else if (strcmp(a, "--timeout") == 0) {
            config.connect_timeout_ms = atoi(vento_probe_value(argc, argv, &i));
        } else if (a[0] != '-' && !config.address) {
            config.address = a;
            i++;
        } else {
            usage();
        }
    }
    if (!config.address || !config.library_dir) {
        usage();
    }
    /* The user name is passed in for the prompt to propose; the password is
       not, so that the probe answers the prompt every real session gets. */
    config.user_name = probe.user;
    printf("%s\n", vento_realvnc_version());
    return vento_probe_run(&probe, create, &config);
}
