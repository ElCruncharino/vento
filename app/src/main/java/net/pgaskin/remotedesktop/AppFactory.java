// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop;

import android.annotation.SuppressLint;
import androidx.core.app.CoreComponentFactory;
import android.content.pm.ApplicationInfo;

import net.pgaskin.remotedesktop.backend.Backends;

/**
 * The only hook that runs before the app's class loader is created, and an
 * add-on's classes have to be reachable from that class loader.
 *
 * <p>Nothing else about component instantiation is changed.
 *
 * <p>The base class is androidx-internal, and extending it is deliberate: androidx
 * declares this factory in its own manifest, so a replacement factory has to
 * extend it or silently drop its behaviour.
 */
@SuppressLint("RestrictedApi")
public final class AppFactory extends CoreComponentFactory {

    @Override
    public ClassLoader instantiateClassLoader(ClassLoader cl, ApplicationInfo aInfo) {
        return Backends.classLoader(cl);
    }
}
