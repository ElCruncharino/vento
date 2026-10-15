// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop.backend.rfb;

import android.content.Context;

import net.pgaskin.remotedesktop.backend.Backend;
import net.pgaskin.remotedesktop.backend.BackendOption;
import net.pgaskin.remotedesktop.backend.BackendProvider;

import java.util.List;
import java.util.Map;

/**
 * This module's entry in a build's list of backends. What the RFB backend is
 * called and what it can be told are the catalog's, which the Linux client
 * reads too: {@link RfbCatalog}, generated from {@code backends/session} at
 * build time, holds the words and the option keys.
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
        return new RfbBackend(context, address, userName, password, options);
    }

    /** The compression option as the native side wants it: 0–9, or −1 for none. */
    static int compressLevel(String value) {
        try {
            return Integer.parseInt(value);
        } catch (NumberFormatException e) {
            return -1;
        }
    }
}
