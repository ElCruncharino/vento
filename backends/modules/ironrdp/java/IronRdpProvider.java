// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop.backend.ironrdp;

import android.content.Context;

import net.pgaskin.remotedesktop.backend.Backend;
import net.pgaskin.remotedesktop.backend.BackendOption;
import net.pgaskin.remotedesktop.backend.BackendProvider;
import net.pgaskin.remotedesktop.backend.NativeBackend;
import net.pgaskin.remotedesktop.backend.RdpHooks;

import java.util.List;
import java.util.Map;

/**
 * This module's entry in a build's list of backends. The IronRDP backend's
 * name and options come from the catalog, which the Linux client reads too:
 * {@link IronRdpCatalog}, generated from {@code backends/api} at build time,
 * holds the text and the option keys.
 *
 * <p>A session is a {@link NativeBackend} on {@link IronRdpNatives}, also
 * generated: IronRDP in the module's crate as an {@code api::Backend}, which
 * reads the desktop size option and splits a domain out of the user name
 * itself. The Java side only adds {@link RdpHooks}: the desktop size this
 * address was left at, and the prompt's text.
 */
public final class IronRdpProvider implements BackendProvider {

    @Override
    public String id() {
        return IronRdpCatalog.ID;
    }

    @Override
    public String name() {
        return IronRdpCatalog.NAME;
    }

    @Override
    public String shortName() {
        return IronRdpCatalog.SHORT_NAME;
    }

    @Override
    public String description() {
        return IronRdpCatalog.DESCRIPTION;
    }

    @Override
    public int order() {
        return IronRdpCatalog.ORDER;
    }

    @Override
    public List<BackendOption> options() {
        return IronRdpCatalog.OPTIONS;
    }

    @Override
    public Backend create(Context context, String address, String userName, String password,
                          Map<String, String> options) {
        return new NativeBackend(context, "Rdp", IronRdpCatalog.OPTIONS, new IronRdpNatives(),
                new RdpHooks(context, address), address, userName, password, options);
    }
}
