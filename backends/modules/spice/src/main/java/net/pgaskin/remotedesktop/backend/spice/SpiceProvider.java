// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop.backend.spice;

import android.content.Context;

import net.pgaskin.remotedesktop.backend.Backend;
import net.pgaskin.remotedesktop.backend.BackendOption;
import net.pgaskin.remotedesktop.backend.BackendProvider;

import java.util.List;
import java.util.Map;

/**
 * This module's entry in a build's list of backends. What the SPICE backend is
 * called and what it can be told are the catalog's, which the Linux client
 * reads too: {@link SpiceCatalog}, generated from {@code backends/session} at
 * build time, holds the words and the option keys.
 */
public final class SpiceProvider implements BackendProvider {

    /** {@code Security}'s two values, which are two different ports. */
    public static final String PLAIN = "plain";
    public static final String TLS = "tls";

    @Override
    public String id() {
        return SpiceCatalog.ID;
    }

    @Override
    public String name() {
        return SpiceCatalog.NAME;
    }

    @Override
    public String shortName() {
        return SpiceCatalog.SHORT_NAME;
    }

    @Override
    public String description() {
        return SpiceCatalog.DESCRIPTION;
    }

    @Override
    public int order() {
        return SpiceCatalog.ORDER;
    }

    @Override
    public List<BackendOption> options() {
        return SpiceCatalog.OPTIONS;
    }

    @Override
    public Backend create(Context context, String address, String userName, String password,
                          Map<String, String> options) {
        return new SpiceBackend(context, address, userName, password, options);
    }
}
