// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop;

import android.content.Context;
import android.content.pm.ShortcutInfo;
import android.content.pm.ShortcutManager;
import android.graphics.drawable.Icon;
import android.util.Log;

import java.util.ArrayList;
import java.util.HashSet;
import java.util.List;
import java.util.Set;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;

/**
 * Launcher shortcuts for the connections: the few in the menu on a long press
 * of the app icon, and any pinned to the home screen.
 *
 * <p>Both are the same shortcut. One id per connection means a rename updates a
 * pinned icon and a menu entry with one call, and an icon pinned long ago keeps
 * being updated rather than drifting out of date. So {@link #publish} is the
 * whole API: it is not told what changed, but reads the list and makes the
 * launcher match it, which also makes it correct after an import, a restore,
 * or an upgrade from a version without shortcuts.
 *
 * <p>A shortcut does <em>not</em> use the connection's desktop preview as its
 * icon. The preview is an image of the remote screen, which is why
 * {@link Connections#thumbnail} keeps it encrypted, while a launcher icon is
 * cached by the launcher, visible on the home screen, and outside the
 * app's control. The app's icon says which app the shortcut is for, and nothing
 * about the machine.
 *
 * <p>Off the main thread, because every call here is a binder round trip and
 * they follow writes to the connection list, which happen on user input. One
 * thread rather than a pool: two concurrent publishes could finish out of
 * order, and the launcher would keep whichever list arrived last.
 */
public final class Shortcuts {

    private static final String TAG = "Shortcuts";

    /**
     * The prefix of a connection's shortcut id. Prefixed rather than the bare
     * id so that {@code dumpsys shortcut} is readable, and so that a shortcut
     * this class did not create (such as a future one for a screen rather than a
     * connection) is not disabled by {@link #publish} as an orphan.
     */
    private static final String PREFIX = "connection:";

    private static final ExecutorService worker =
            Executors.newSingleThreadExecutor(r -> {
                final Thread t = new Thread(r, "shortcuts");
                t.setDaemon(true);
                return t;
            });

    private Shortcuts() {
    }

    /** Make the launcher's shortcuts match the saved connections, asynchronously. */
    public static void publish(Context ctx) {
        final Context app = ctx.getApplicationContext();
        worker.execute(() -> republish(app));
    }

    /**
     * Report that this connection was opened, by any means.
     *
     * <p>This feeds the launcher's ranking: the app icon menu is short, and the
     * launcher can put the most used connection first. Reported for every way of
     * opening, not only shortcuts, since a connection opened from the home
     * screen every day is the one that should be one press away.
     */
    public static void used(Context ctx, String connectionId) {
        if (connectionId == null || connectionId.isEmpty()) {
            return;
        }
        final Context app = ctx.getApplicationContext();
        worker.execute(() -> {
            final ShortcutManager mgr = app.getSystemService(ShortcutManager.class);
            if (mgr != null) {
                try {
                    mgr.reportShortcutUsed(PREFIX + connectionId);
                } catch (Exception e) {
                    Log.w(TAG, "reporting use", e);
                }
            }
        });
    }

    /** Whether the launcher supports pinned shortcuts. */
    public static boolean canPin(Context ctx) {
        final ShortcutManager mgr = ctx.getSystemService(ShortcutManager.class);
        return mgr != null && mgr.isRequestPinShortcutSupported();
    }

    /**
     * Ask the launcher to pin a home screen icon for this connection.
     *
     * <p>On the calling thread, unlike everything else here, because the caller
     * needs the result: the system shows a dialog for this request, and only a
     * foreground app may make it.
     *
     * @return false if the request was not made (no launcher support, or the
     *         launcher refused), the only case the caller has to report; the
     *         system's dialog handles the rest
     */
    public static boolean requestPin(Context ctx, Connection conn) {
        final ShortcutManager mgr = ctx.getSystemService(ShortcutManager.class);
        if (mgr == null || !mgr.isRequestPinShortcutSupported() || conn.title().isEmpty()) {
            return false;
        }
        try {
            // No IntentSender: it only reports that the launcher accepted the
            // pin, which the launcher already shows, and a callback held across
            // a dialog would have to survive a rotation.
            return mgr.requestPinShortcut(build(ctx, conn, 0), null);
        } catch (Exception e) {
            Log.w(TAG, "requesting a pinned shortcut", e);
            return false;
        }
    }

