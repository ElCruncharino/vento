// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop.backend.spice;

import android.content.Context;

import net.pgaskin.remotedesktop.backend.Backend;
import net.pgaskin.remotedesktop.backend.BackendOption;
import net.pgaskin.remotedesktop.backend.BackendProvider;
import net.pgaskin.remotedesktop.backend.NativeBackend;

import java.util.List;
import java.util.Map;

/**
 * This module's entry in a build's list of backends. The SPICE backend's name
 * and options come from the catalog, which the Linux client reads too:
 * {@link SpiceCatalog}, generated from {@code backends/api} at build time,
 * holds the strings and the option keys.
 *
 * <p>A session is a {@link NativeBackend} on {@link SpiceNatives}, also
 * generated, which binds the module crate's {@code api::Backend}. No
 * SPICE-specific hooks are needed: the session reports that a ticket prompt
 * has no user name, and asks about a certificate only on a TLS connection.
 */
public final class SpiceProvider implements BackendProvider {

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
        return new NativeBackend(context, "Spice", SpiceCatalog.OPTIONS, new SpiceNatives(),
                NativeBackend.Hooks.NONE, address, userName, password, options);
    }
}
