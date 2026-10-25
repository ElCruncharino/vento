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
 * This module's entry in a build's list of backends. What the SPICE backend is
 * called and what it can be told are the catalog's: {@link SpiceCatalog},
 * generated from {@code backends/api} at build time, holds the words and the
 * option keys.
 *
 * <p>A session is a {@link NativeBackend} on {@link SpiceNatives}, generated
 * too: the client in the module's crate as an {@code api::Backend}.
 * Nothing about SPICE on this side of the boundary needs a hook — a ticket
 * prompt has no user name because the session says so, and a pin exists only
 * for a TLS session because only a TLS session asks.
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
