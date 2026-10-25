// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop.backend.freerdp;

import android.content.Context;
import android.graphics.Insets;
import android.graphics.Rect;
import android.view.WindowInsets;
import android.view.WindowManager;
import android.view.WindowMetrics;

import net.pgaskin.remotedesktop.backend.Backend;
import net.pgaskin.remotedesktop.backend.BackendOption;
import net.pgaskin.remotedesktop.backend.BackendProvider;

import java.util.List;
import java.util.Map;

/**
 * This module's entry in a build's list of backends. What the FreeRDP backend
 * is called and what it can be told are the catalog's: {@link FreeRdpCatalog},
 * generated from {@code backends/session} at build time, holds the words and
 * the option keys.
 */
public final class FreeRdpProvider implements BackendProvider {

    /** The two answers to "how big" that are not a number; see {@link #size}. */
    static final String REMOTE = "remote";
    static final String DEVICE = "device";

    /**
     * What to ask for when nothing better is known: the first connection to a
     * machine under {@link #REMOTE}, and anything unparseable.
     */
    private static final int[] FALLBACK = {1920, 1200};

    @Override
    public String id() {
        return FreeRdpCatalog.ID;
    }

    @Override
    public String name() {
        return FreeRdpCatalog.NAME;
    }

    @Override
    public String shortName() {
        return FreeRdpCatalog.SHORT_NAME;
    }

    @Override
    public String description() {
        return FreeRdpCatalog.DESCRIPTION;
    }

    @Override
    public int order() {
        return FreeRdpCatalog.ORDER;
    }

    @Override
    public List<BackendOption> options() {
        return FreeRdpCatalog.OPTIONS;
    }

    @Override
    public Backend create(Context context, String address, String userName, String password,
                          Map<String, String> options) {
        return new FreeRdpBackend(context, address, userName, password, options);
    }

    /** The library kept a certificate store of its own once; see {@link FreeRdpBackend}. */
    @Override
    public void forgetHosts(Context context) {
        FreeRdpBackend.forgetHosts(context);
    }

    /**
     * The desktop size to ask this connection's server for, as
     * {@code {width, height}}.
     *
     * <p>Whatever comes back is the server's to decide and is reported in the
     * connection result, which is why nothing is rounded here.
     */
    static int[] size(Context ctx, String address, String value) {
        if (REMOTE.equals(value)) {
            final int[] last = DesktopSizes.remembered(ctx, address);
            return last != null ? last : FALLBACK;
        }
        if (DEVICE.equals(value)) {
            return deviceSize(ctx);
        }
        final int[] parsed = parse(value);
        return parsed != null ? parsed : FALLBACK;
    }

    /** {@code 1920x1200} as its two numbers, or null. */
    static int[] parse(String value) {
        if (value == null) {
            return null;
        }
        final int at = value.indexOf('x');
        if (at <= 0) {
            return null;
        }
        try {
            final int w = Integer.parseInt(value.substring(0, at));
            final int h = Integer.parseInt(value.substring(at + 1));
            return w > 0 && h > 0 ? new int[]{w, h} : null;
        } catch (NumberFormatException e) {
            return null;
        }
    }

    /**
     * The window a session gets on this phone, in pixels — the screen less the
     * status and navigation bars, in whichever orientation it is held now.
     *
     * <p>The bars are subtracted because the desktop is drawn in what is left of
     * the window. There is no activity here to ask — the session outlives every
     * screen — so this is the display's own window, which differs from the
     * session's only in a split screen.
     */
    private static int[] deviceSize(Context ctx) {
        final WindowManager wm = ctx.getSystemService(WindowManager.class);
        if (wm == null) {
            return FALLBACK;
        }
        final WindowMetrics metrics = wm.getCurrentWindowMetrics();
        final Insets bars = metrics.getWindowInsets().getInsetsIgnoringVisibility(
                WindowInsets.Type.systemBars() | WindowInsets.Type.displayCutout());
        final Rect bounds = metrics.getBounds();
        final int w = bounds.width() - bars.left - bars.right;
        final int h = bounds.height() - bars.top - bars.bottom;
        // Both ends of the protocol's range: the field is 16 bits, and a server
        // handed something absurd refuses the connection rather than arguing.
        if (w < 200 || h < 200 || w > 4096 || h > 4096) {
            return FALLBACK;
        }
        return new int[]{w, h};
    }
}
