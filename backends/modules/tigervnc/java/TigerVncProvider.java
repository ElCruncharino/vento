// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop.backend.tigervnc;

import android.content.Context;
import android.util.Log;

import net.pgaskin.remotedesktop.backend.Backend;
import net.pgaskin.remotedesktop.backend.BackendOption;
import net.pgaskin.remotedesktop.backend.BackendProvider;
import net.pgaskin.remotedesktop.backend.NativeBackend;
import net.pgaskin.remotedesktop.backend.VncHooks;

import java.io.File;
import java.util.List;
import java.util.Map;

/**
 * This module's entry in a build's list of backends. The TigerVNC backend's
 * name and options come from the catalog, which the Linux client reads too:
 * {@link TigerVncCatalog}, generated from {@code backends/api} at build time,
 * holds the strings and the option keys.
 *
 * <p>A session is a {@link NativeBackend} on {@link TigerVncNatives}, also
 * generated: the shim over TigerVNC's client in {@code cpp/}, linked into one
 * library with the module's crate, the backend over it, which turns the
 * options into the RFB encoding the client prefers and its levels, and writes
 * the panel's rows. A certificate is checked against {@code KnownHosts} as
 * every other backend's is, including one a certificate authority vouches
 * for, which TigerVNC's own viewer would accept silently. An RSA-AES server
 * key, which only this client supports, is asked about in TigerVNC's viewer's
 * words every time, since that viewer does not store one either. What remains
 * on this side is {@link VncHooks} and the file below.
 */
public final class TigerVncProvider implements BackendProvider {

    private static final String TAG = "TigerVnc";

    /** Where earlier builds kept TigerVNC's own known hosts, under the files directory. */
    private static final String STATE_DIR = "tigervnc";
    private static final String KNOWN_HOSTS = "x509_known_hosts";

    @Override
    public String id() {
        return TigerVncCatalog.ID;
    }

    @Override
    public String name() {
        return TigerVncCatalog.NAME;
    }

    @Override
    public String shortName() {
        return TigerVncCatalog.SHORT_NAME;
    }

    @Override
    public String description() {
        return TigerVncCatalog.DESCRIPTION;
    }

    @Override
    public int order() {
        return TigerVncCatalog.ORDER;
    }

    @Override
    public List<BackendOption> options() {
        return TigerVncCatalog.OPTIONS;
    }

    @Override
    public Backend create(Context context, String address, String userName, String password,
                          Map<String, String> options) {
        return new NativeBackend(context, TAG, TigerVncCatalog.OPTIONS, new TigerVncNatives(),
                VncHooks.INSTANCE, address, userName, password, options);
    }

    /**
     * Deletes the TigerVNC known-hosts file that earlier builds kept under the
     * app's files directory. It is unused (certificates are pinned in
     * {@code KnownHosts}), but it still lists the machines this device has
     * connected to, written without the user being asked, so "forget every
     * server" would be incomplete if it left the file behind.
     */
    @Override
    public void forgetHosts(Context context) {
        final File hosts = new File(new File(context.getFilesDir(), STATE_DIR), KNOWN_HOSTS);
        if (hosts.exists() && !hosts.delete()) {
            Log.w(TAG, "could not delete " + hosts);
        }
    }
}
