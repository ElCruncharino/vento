// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop.session;

import net.pgaskin.remotedesktop.control.input.ExtensionKeyboard;
import net.pgaskin.remotedesktop.control.input.KeySink;
import net.pgaskin.remotedesktop.control.input.Keysym;
import net.pgaskin.remotedesktop.control.input.Scheduler;

/**
 * Pasting is <b>typing</b>: the clipboard's characters are sent to the remote
 * one at a time as key presses, rather than as a Ctrl+V that assumes the far
 * end has a synced clipboard and pastes with that shortcut.
 *
 * <p>Two cases need confirmation first, because typed text cannot be undone
 * from here: a lot of text, and a modifier locked on the extension row. The
 * view cannot show dialogs, so confirmation is a callback, and this class only
 * decides when to ask.
 */
final class PasteTyper {

    interface Confirm {

        /**
         * Ask for confirmation before typing anything. Run {@code proceed} if
         * confirmed.
         *
         * @param heldModifiers the locked ones, "Ctrl + Shift", or empty
         */
        void ask(int characters, String heldModifiers, Runnable proceed);
    }

    private static final int CONFIRM_CHARS = 250; // longer is worth confirming first

    private static final long CHAR_MS = 8;        // spaces out a long paste

    private final KeySink keys;
    private final Scheduler scheduler;
    private final Confirm confirm;
    private final Runnable nothingToPaste;

    private String pasting;
    private int at;

    PasteTyper(KeySink keys, Scheduler scheduler, Confirm confirm, Runnable nothingToPaste) {
        this.keys = keys;
        this.scheduler = scheduler;
        this.confirm = confirm;
        this.nothingToPaste = nothingToPaste;
    }

    /**
     * Type {@code text}, asking for confirmation first if needed.
     *
     * @param text            the clipboard text, or null or empty for none
     * @param lockedModifiers {@link #lockedModifiers} for the extension row
     */
    void paste(String text, String lockedModifiers) {
        if (text == null || text.isEmpty()) {
            nothingToPaste.run();
            return;
        }
        // A locked modifier is held at the far end while it is locked, so every
        // character of the paste would arrive as a shortcut, at any length.
        // One-shot modifiers do not matter: the row releases them as soon as
        // the paste key fires.
        final int chars = text.codePointCount(0, text.length());
        if (chars > CONFIRM_CHARS || !lockedModifiers.isEmpty()) {
            confirm.ask(chars, lockedModifiers, () -> typeOut(text));
        } else {
            typeOut(text);
        }
    }

    /** The locked modifiers, in row order, as "Ctrl + Shift"; empty if none. */
    static String lockedModifiers(ExtensionKeyboard keyboard) {
        final StringBuilder sb = new StringBuilder();
        for (ExtensionKeyboard.Key m : keyboard.modifiers()) {
            if (keyboard.sticky(m) == ExtensionKeyboard.Sticky.LOCKED) {
                if (sb.length() > 0) {
                    sb.append(" + ");
                }
                sb.append(m.label());
            }
        }
        return sb.toString();
    }

    /**
     * One character per tick. Scheduled rather than in a loop for two reasons:
     * a thousand key events sent at once may not arrive intact or in order at
     * the far end, and a paste in progress has to be cancellable
     * ({@link #cancel()}).
     */
    private void typeOut(String text) {
        pasting = text;
        at = 0;
        scheduler.removeCallbacks(tick);
        scheduler.postDelayed(tick, 0);
    }

    private final Runnable tick = new Runnable() {
        @Override
        public void run() {
            if (pasting == null) {
                return;
            }
            if (at >= pasting.length()) {
                pasting = null;
                return;
            }
            final int cp = pasting.codePointAt(at);
            at += Character.charCount(cp);
            // A newline is sent as Return and a tab as Tab, since they are keys
            // rather than characters on a keyboard.
            final int keysym = Keysym.forCharacter(cp);
            if (keysym != 0) {
                keys.keyDown(keysym, KeySink.ID_TEXT);
                keys.keyUp(KeySink.ID_TEXT);
            }
            scheduler.postDelayed(this, CHAR_MS);
        }
    };

    /** Stop typing; what has already been typed stays. */
    void cancel() {
        pasting = null;
        scheduler.removeCallbacks(tick);
    }
}
