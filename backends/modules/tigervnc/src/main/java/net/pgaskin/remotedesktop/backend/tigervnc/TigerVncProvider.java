// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop.backend.tigervnc;

import android.content.Context;

import net.pgaskin.remotedesktop.backend.Backend;
import net.pgaskin.remotedesktop.backend.BackendOption;
import net.pgaskin.remotedesktop.backend.BackendProvider;

import java.util.List;
import java.util.Map;

/**
 * This module's entry in a build's list of backends. What the TigerVNC backend
 * is called and what it can be told are the catalog's: {@link
 * TigerVncCatalog}, generated from {@code backends/session} at build time,
 * holds the words and the option keys.
 */
public final class TigerVncProvider implements BackendProvider {

    /** RFB encoding numbers, which is what this client's preference is. */
    private static final int ENCODING_RAW = 0;
    private static final int ENCODING_RRE = 2;
    private static final int ENCODING_HEXTILE = 5;
    private static final int ENCODING_TIGHT = 7;
    private static final int ENCODING_ZRLE = 16;
    private static final int ENCODING_H264 = 50;

    /**
     * Automatic, for a quality or a colour depth. Both are the client's own
     * rules over its own bandwidth estimate, which is the row the panel already
     * shows as the line speed — this is the one client here that measures
     * anything, so it is the only one with an automatic to offer.
     */
    static final int LEVEL_AUTO = -2;

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
        return new TigerVncBackend(context, address, userName, password, options);
    }

    /** The known-hosts file earlier builds left behind; see {@link TigerVncBackend}. */
    @Override
    public void forgetHosts(Context context) {
        TigerVncBackend.forgetHosts(context);
    }

    /** A 0–9 option as the native side wants it, or −1 for "do not send it". */
    static int level(String value) {
        try {
            return Integer.parseInt(value);
        } catch (NumberFormatException e) {
            return -1;
        }
    }

    /**
     * The quality option, which has one more answer than the compression one:
     * "auto" is the client's own rule over its estimate, and everything else is
     * a level, with "lossless" falling through to the absence of one.
     */
    static int quality(String value) {
        return "auto".equals(value) ? LEVEL_AUTO : level(value);
    }

    /** The colour option as the shim's level: 0 full, 1 rgb222, 2 rgb111. */
    static int colorLevel(String value) {
        return switch (value == null ? "" : value) {
            case "auto" -> LEVEL_AUTO;
            case "rgb222" -> 1;
            case "rgb111" -> 2;
            default -> 0;
        };
    }

    /** The encoding option as the RFB number this client prefers. */
    static int encoding(String value) {
        if (value == null) {
            return ENCODING_TIGHT;
        }
        return switch (value) {
            case "h264" -> ENCODING_H264;
            case "zrle" -> ENCODING_ZRLE;
            case "hextile" -> ENCODING_HEXTILE;
            case "rre" -> ENCODING_RRE;
            case "raw" -> ENCODING_RAW;
            default -> ENCODING_TIGHT;
        };
    }
}
