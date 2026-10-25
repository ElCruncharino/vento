// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop;

import android.content.Context;

import net.pgaskin.remotedesktop.backend.BackendOption;

import java.util.List;
import java.util.Map;

/**
 * Per-connection settings the <em>app</em> owns.
 *
 * <p>A {@link BackendOption} normally describes a backend's own setting. These
 * are settings of this app that need a different value per machine, which the
 * app's settings tree cannot express. They are stored in the same map as a
 * backend's own options and shown by the same editor rows, so the user sees
 * one kind of setting rather than two. The keys carry a prefix no backend uses,
 * and {@link Options#forConnection} passes a backend only its own keys.
 *
 * <p>Nothing goes here that a backend could decide for itself. Whether the far
 * end <em>can</em> resize is a fact about a live session; whether it may be
 * <em>asked</em> to is a setting of this app, and only the second one is here.
 */
public final class AppOptions {

    /** Namespaced, because these share a map with every backend's own keys. */
    public static final String FOLLOW_WINDOW = "app.FollowWindow";

    public static final String DIRECT_TOUCH = "app.DirectTouch";

    private AppOptions() {
    }

    public static List<BackendOption> options(Context ctx) {
        return List.of(BackendOption.bool(FOLLOW_WINDOW,
                        ctx.getString(R.string.option_follow_window),
                        ctx.getString(R.string.option_follow_window_summary),
                        false, BackendOption.Scope.CONNECTION, false),
                BackendOption.bool(DIRECT_TOUCH,
                        ctx.getString(R.string.option_direct_touch),
                        ctx.getString(R.string.option_direct_touch_summary),
                        false, BackendOption.Scope.CONNECTION, false));
    }

    public static boolean directTouch(Map<String, String> options) {
        return Boolean.parseBoolean(options.getOrDefault(DIRECT_TOUCH, "false"));
    }

    public static boolean followWindow(Map<String, String> options) {
        return Boolean.parseBoolean(options.getOrDefault(FOLLOW_WINDOW, "false"));
    }
}
