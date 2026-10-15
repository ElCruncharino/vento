// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop.backend.realvnc;

import android.app.Activity;
import android.content.Context;
import android.content.Intent;

import net.pgaskin.remotedesktop.backend.Backend;
import net.pgaskin.remotedesktop.backend.BackendOption;
import net.pgaskin.remotedesktop.backend.BackendProvider;
import net.pgaskin.remotedesktop.plugin.Plugin;

import java.util.List;
import java.util.Map;

/**
 * This module's entry in a build's list of backends. What the RealVNC backend
 * is called and what it can be told are the catalog's, which the Linux client
 * reads too: {@link RealVncCatalog}, generated from {@code backends/session} at
 * build time, holds the words and the option keys.
 *
 * <p>On the phone the backend is this module's Java over RealVNC's own core,
 * and the catalog's rows are what it hands that core as parameters.
 */
public final class RealVncProvider implements BackendProvider {

    /**
     * What Low means here, when nobody has said otherwise: the one quality this
     * app gives a colour depth of its own.
     *
     * <p>Only Low, because only Low and Custom reach the parameter at all — a
     * Medium that asked for {@code rgb222} was asking for something the core
     * throws away — and not Custom, because Custom is the value that hands the
     * choice back to whoever set it. A connection that has chosen a depth
     * explicitly keeps it (see {@code RealVncBackend}).
     */
    static String colorLevelFor(String quality) {
        return "Low".equals(quality) ? "rgb111" : "full";
    }

    /**
     * The quality group, and the only thing known to need {@code applyOptions}
     * after {@code setOption}. The original sets {@code ViewOnly} and
     * {@code AutoReconnect} alone and calls {@code applyOptions} only after
     * quality and colour depth.
     */
    static boolean needsApply(String key) {
        return "Quality".equals(key) || "ColorLevel".equals(key)
                || "PreferredEncoding".equals(key);
    }

    @Override
    public String id() {
        return RealVncCatalog.ID;
    }

    @Override
    public String name() {
        return RealVncCatalog.NAME;
    }

    @Override
    public String shortName() {
        return RealVncCatalog.SHORT_NAME;
    }

    @Override
    public String description() {
        return RealVncCatalog.DESCRIPTION;
    }

    @Override
    public int order() {
        return RealVncCatalog.ORDER;
    }

    @Override
    public List<BackendOption> options() {
        return RealVncCatalog.OPTIONS;
    }

    @Override
    public Backend create(Context context, String address, String userName, String password,
                          Map<String, String> options) {
        return new RealVncBackend(context, address, userName, password, options);
    }

    /**
     * The one backend that is not ready the moment it is installed: its library
     * is RealVNC's, is in no build, and arrives on the device.
     */
    @Override
    public boolean isSetup(Context context) {
        return RealVncLibrary.isSetup(context);
    }

    @Override
    public void setup(Activity host) {
        host.startActivity(new Intent(Plugin.ACTION_SETUP)
                .setPackage(RealVncLibrary.pluginPackage()));
    }

    /** The core recognises a server by a blob of its own; see {@link RealVncBackend}. */
    @Override
    public void forgetHosts(Context context) {
        RealVncBackend.forgetIdentities(context);
    }

    /** Every option at its default, as the map {@code createSession} takes. */
    static Map<String, String> defaults() {
        final java.util.Map<String, String> m = new java.util.LinkedHashMap<>();
        for (BackendOption o : RealVncCatalog.OPTIONS) {
            m.put(o.key(), o.defaultValue());
        }
        return m;
    }
}
