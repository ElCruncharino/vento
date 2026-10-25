// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop;

import android.content.Context;
import android.content.SharedPreferences;

import net.pgaskin.remotedesktop.backend.BackendOption;
import net.pgaskin.remotedesktop.backend.Backends;

import java.util.LinkedHashMap;
import java.util.Map;

/**
 * Where an option's value comes from.
 *
 * <p>Four layers, the most general first, each able to override the one above
 * it:
 *
 * <ol>
 *   <li><b>the schema's default</b>, used when nothing is set
 *   <li><b>the backend's own settings</b>, in a preference file of its own
 *   <li><b>this connection's</b>, on the record
 *   <li><b>this session's</b>, sent to a backend that is already running
 * </ol>
 *
 * <p>Which of the middle two an option may be read from is its
 * {@link BackendOption.Scope}, and that rule is why this is one class rather
 * than the layering repeated wherever it is needed. A
 * {@link BackendOption.Scope#CONNECTION} option is offered in the editor only,
 * so a value for it in a backend's preference file is stale (set when the
 * option had a different scope, or by another option with the same key), and
 * reading it would make a connection silently inherit a setting no screen
 * offered. The same applies in reverse to {@link BackendOption.Scope#GLOBAL}.
 *
 * <p><b>Only what has been set.</b> {@link #forConnection} and
 * {@link #forBackend} leave out every option that has not been set rather than
 * filling in its default. Otherwise a backend cannot tell "this connection
 * asked for full colour" from "never set", and one that adjusts a setting on
 * another's behalf cannot know whether it may. This broke the RealVNC quality
 * control, which rewrote ColorLevel with its current value, so applyOptions
 * re-applied the same pixel format and the picture never changed. Backends
 * fill in their own defaults.
 */
public final class Options {

    private Options() {
    }

    /** Where a backend's {@link BackendOption.Scope#GLOBAL} settings live. */
    public static SharedPreferences backendPrefs(Context ctx, String backendId) {
        return ctx.getApplicationContext()
                .getSharedPreferences("backend_" + backendId, Context.MODE_PRIVATE);
    }

    /**
     * One option's value, from the most specific layer that has one.
     *
     * @param own the connection's own values, or null when there is no
     *            connection (a session opened from the command line)
     * @return the value set, or null if none is
     */
    private static String chosen(BackendOption o, SharedPreferences global,
                                 Map<String, String> own) {
        if (own != null && o.scope() != BackendOption.Scope.GLOBAL) {
            final String v = own.get(o.key());
            if (v != null && !v.isEmpty()) {
                return v;
            }
        }
        if (o.scope() != BackendOption.Scope.CONNECTION) {
            final String v = global.getString(o.key(), null);
            if (v != null && !v.isEmpty()) {
                return v;
            }
        }
        return null;
    }

    /** The options to pass to {@link Backends#create} for a saved connection. */
    public static Map<String, String> forConnection(Context ctx, Connection conn) {
        return chosen(ctx, conn.backendId(), conn.options());
    }

    /**
     * The same for a session with no saved connection: everything set in the
     * backend's own settings screen, and nothing else.
     */
    public static Map<String, String> forBackend(Context ctx, String backendId) {
        return chosen(ctx, backendId, null);
    }

    private static Map<String, String> chosen(Context ctx, String backendId,
                                              Map<String, String> own) {
        final Map<String, String> out = new LinkedHashMap<>();
        final SharedPreferences global = backendPrefs(ctx, backendId);
        for (BackendOption o : Backends.options(backendId)) {
            final String v = chosen(o, global, own);
            if (v != null) {
                out.put(o.key(), v);
            }
        }
        return out;
    }

    /**
     * Every one of a backend's options as a connection that overrides none of
     * them would get them: all layers but the connection's, so the editor can
     * show the inherited value.
     *
     * <p>Every key, with the schema's default where no layer sets it: unlike the
     * two above, the caller displays a value rather than passing it on, and a
     * row cannot display "nothing".
     */
    public static Map<String, String> inherited(Context ctx, String backendId) {
        final Map<String, String> out = new LinkedHashMap<>();
        final SharedPreferences global = backendPrefs(ctx, backendId);
        for (BackendOption o : Backends.options(backendId)) {
            final String v = chosen(o, global, null);
            out.put(o.key(), v != null ? v : o.defaultValue());
        }
        return out;
    }

    /**
     * The fourth layer, for a session that is already running: the value sent
     * to it since it started, else the value it was created with (from
     * {@link #forConnection}, computed once when the backend was created),
     * else the schema's default.
     *
     * @param told what {@link Session#liveOption} returns, or null
     */
    public static String live(String told, Map<String, String> connectedWith, BackendOption o) {
        if (told != null) {
            return told;
        }
        final String connected = connectedWith.get(o.key());
        return connected != null ? connected : o.defaultValue();
    }
}
