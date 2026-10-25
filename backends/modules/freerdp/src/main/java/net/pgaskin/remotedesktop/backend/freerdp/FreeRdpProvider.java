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
 * This module's entry in a build's list of backends. What the FreeRDP backend
 * is called and what it can be told are the catalog's: {@link FreeRdpCatalog},
 * generated from {@code backends/session} at build time, holds the words and
 * the option keys.
 *
 * <p>A session is a {@link NativeBackend} on {@link FreeRdpNatives}, generated
 * too: the shim over FreeRDP in {@code src/main/cpp}, with the crate in
 * {@code src/main/rust} linked into the same library as the backend over it —
 * which reads the desktop size option, splits a domain out of the user name,
 * and gives the library a directory of ours to keep its configuration in.
 * Where it differs from IronRDP, the library is why: credentials are asked for
 * by FreeRDP itself, so a mistyped password is a second prompt rather than a
 * closed session; and its own certificate store is told to stay out of it, so
 * the pins in {@code KnownHosts} are the only thing that decides. What is left on this side
 * is {@link RdpHooks}, the same as IronRDP's — the size this machine was left
 * at, and the prompt's words — and the store below.
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
     * The library's own certificate store, emptied: one PEM per server, under
     * {@code files/freerdp/server/}, which builds before this one wrote and
     * this one neither writes nor reads (the shim answers
     * {@code VerifyX509Certificate} from the trust prompt). The {@code certs/}
     * directory beside it is left alone, since the library only ever read it
     * and nothing here puts anything there.
     *
     * <p>So nothing in it decides anything — but it is still a list of the
     * machines this phone has been to, written without anybody asking, and
     * "forget every server" that left it behind would be a half-truth. The
     * directory is the one the crate hands the library, under the files
     * directory {@link NativeBackend} gives every session.
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
