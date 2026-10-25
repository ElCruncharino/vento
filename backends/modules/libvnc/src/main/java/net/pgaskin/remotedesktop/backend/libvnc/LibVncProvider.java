// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop.backend.libvnc;

import android.content.Context;

import net.pgaskin.remotedesktop.backend.Backend;
import net.pgaskin.remotedesktop.backend.BackendOption;
import net.pgaskin.remotedesktop.backend.BackendProvider;

import java.util.List;
import java.util.Map;

/**
 * This module's entry in a build's list of backends. What the libvncclient
 * backend is called and what it can be told are the catalog's: {@link
 * LibVncCatalog}, generated from {@code backends/session} at build time, holds
 * the words and the option keys.
 */
public final class LibVncProvider implements BackendProvider {

    /**
     * The encodings string libvncclient parses, best first, and "Automatic" is
     * its own default list unchanged — including the two nothing here decodes
     * any better than the server's alternative, since what the list decides is
     * what the server picks.
     */
    static final String AUTO_ENCODINGS = "tight zrle ultra copyrect hextile zlib corre rre raw";

    @Override
    public String id() {
        return LibVncCatalog.ID;
    }

    @Override
    public String name() {
        return LibVncCatalog.NAME;
    }

    @Override
    public String shortName() {
        return LibVncCatalog.SHORT_NAME;
    }

    @Override
    public String description() {
        return LibVncCatalog.DESCRIPTION;
    }

    @Override
    public int order() {
        return LibVncCatalog.ORDER;
    }

    @Override
    public List<BackendOption> options() {
        return LibVncCatalog.OPTIONS;
    }

    @Override
    public Backend create(Context context, String address, String userName, String password,
                          Map<String, String> options) {
        return new LibVncBackend(context, address, userName, password, options);
    }

    /** A 0–9 option as the native side wants it, or −1 for "do not send it". */
    static int level(String value) {
        try {
            return Integer.parseInt(value);
        } catch (NumberFormatException e) {
            return -1;
        }
    }

    /** The colour option as the shim's level: 0 full, 1 rgb222, 2 rgb111. */
    static int colorLevel(String value) {
        return switch (value == null ? "" : value) {
            case "rgb222" -> 1;
            case "rgb111" -> 2;
            default -> 0;
        };
    }

    /** The encoding option as libvncclient's encodings string. */
    static String encodings(String value) {
        return value == null || value.equals("auto") ? AUTO_ENCODINGS : value + " copyrect";
    }
}
