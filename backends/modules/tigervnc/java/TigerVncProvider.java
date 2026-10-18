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
 * This module's entry in a build's list of backends. What the TigerVNC backend
 * is called and what it can be told are the catalog's, which the Linux client
 * reads too: {@link TigerVncCatalog}, generated from {@code backends/api}
 * at build time, holds the words and the option keys.
 *
 * <p>A session is a {@link NativeBackend} on {@link TigerVncNatives}, generated
 * too: the shim over TigerVNC's client in {@code cpp/}, with the module's
 * crate linked into the same library as the backend over it
 * — which turns the options into the RFB encoding the client prefers and its
 * levels, and words the panel's rows. A certificate meets {@code KnownHosts}
 * as every other backend's does, and one an authority vouches for goes there
 * too, where their viewer would take it without a word; an RSA-AES server key,
 * which only this client speaks, is asked about in their viewer's words every
 * time, since their viewer keeps nothing of one either. What is left on this
 * side is {@link VncHooks}, and the file below.
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
     * The known-hosts file of TigerVNC's own that builds before this one kept
     * under the app's files directory, deleted. Nothing reads it any more —
     * certificates are pinned in {@code KnownHosts} — but it is still a list
     * of the machines this phone has been to, written without anybody asking,
     * and "forget every server" that left it behind would be a half-truth.
     */
    @Override
    public void forgetHosts(Context context) {
        final File hosts = new File(new File(context.getFilesDir(), STATE_DIR), KNOWN_HOSTS);
        if (hosts.exists() && !hosts.delete()) {
            Log.w(TAG, "could not delete " + hosts);
        }
    }
}
