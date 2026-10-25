// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop.session;

import android.content.ClipData;
import android.content.ClipboardManager;
import android.content.Context;
import android.util.Log;

import net.pgaskin.remotedesktop.AppSettings;

/**
 * Clipboard sync between the phone and the remote session.
 *
 * <p>The two directions use different threads and have different rules:
 *
 * <ul>
 *   <li><b>Local to remote</b> is a <em>pull</em>. The RealVNC binding has no
 *       way to push a clipboard; the core asks inline on its own session
 *       thread, so the answer has to be cached in a field already.
 *   <li><b>Remote to local</b> is a push on the main thread, and is a plain
 *       {@code setPrimaryClip}.
 * </ul>
 *
 * <p>The cache is required, not an optimisation. Since Android 10 an app may
 * only read the clipboard <em>while it has focus</em>, and since Android 12
 * every read shows a "pasted from clipboard" toast; a background thread reading
 * at an arbitrary moment would often get null, and would show the toast
 * repeatedly. Reading on the clip-changed callback and on focus reads exactly
 * when it is allowed. The original (RealVNC Viewer) reads inline, which works
 * because it targets Android 5.
 */
final class SessionClipboard implements ClipboardManager.OnPrimaryClipChangedListener {

    private static final String TAG = "SessionClipboard";

    private static final int MAX_CHARS = 1 << 20; // longer clips are ignored

    /** Called whenever the local clipboard changes, for a backend that can push. */
    interface Sink {
        void clipboardToRemote(String text);
    }

    private final Context ctx;
    private final ClipboardManager cm;
    private final Sink sink;

    private volatile String forRemote; // read on the session thread, written on the main thread

    /**
     * The last text seen from either side, so a clipboard just set from the
     * remote is not read back and sent straight back to it. Main thread only.
     */
    private String lastSeen;

    /**
     * Set while this class's own write to the system clipboard is pending.
     *
     * <p>{@link #lastSeen} is not enough for that, because the change callback
     * for our own write arrives <em>before the new value is readable</em>:
     * {@code getPrimaryClip} at that moment returns the <b>previous</b> clip,
     * which differs from {@code lastSeen} and so looks like a new local copy.
     * The cache would then stay one value behind and push the stale text back
     * to the server.
     */
    private boolean written;

    SessionClipboard(Context ctx, Sink sink) {
        this.ctx = ctx.getApplicationContext();
        this.cm = ctx.getSystemService(ClipboardManager.class);
        this.sink = sink;
    }

    void start() {
        if (cm != null) {
            cm.addPrimaryClipChangedListener(this);
            read();
        }
    }

    void stop() {
        if (cm != null) {
            cm.removePrimaryClipChangedListener(this);
        }
    }

    /**
     * Any thread: the last text read from the phone's clipboard.
     *
     * <p>Not gated on the clipboard sharing setting: the Paste key types this,
     * which is an explicit user action rather than automatic sharing.
     */
    String current() {
        return forRemote;
    }

    /** Any thread, called inline by the protocol: the text to send, or null if sharing is off. */
    String forRemote() {
        return AppSettings.clipboardOut(ctx) ? forRemote : null;
    }

    /** Main thread. Set the local clipboard to text copied on the remote. */
    void fromRemote(String text) {
        if (cm == null || text == null || text.isEmpty() || text.equals(lastSeen)
                || !AppSettings.clipboardIn(ctx)) {
            return;
        }
        lastSeen = text;
        forRemote = text;
        written = true;
        try {
            cm.setPrimaryClip(ClipData.newPlainText(null, text));
        } catch (RuntimeException e) {
            // A clipboard write can be refused (a work profile, a device
            // policy); that must not end the session.
            written = false;
            Log.w(TAG, "cannot set the clipboard", e);
        }
    }

    @Override
    public void onPrimaryClipChanged() {
        if (written) {
            // The callback for our own write; reading now would get the
            // previous clip (see the field).
            written = false;
            return;
        }
        read();
    }

    /**
     * Main thread. Re-read the system clipboard, if allowed. Called on every
     * change and whenever the window gains focus: a copy made in another app
     * changes the clip while this app is not focused, so the read in that
     * change callback gets null, and the focus gain reads it again.
     */
    void read() {
        if (cm == null) {
            return;
        }
        final ClipData clip;
        try {
            clip = cm.getPrimaryClip();
        } catch (RuntimeException e) {
            Log.w(TAG, "cannot read the clipboard", e);
            return;
        }
        if (clip == null || clip.getItemCount() == 0) {
            return;
        }
        final CharSequence cs = clip.getItemAt(0).coerceToText(ctx);
        if (cs == null || cs.length() == 0 || cs.length() > MAX_CHARS) {
            return;
        }
        final String text = cs.toString();
        if (text.equals(lastSeen)) {
            return;
        }
        lastSeen = text;
        forRemote = text;
        // Both cached and pushed, because backends differ: RealVNC's core only
        // pulls, and a backend that can push should not have to wait to be
        // asked.
        if (AppSettings.clipboardOut(ctx)) {
            sink.clipboardToRemote(text);
        }
    }

    /** For the HUD: the length of the text the remote would get, or "-". */
    String summary() {
        final String s = forRemote();
        return s == null ? "-" : s.length() + "c";
    }
}
