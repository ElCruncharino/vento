// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop.plugin;

import android.net.Uri;

/**
 * The constants both sides of an add-on share.
 *
 * <p>Two processes and two uids: the add-on's, where a library is obtained and
 * checked, and the app's, where the add-on's dex is loaded and the library is
 * used. Only a file descriptor and an intent pass between them, and both are
 * defined here.
 */
public final class Plugin {

    /**
     * The action the app starts to have an add-on set itself up. Handled by an
     * activity in the add-on's package, so that it runs with the add-on's
     * permissions and package visibility.
     */
    public static final String ACTION_SETUP = "net.pgaskin.remotedesktop.action.SETUP_PLUGIN";

    /**
     * The add-on's provider, as {@code <its package>.libraries}. Derived rather
     * than hardcoded so that a debug build's suffix carries through: an
     * authority is unique on the device, and two builds of one add-on installed
     * side by side must not collide.
     */
    public static Uri libraries(String pluginPackage) {
        return new Uri.Builder().scheme("content").authority(pluginPackage + ".libraries").build();
    }

    /** One file the provider serves, by its name in the pin. */
    public static Uri library(String pluginPackage, String name) {
        return libraries(pluginPackage).buildUpon().appendPath(name).build();
    }

    /** {@link android.content.ContentProvider#call} method: whether it has every library yet. */
    public static final String CALL_READY = "ready";

    public static final String EXTRA_READY = "ready";

    private Plugin() {
    }
}
