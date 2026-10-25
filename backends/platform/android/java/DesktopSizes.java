// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop.backend;

import android.content.Context;
import android.content.SharedPreferences;
import android.graphics.Insets;
import android.graphics.Rect;
import android.view.WindowInsets;
import android.view.WindowManager;
import android.view.WindowMetrics;

import java.util.Locale;

/**
 * The size each machine's desktop turned out to be, so that connecting again
 * can ask for the same thing, and the other size a session can ask for: this
 * phone's own.
 *
 * <p>RDP has no way to ask for "whatever it already is": the size is in the
 * client's connection data and something has to go there. So the nearest true
 * answer is the size the last session ended up at, which is what this
 * remembers. It is the size the <em>server</em> reported rather than the one
 * that was asked for, since those differ (a server is free to round the
 * width, and does).
 *
 * <p>One file for both RDP backends: the two clients ask the same machine for
 * the same desktop, and a size remembered through one of them is true of the
 * other. Keyed on the address as written, for {@link KnownHosts}' reason: the
 * desktop belongs to the machine and not to whichever saved record reached it.
 */
public final class DesktopSizes {

    private static final String FILE = "rdp_desktop_sizes";

    private DesktopSizes() {
    }

    private static SharedPreferences prefs(Context ctx) {
        return ctx.getApplicationContext().getSharedPreferences(FILE, Context.MODE_PRIVATE);
    }

    private static String key(String address) {
        return address == null ? "" : address.trim().toLowerCase(Locale.ROOT);
    }

    /** {@code {width, height}}, or null if this address has never connected. */
    public static int[] remembered(Context ctx, String address) {
        final String value = prefs(ctx).getString(key(address), null);
        return value == null ? null : parse(value);
    }

    public static void remember(Context ctx, String address, int width, int height) {
        if (width <= 0 || height <= 0) {
            return;
        }
        prefs(ctx).edit().putString(key(address), width + "x" + height).apply();
    }

    /** {@code 1920x1200} as its two numbers, or null. */
    public static int[] parse(String value) {
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
     * The window a session gets on this phone, in pixels (the screen less the
     * status and navigation bars, in its current orientation), or null where
     * there is no window manager to ask.
     *
     * <p>The bars are subtracted because the desktop is drawn in what is left of
     * the window, so it is that rectangle, and not the panel, that makes a scale
     * of 1 look like one pixel each. There is no activity here to ask (the
     * session outlives every screen), so this is the display's own window, which
     * differs from the session's only in a split screen. Whether a server will
     * take the size is for the caller to check: the field is 16 bits, and a
     * server handed something absurd refuses it.
     */
    public static int[] device(Context ctx) {
        final WindowManager wm = ctx.getSystemService(WindowManager.class);
        if (wm == null) {
            return null;
        }
        final WindowMetrics metrics = wm.getCurrentWindowMetrics();
        // Ignoring visibility: a bar hidden at this instant is one the session
        // will show again, and a desktop that changed size for it would be
        // worse than one that is a few pixels short.
        final Insets bars = metrics.getWindowInsets().getInsetsIgnoringVisibility(
                WindowInsets.Type.systemBars() | WindowInsets.Type.displayCutout());
        final Rect bounds = metrics.getBounds();
        return new int[]{bounds.width() - bars.left - bars.right,
                bounds.height() - bars.top - bars.bottom};
    }
}
