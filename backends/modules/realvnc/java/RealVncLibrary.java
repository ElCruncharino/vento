// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop.backend.realvnc;

import android.annotation.SuppressLint;
import android.content.Context;

import com.realvnc.vncviewer.jni.Library;

import net.pgaskin.remotedesktop.backend.Backends;
import net.pgaskin.remotedesktop.plugin.Libraries;

import java.io.File;
import java.io.IOException;
import java.util.Map;

/**
 * Where {@code libvncviewer.so} comes from; it is not in this build.
 *
 * <p>It is in no APK: the add-on this module ships in acquires a copy on the
 * device, from RealVNC, and keeps it in its own data directory. This code runs
 * as the app, so a copy crosses over once and is loaded from the app's own
 * files, verified against {@link Library} on the way, since the hash of a
 * third-party binary is the only thing that makes loading it safe.
 */
final class RealVncLibrary {

    private RealVncLibrary() {
    }

    /**
     * The package of the add-on this class was loaded from. Null would mean
     * these classes are in the app itself, which is not supported.
     */
    static String pluginPackage() {
        final String pkg = Backends.packageOf(RealVncLibrary.class.getClassLoader());
        if (pkg == null) {
            throw new IllegalStateException("not loaded from an add-on");
        }
        return pkg;
    }

    static boolean isSetup(Context context) {
        final Map<String, String> wanted = Library.wanted();
        // Not just `ready`: an empty set of requirements is met by an empty
        // store, and a device with no pinned build can never be set up, rather
        // than being set up already.
        return !wanted.isEmpty() && Libraries.ready(context, pluginPackage(), wanted);
    }

    /** Loads both libraries, in order, from the paths that were just verified. */
    // Not loadLibrary: the library is not in the APK. It is a file passed over
    // by an add-on, copied into this app's own storage and checked against a
    // hash this build carries before it is opened.
    @SuppressLint("UnsafeDynamicallyLoadedCode")
    static void load(Context app) throws IOException {
        final Map<String, String> wanted = Library.wanted();
        if (wanted.isEmpty()) {
            throw new IOException("There is no build of this backend's library for this phone.");
        }
        for (File file : Libraries.load(app, pluginPackage(), wanted)) {
            System.load(file.getAbsolutePath());
        }
    }
}
