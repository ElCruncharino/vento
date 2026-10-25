// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop.backend.libvnc;

import android.content.Context;

import net.pgaskin.remotedesktop.backend.Backend;
import net.pgaskin.remotedesktop.backend.BackendOption;
import net.pgaskin.remotedesktop.backend.BackendProvider;
import net.pgaskin.remotedesktop.backend.NativeBackend;
import net.pgaskin.remotedesktop.backend.VncHooks;

import java.util.List;
import java.util.Map;

/**
 * This module's entry in a build's list of backends. The libvncclient
 * backend's name and options come from the catalog, which the Linux client
 * reads too: {@link LibVncCatalog}, generated from {@code backends/api} at
 * build time, holds the text and the option keys.
 *
 * <p>A session is a {@link NativeBackend} on {@link LibVncNatives}, also
 * generated. The native library holds the shim over libvncclient in {@code
 * cpp/} and the module's crate, which converts the options into the library's
 * encodings string and levels and writes the panel's rows. The only
 * Java-side behaviour is {@link VncHooks}.
 */
public final class LibVncProvider implements BackendProvider {

    @Override
    public String id() {
        return LibVncCatalog.ID;
    }

    @Override
    public String name() {
        return LibVncCatalog.NAME;
    }

    @Override
    public String shortName() {
        return LibVncCatalog.SHORT_NAME;
    }

    @Override
    public String description() {
        return LibVncCatalog.DESCRIPTION;
    }

    @Override
    public int order() {
        return LibVncCatalog.ORDER;
    }

    @Override
    public List<BackendOption> options() {
        return LibVncCatalog.OPTIONS;
    }

    @Override
    public Backend create(Context context, String address, String userName, String password,
                          Map<String, String> options) {
        return new NativeBackend(context, "LibVnc", LibVncCatalog.OPTIONS, new LibVncNatives(),
                VncHooks.INSTANCE, address, userName, password, options);
    }
}