    // ---- publishing ---------------------------------------------------------

    private static void republish(Context ctx) {
        final ShortcutManager mgr = ctx.getSystemService(ShortcutManager.class);
        if (mgr == null) {
            return;
        }
        try {
            // In the home screen's order: pinned first, then in the order they
            // were added. The launcher also ranks by use (see #used), so this
            // order applies until it has usage data.
            final List<ShortcutInfo> all = new ArrayList<>();
            final Set<String> live = new HashSet<>();
            for (Connection c : Connections.all(ctx)) {
                // A shortcut needs a label, and a record with neither name nor
                // address has none. The editor will not save one, but an
                // imported file can contain one.
                if (c.title().isEmpty()) {
                    continue;
                }
                all.add(build(ctx, c, all.size()));
                live.add(PREFIX + c.id());
            }

            // Sync pinned icons with the list in both directions, from the
            // launcher's state rather than from what callers report. A
            // connection that came back (an import, a restored backup) gets its
            // icon re-enabled; a deleted one leaves a disabled icon that shows a
            // message when pressed, rather than one that fails to open. Enabling
            // first, so that the update below applies to enabled shortcuts.
            final List<String> back = new ArrayList<>();
            final List<String> gone = new ArrayList<>();
            for (ShortcutInfo s : mgr.getPinnedShortcuts()) {
                if (!s.getId().startsWith(PREFIX)) {
                    continue;
                }
                if (live.contains(s.getId())) {
                    if (!s.isEnabled()) {
                        back.add(s.getId());
                    }
                } else if (s.isEnabled()) {
                    gone.add(s.getId());
                }
            }
            if (!back.isEmpty()) {
                mgr.enableShortcuts(back);
            }

            // As many dynamic shortcuts as the launcher allows, and the rest
            // only as updates: a pinned shortcut no longer near the top of the
            // list is still an icon on the home screen, and has to follow a
            // rename like the others.
            final int max = Math.max(0, mgr.getMaxShortcutCountPerActivity());
            mgr.setDynamicShortcuts(all.subList(0, Math.min(max, all.size())));
            if (!all.isEmpty()) {
                mgr.updateShortcuts(all);
            }

            if (!gone.isEmpty()) {
                mgr.disableShortcuts(gone, ctx.getString(R.string.shortcut_deleted));
            }
        } catch (Exception e) {
            // Every call above is rate limited for a background app, and a
            // refused call leaves the launcher one edit out of date until the
            // next write. Not worth a crash after saving a connection.
            Log.w(TAG, "publishing shortcuts", e);
        }
    }

    private static ShortcutInfo build(Context ctx, Connection conn, int rank) {
        return new ShortcutInfo.Builder(ctx, PREFIX + conn.id())
                .setShortLabel(conn.title())
                // Under the icon there is room for a word; the menu has room for
                // the home screen card's subtitle too.
                .setLongLabel(conn.subtitle().isEmpty() ? conn.title()
                        : ctx.getString(R.string.shortcut_label, conn.title(), conn.subtitle()))
                .setIcon(Icon.createWithResource(ctx, R.mipmap.ic_shortcut_connection))
                .setIntent(SessionActivity.intentFor(ctx, conn.id()))
                // Lets a pinned icon outlive its dynamic shortcut: the system
                // may drop a shortcut that is not long lived as soon as it is
                // no longer one of the dynamic ones published above, and there
                // can be more connections than the launcher has room for.
                .setLongLived(true)
                .setRank(rank)
                .build();
    }
}
