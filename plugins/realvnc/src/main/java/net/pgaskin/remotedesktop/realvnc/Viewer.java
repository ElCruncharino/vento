// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop.realvnc;

import com.realvnc.vncviewer.jni.Library;

import net.pgaskin.remotedesktop.plugin.Download;

import java.util.List;

/**
 * Where a copy of RealVNC's viewer can be downloaded. Only this add-on knows
 * this, not the app.
 *
 * <p>None of this is what gets verified: {@link Library} names the build the
 * declarations bind to and the hash of each library in it, and that decides
 * whether a copy is kept. A mirror may repack the APK, so the libraries in it
 * are checked, not the archive.
 */
final class Viewer {

    /** RealVNC Viewer's package name, as the Aptoide API takes it. */
    static final String PACKAGE = "com.realvnc.viewer.android";

    /** The version code of the release {@link Library} was read from. */
    static final int VERSION_CODE = 2070175;

    private Viewer() {
    }

    /**
     * Tried in order. Aptoide has an API that takes an exact version code, with
     * nothing to scrape; the second is a Wayback Machine snapshot of the file
     * that API pointed at when this was written. APKMirror is left out on
     * purpose: it returns 403 to scripts, so it would need a cookie jar and
     * HTML parsing.
     */
    static List<Download.Source> sources() {
        return List.of(
                new Download.Source(Download.Kind.APTOIDE,
                        "https://ws75.aptoide.com/api/7/app/getMeta"
                                + "?package_name=" + PACKAGE + "&vercode=" + VERSION_CODE),
                new Download.Source(Download.Kind.DIRECT,
                        "https://web.archive.org/web/20260811195414id_/"
                                + "https://pool.apk.aptoide.com/appupdater/"
                                + "com-realvnc-viewer-android-2070175-75463342-"
                                + "f98743311290dcd93f886016879cadfb.apk"));
    }
}
