// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop.backend.rfb;

import android.content.Context;

import net.pgaskin.remotedesktop.backend.Backend;
import net.pgaskin.remotedesktop.backend.BackendOption;
import net.pgaskin.remotedesktop.backend.BackendProvider;
import net.pgaskin.remotedesktop.backend.NativeBackend;

import java.util.List;
import java.util.Map;

/**
 * This module's entry in a build's list of backends. What the RFB backend is
 * called and what it can be told are the catalog's, which the Linux client
 * reads too: {@link RfbCatalog}, generated from {@code backends/session} at
 * build time, holds the words and the option keys.
 *
 * <p>A session is a {@link NativeBackend} on {@link RfbNatives}, generated
 * too: the client in {@code src/main/rust} as a {@code session::Backend},
 * with nothing about RFB on this side of the boundary that needs a hook.
 */
public final class RfbProvider implements BackendProvider {

    @Override
    public String id() {
        return RfbCatalog.ID;
    }

    @Override
    public String name() {
        return RfbCatalog.NAME;
    }

    @Override
    public String shortName() {
        return RfbCatalog.SHORT_NAME;
    }

    @Override
    public String description() {
        return RfbCatalog.DESCRIPTION;
    }

    @Override
    public int order() {
        return RfbCatalog.ORDER;
    }

    @Override
    public List<BackendOption> options() {
        return RfbCatalog.OPTIONS;
    }

    @Override
    public Backend create(Context context, String address, String userName, String password,
                          Map<String, String> options) {
        return new NativeBackend(context, "Rfb", RfbCatalog.OPTIONS, new RfbNatives(),
                NativeBackend.Hooks.NONE, address, userName, password, options);
    }
}
