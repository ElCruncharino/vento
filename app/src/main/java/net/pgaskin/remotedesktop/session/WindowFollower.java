// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop.session;

import android.app.Activity;
import android.view.WindowInsets;
import android.view.WindowMetrics;

import net.pgaskin.remotedesktop.backend.Backend;

/**
 * Asks the far end for a desktop the size of this phone's window.
 *
 * <p>Off unless enabled per connection: resizing changes the remote machine's
 * desktop, which should not happen just because the phone was rotated.
 *
 * <p>Only debouncing and the resize request, nothing to do with drawing. It is
 * polled on a tick rather than triggered by events, because the far end may
 * only accept a size some time after connecting, and a one-shot request sent
 * too early would never be retried.
 */
public final class WindowFollower {

    /**
     * How long after the window changes size the request is sent. A rotation is
     * one event and dragging a split screen divider is dozens; a resize is a
     * round trip and, on RDP, a full reactivation.
     */
    static final long DEBOUNCE_MS = 600;

    private final Backend backend;

    private boolean enabled;
    /** The size last requested, so an unchanged window sends no request. */
    private int followedW, followedH;
    /** The window size at the previous tick: two equal ticks mean it has settled. */
    private int settledW, settledH;

    WindowFollower(Backend backend) {
        this.backend = backend;
    }

    boolean enabled() {
        return enabled;
    }

    /**
     * Set from the connection's option. Checked at the next tick rather than at
     * the next rotation, so turning it on takes effect immediately.
     */
    void setEnabled(boolean follow) {
        enabled = follow;
    }

    /**
     * Request the window size, if enabled for this connection and the window
     * has stopped changing.
     *
     * <p>Two ticks with the same size count as settled, which is the debounce: a
     * split screen being dragged is a new size every frame and a resize is a
     * round trip. The same size is never requested twice, since a far end that
     * refused it will refuse it again, so a refusal costs one request rather
     * than one per tick.
     *
     * @param canResize from the caller's own poll ({@link Backend.Facts}), to
     *                  avoid querying the backend a second time
     */
    void tick(Activity activity, boolean canResize) {
        if (!enabled) {
            return;
        }
        final int[] size = deviceSize(activity);
        if (size == null) {
            return;
        }
        final boolean settled = size[0] == settledW && size[1] == settledH;
        settledW = size[0];
        settledH = size[1];
        if (!settled || (size[0] == followedW && size[1] == followedH) || !canResize) {
            return;
        }
        followedW = size[0];
        followedH = size[1];
        backend.requestDesktopSize(size[0], size[1]);
    }

    /**
     * This session's window size, minus the system bars and display cutout:
     * the rectangle the desktop is drawn in, so a desktop of this size maps one
     * pixel to one pixel at scale 1. Bars that are currently hidden are still
     * subtracted: they come back, and a desktop a few pixels short is better
     * than one that changes size for a swipe.
     *
     * <p>Deliberately not the viewport's content rect, which shrinks for the
     * extension keyboard and the IME: those come and go several times a minute,
     * and resizing the desktop for each would make it unusable.
     *
     * <p>Public because the connection panel offers the same size as a choice,
     * and the two must agree.
     */
    public static int[] deviceSize(Activity activity) {
        if (activity == null) {
            return null;
        }
        final WindowMetrics metrics = activity.getWindowManager().getCurrentWindowMetrics();
        final android.graphics.Insets bars = metrics.getWindowInsets().getInsetsIgnoringVisibility(
                WindowInsets.Type.systemBars() | WindowInsets.Type.displayCutout());
        final int w = metrics.getBounds().width() - bars.left - bars.right;
        final int h = metrics.getBounds().height() - bars.top - bars.bottom;
        // A sanity check on the rectangle rather than a protocol limit: a size
        // outside this range is not a usable window for a desktop.
        return w < 200 || h < 200 || w > 16384 || h > 16384 ? null : new int[]{w, h};
    }
}
