// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop.backend.rustdesk;

import android.content.Context;

import net.pgaskin.remotedesktop.backend.Backend;
import net.pgaskin.remotedesktop.backend.BackendOption;
import net.pgaskin.remotedesktop.backend.BackendProvider;

import java.util.List;
import java.util.Map;

/**
 * This module's entry in a build's list of backends. What the RustDesk backend
 * is called and what it can be told are the catalog's: {@link
 * RustDeskCatalog}, generated from {@code backends/session} at build time,
 * holds the words and the option keys.
 */
public final class RustDeskProvider implements BackendProvider {

    /** {@code ConnectBy}'s two values, which decide what the address means. */
    public static final String BY_ID = "id";
    public static final String BY_ADDRESS = "address";

    /**
     * The public rendezvous network, which is what an empty
     * {@code RendezvousServer} means. Named here so that a pin keyed on it is
     * keyed on something rather than on the absence of an answer.
     */
    public static final String PUBLIC_NETWORK = "rs-ny.rustdesk.com";

    @Override
    public String id() {
        return RustDeskCatalog.ID;
    }

    @Override
    public String name() {
        return RustDeskCatalog.NAME;
    }

    @Override
    public String shortName() {
        return RustDeskCatalog.SHORT_NAME;
    }

    @Override
    public String description() {
        return RustDeskCatalog.DESCRIPTION;
    }

    @Override
    public int order() {
        return RustDeskCatalog.ORDER;
    }

    @Override
    public List<BackendOption> options() {
        return RustDeskCatalog.OPTIONS;
    }

    /**
     * What the address field holds by id: nine digits somebody reads off the
     * other machine. Dialled by address it is an address, which is what the app
     * calls it anyway.
     */
    @Override
    public String addressLabel(Map<String, String> options) {
        return BY_ID.equals(options.getOrDefault(RustDeskCatalog.CONNECT_BY, BY_ID)) ? "ID" : null;
    }

    @Override
    public Backend create(Context context, String address, String userName, String password,
                          Map<String, String> options) {
        return new RustDeskBackend(context, address, userName, password, options);
    }

    /** The frame rate option as the native side wants it: 0 for their choice. */
    static int fps(String value) {
        try {
            return Integer.parseInt(value);
        } catch (NumberFormatException e) {
            return 0;
        }
    }
}
