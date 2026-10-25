// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop.backend.freerdp;

import android.content.Context;
import android.util.Log;

import net.pgaskin.remotedesktop.backend.Backend;
import net.pgaskin.remotedesktop.backend.BackendOption;
import net.pgaskin.remotedesktop.backend.BackendProvider;
import net.pgaskin.remotedesktop.backend.NativeBackend;
import net.pgaskin.remotedesktop.backend.RdpHooks;

import java.io.File;
import java.util.List;
import java.util.Map;

/**
 * This module's entry in a build's list of backends. The FreeRDP backend's name
 * and options come from the catalog, which the Linux client also reads:
 * {@link FreeRdpCatalog}, generated from {@code backends/api} at build time,
 * holds the text and the option keys.
 *
 * <p>A session is a {@link NativeBackend} on {@link FreeRdpNatives}, also
 * generated: the shim over FreeRDP in {@code cpp/}, with the module's crate
 * linked into the same library as the backend over it, which reads the desktop
 * size option, splits a domain out of the user name, and gives the library a
 * directory of ours for its configuration. The differences from IronRDP come
 * from the library: FreeRDP itself asks for credentials, and only when the
 * record has none (a mistyped password is refused by the server, and the
 * session closes with "Logon failed."); and its own certificate store is
 * bypassed, so only the pins in {@code KnownHosts} decide. What remains on this
 * side is {@link RdpHooks}, shared with IronRDP (the desktop size remembered
 * for this machine, and the prompt's text), and the store cleanup below.
 */
public final class FreeRdpProvider implements BackendProvider {

    private static final String TAG = "FreeRdp";

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
        return new NativeBackend(context, TAG, FreeRdpCatalog.OPTIONS, new FreeRdpNatives(),
                new RdpHooks(context, address), address, userName, password, options);
    }

    /**
     * Empties the library's own certificate store: one PEM per server, under
     * {@code files/freerdp/server/}, written by earlier builds and neither
     * written nor read by this one (the shim answers
     * {@code VerifyX509Certificate} from the trust prompt). The {@code certs/}
     * directory next to it is left alone, since the library only ever read it
     * and nothing here writes to it.
     *
     * <p>Nothing in it affects trust, but it is still a list of the machines
     * this phone has connected to, written without asking, so "forget every
     * server" removes it too. The directory is the one the crate gives the
     * library, under the files directory {@link NativeBackend} gives every
     * session.
     */
    @Override
    public void forgetHosts(Context context) {
        final File[] servers = new File(new File(context.getFilesDir(), "freerdp"), "server")
                .listFiles();
        if (servers != null) {
            for (File f : servers) {
                if (f.exists() && !f.delete()) {
                    Log.w(TAG, "could not delete " + f);
                }
            }
        }
    }
}
