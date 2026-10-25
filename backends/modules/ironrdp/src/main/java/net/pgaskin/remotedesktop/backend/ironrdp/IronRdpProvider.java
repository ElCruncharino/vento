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
 * This module's entry in a build's list of backends. What the IronRDP backend
 * is called and what it can be told are the catalog's: {@link IronRdpCatalog},
 * generated from {@code backends/session} at build time, holds the words and
 * the option keys.
 *
 * <p>A session is a {@link NativeBackend} on {@link IronRdpNatives}, generated
 * too: IronRDP in {@code src/main/rust} as a {@code session::Backend}, which
 * reads the desktop size option and splits a domain out of the user name
 * itself. What is left on this side is {@link RdpHooks}: the size this machine
 * was left at, and the prompt's words.
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
