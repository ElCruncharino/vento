// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop.backend.rustdesk;

import android.content.Context;

import net.pgaskin.remotedesktop.backend.Backend;
import net.pgaskin.remotedesktop.backend.BackendOption;
import net.pgaskin.remotedesktop.backend.BackendProvider;
import net.pgaskin.remotedesktop.backend.NativeBackend;

import java.util.List;
import java.util.Map;

/**
 * This module's entry in a build's list of backends. What the RustDesk backend
 * is called and what it can be told are the catalog's: {@link RustDeskCatalog},
 * generated from {@code backends/session} at build time, holds the words and
 * the option keys.
 *
 * <p>A session is a {@link NativeBackend} on {@link RustDeskNatives}, generated
 * too: the client in the module's crate as a {@code session::Backend},
 * decoding with the phone's MediaCodec. Nothing is left on this side: how
 * its pin is filed — under the id and the network it means something on — and
 * that a first sight of it is pinned without asking are {@code session::trust}'s,
 * applied in the JNI layer.
 */
public final class RustDeskProvider implements BackendProvider {

    /** {@code ConnectBy}'s two values, which decide what the address means. */
    public static final String BY_ID = "id";
    public static final String BY_ADDRESS = "address";

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
        return new NativeBackend(context, "RustDesk", RustDeskCatalog.OPTIONS,
                new RustDeskNatives(), NativeBackend.Hooks.NONE, address, userName, password, options);
    }
}
