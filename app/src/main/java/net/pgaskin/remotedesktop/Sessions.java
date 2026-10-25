// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop;

import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;

/**
 * Every session that has been opened and not yet closed, keyed by its target.
 *
 * <p>A registry rather than a single live session because a window is not the
 * connection: each session has its own window in the recents list, two side by
 * side in split screen are both live, and a session with no window is the usual
 * case the foreground service exists for. A key names a target, so opening the
 * same connection twice attaches to the running session instead of
 * reconnecting.
 *
 * <p>Main thread only. Everything that calls it (a screen opening or
 * finishing, a state callback the backend has already posted) runs there.
 */
public final class Sessions {

    /**
     * Called when the set changes or when a session in it changes state. One
     * callback for the whole set: both watchers (the service and the home
     * screen) rebuild from the whole set anyway, and a per-session callback
     * would mean the service subscribing and unsubscribing as sessions come and
     * go.
     */
    public interface Watcher {
        void sessionsChanged();
    }

    private static final Map<String, Session> live = new LinkedHashMap<>();
    private static final List<Watcher> watchers = new ArrayList<>();

    private Sessions() {
    }

    /**
     * The key for a saved connection's session, and ({@link #keyForAddress})
     * for an address without a saved connection. Defined here rather than at
     * either caller because the home screen and the session screen both look
     * up sessions by it, and must build it the same way.
     */
    public static String keyFor(String connectionId) {
        return "id:" + connectionId;
    }

    public static String keyForAddress(String address) {
        return "addr:" + address;
    }

    /** The session registered for {@code key}, alive or ended, or null. */
    public static Session byKey(String key) {
        return key == null ? null : live.get(key);
    }

    /** Every session, in the order they were opened. */
    public static List<Session> all() {
        return List.copyOf(live.values());
    }

    /**
     * The sessions that need a notification: connected, or connecting. A
     * session that has ended stays registered until its screen closes it, so
     * that the screen can keep showing the reason it stopped.
     */
    public static List<Session> alive() {
        final List<Session> out = new ArrayList<>(live.size());
        for (Session s : live.values()) {
            if (s.alive()) {
                out.add(s);
            }
        }
        return out;
    }

    /** Whether {@code key} has a session that has not ended. */
    public static boolean isAlive(String key) {
        final Session s = byKey(key);
        return s != null && s.alive();
    }

    static void add(Session s) {
        live.put(s.key(), s);
        changed();
    }

    static void remove(Session s) {
        // Only if it is still the registered one: closing a session that a
        // reconnect has already replaced must not unregister the new one.
        if (live.get(s.key()) == s) {
            live.remove(s.key());
        }
        changed();
    }

    /**
     * The session timeout setting has been changed, imported or reset, so every
     * session's deadline moves with it, including one already off screen for
     * longer than the new timeout, which closes now.
     */
    public static void timeoutChanged() {
        for (Session s : all()) {
            s.rearmTimeout();
        }
    }

    public static void addWatcher(Watcher w) {
        watchers.add(w);
    }

    public static void removeWatcher(Watcher w) {
        watchers.remove(w);
    }

    /** A copy, because a watcher may register or unregister from inside this. */
    static void changed() {
        for (Watcher w : new ArrayList<>(watchers)) {
            w.sessionsChanged();
        }
    }
}
