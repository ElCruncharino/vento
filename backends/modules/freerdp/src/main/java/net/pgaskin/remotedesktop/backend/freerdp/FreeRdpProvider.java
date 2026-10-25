// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop.backend.freerdp;

import android.content.Context;

import net.pgaskin.remotedesktop.backend.Backend;
import net.pgaskin.remotedesktop.backend.BackendOption;
import net.pgaskin.remotedesktop.backend.BackendProvider;
import net.pgaskin.remotedesktop.backend.DesktopSizes;

import java.util.List;
import java.util.Map;

/**
 * This module's entry in a build's list of backends. What the FreeRDP backend
 * is called and what it can be told are the catalog's: {@link FreeRdpCatalog},
 * generated from {@code backends/session} at build time, holds the words and
 * the option keys.
 */
public final class FreeRdpProvider implements BackendProvider {

    /** The two answers to "how big" that are not a number; see {@link #size}. */
    static final String REMOTE = "remote";
    static final String DEVICE = "device";

    /**
     * What to ask for when nothing better is known: the first connection to a
     * machine under {@link #REMOTE}, and anything unparseable.
     */
    private static final int[] FALLBACK = {1920, 1200};

    @Override
    public String id() {
        return FreeRdpCatalog.ID;
    }

    @Override
    public String name() {
        return FreeRdpCatalog.NAME;
    }

    @Override
    public String shortName() {
        return FreeRdpCatalog.SHORT_NAME;
    }

    @Override
    public String description() {
        return FreeRdpCatalog.DESCRIPTION;
    }

    @Override
    public int order() {
        return FreeRdpCatalog.ORDER;
    }

    @Override
    public List<BackendOption> options() {
        return FreeRdpCatalog.OPTIONS;
    }

    @Override
    public Backend create(Context context, String address, String userName, String password,
                          Map<String, String> options) {
        return new FreeRdpBackend(context, address, userName, password, options);
    }

    /** The library kept a certificate store of its own once; see {@link FreeRdpBackend}. */
    @Override
    public void forgetHosts(Context context) {
        FreeRdpBackend.forgetHosts(context);
    }

    /**
     * The desktop size to ask this connection's server for, as
     * {@code {width, height}}.
     *
     * <p>Whatever comes back is the server's to decide and is reported in the
     * connection result, which is why nothing is rounded here.
     */
    static int[] size(Context ctx, String address, String value) {
        if (REMOTE.equals(value)) {
            final int[] last = DesktopSizes.remembered(ctx, address);
            return last != null ? last : FALLBACK;
        }
        if (DEVICE.equals(value)) {
            // Both ends of the protocol's range: the field is 16 bits, and a
            // server handed something absurd refuses the connection rather
            // than arguing.
            final int[] device = DesktopSizes.device(ctx);
            return device == null || device[0] < 200 || device[1] < 200
                    || device[0] > 4096 || device[1] > 4096 ? FALLBACK : device;
        }
        final int[] parsed = DesktopSizes.parse(value);
        return parsed != null ? parsed : FALLBACK;
    }
}
