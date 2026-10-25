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
 * This module's entry in a build's list of backends. The RustDesk backend's
 * name and options come from the catalog, which the Linux client reads too:
 * {@link RustDeskCatalog}, generated from {@code backends/api} at build time,
 * holds the text and the option keys.
 *
 * <p>A session is a {@link NativeBackend} on {@link RustDeskNatives}, also
 * generated: the client in the module's crate as an {@code api::Backend},
 * decoding with the phone's MediaCodec. There is no Java-side behaviour: the
 * pin's key (the id and its network) and pinning a first key without asking
 * are handled by {@code api::trust}, applied in the JNI layer as in the Linux
 * client.
 */
public final class RustDeskProvider implements BackendProvider {

    /** The two {@code ConnectBy} values, which decide what the address means. */
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
     * Labels the address field "ID" when connecting by id: the nine digits
     * shown on the other machine. When connecting by address, the app's
     * default label applies.
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
