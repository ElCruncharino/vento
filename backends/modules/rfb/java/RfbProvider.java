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
 * This module's entry in a build's list of backends. The RFB backend's name and
 * options come from the catalog, which the Linux client also reads:
 * {@link RfbCatalog}, generated from {@code backends/api} at build time, holds
 * the text and the option keys.
 *
 * <p>A session is a {@link NativeBackend} on {@link RfbNatives}, also
 * generated: the client in the module's crate as an {@code api::Backend}.
 * Nothing about RFB on the Java side needs a hook.
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
