// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop.plugin;

import android.content.Context;

import java.io.File;
import java.io.IOException;
import java.io.InputStream;
import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.zip.ZipEntry;
import java.util.zip.ZipFile;

/**
 * Extracts pinned libraries from a third-party APK.
 *
 * <p>Both routes an add-on offers end here, with an archive and a list of the
 * libraries it must contain. Each name is looked up at {@code lib/<abi>/<name>}
 * and kept only if it matches the pinned hash, so the archive's source (a
 * download, or a file the user picked) does not matter.
 */
public final class Apks {

    private Apks() {
    }

    /** Reports the library being extracted, for the screen to show. */
    public interface Taking {
        void library(String name);
    }

    /**
     * Reads every wanted library out of the archives, in order, and stores the
     * ones that verify.
     *
     * <p>More than one archive because an installer may have split an app by
     * ABI, in which case the libraries are in a split and not in the base.
     *
     * @return what is still missing afterwards, empty if it all arrived
     */
    public static Map<String, String> take(Context context, List<File> apks, String abi,
                                           Map<String, String> wanted, Taking taking)
            throws IOException {
        final Map<String, String> missing = new LinkedHashMap<>(wanted);
        IOException last = null;
        for (File apk : apks) {
            if (missing.isEmpty()) {
                break;
            }
            final ZipFile zip;
            try {
                zip = new ZipFile(apk);
            } catch (IOException x) {
                last = x; // a split that is not a zip does not stop the others
                continue;
            }
            // A library that is present but does not verify does stop it: the
            // archive has the wrong build, and the remaining entries come from
            // the same archive. Its error is also the most useful one to show.
            try (zip) {
                for (Map.Entry<String, String> e : new ArrayList<>(missing.entrySet())) {
                    final ZipEntry entry = zip.getEntry("lib/" + abi + "/" + e.getKey());
                    if (entry == null) {
                        continue;
                    }
                    taking.library(e.getKey());
                    try (InputStream in = zip.getInputStream(entry)) {
                        LibraryStore.store(context, e.getKey(), e.getValue(), in);
                    }
                    missing.remove(e.getKey());
                }
            }
        }
        if (!missing.isEmpty() && last != null) {
            throw last;
        }
        return missing;
    }

}
