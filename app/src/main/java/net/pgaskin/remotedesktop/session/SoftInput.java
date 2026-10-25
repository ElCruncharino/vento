// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop.session;

import android.view.View;
import android.view.WindowInsets;
import android.view.inputmethod.InputMethodManager;

import net.pgaskin.remotedesktop.control.input.ExtensionKeyboard;
import net.pgaskin.remotedesktop.control.input.MouseOverlay;

/**
 * The overlays along the bottom of the desktop, and keeping the system
 * keyboard in sync with them.
 *
 * <p>There are three, with limited room: the extension row, the mouse overlay,
 * and the system IME under the extension row. This class enforces the rules
 * between them: showing one overlay hides the other, and a hidden one comes
 * back afterwards; the row and the IME act as one keyboard, so dismissing
 * either dismisses both.
 *
 * <p>The IME is a window rather than a widget, so "showing" is a request and
 * "shown" is an inset that arrives later. Both are tracked, because the gap
 * between them causes errors: an inset saying the IME is down, read while a
 * request for it is outstanding, is stale rather than a dismissal.
 */
final class SoftInput {

    interface Listener {

        /**
         * The extension row or the overlay was shown or hidden. The host
         * updates the toolbar's toggle buttons and the viewport insets.
         */
        void chromeChanged();
    }

    private final View view;
    private final ExtensionKeyboard keyboard;
    private final MouseOverlay overlay;
    private final Listener listener;

    private boolean overlayHiddenByKeyboard;  // show it again when the keyboard hides
    private boolean overlayShown;
    private boolean keyboardShown;
    private int imeHeight;                    // the system IME's, from the window insets
    private boolean imeUp;                    // tracked separately from the height
    private boolean imeRequested;             // requested and not shown yet

    SoftInput(View view, ExtensionKeyboard keyboard, MouseOverlay overlay, Listener listener) {
        this.view = view;
        this.keyboard = keyboard;
        this.overlay = overlay;
        this.listener = listener;
    }

    /**
     * How much of the bottom of this window the system IME covers. Needed on
     * its own, not only as part of the row's inset, because the IME can outlive
     * the row that requested it.
     */
    int imeHeight() {
        return imeHeight;
    }

    /** Showing one overlay hides the other, and a displaced one comes back. */
    void setKeyboardVisible(boolean show) {
        if (show == keyboard.visible()) {
            return;
        }
        if (show) {
            overlayHiddenByKeyboard = overlay.visible();
            overlay.setVisible(false);
        }
        // Everything else happens in keyboardChanged(), because the model can
        // also hide itself from its own ✕.
        keyboard.setVisible(show);
    }

    void toggleKeyboard() {
        setKeyboardVisible(!keyboard.visible());
    }

    void toggleOverlay() {
        if (overlay.visible()) {
            overlay.setVisible(false);
        } else {
            setKeyboardVisible(false);
            overlay.setVisible(true);
        }
    }

    /**
     * Hide both, and do not show the overlay again later. For a session that
     * has become view-only, where they are hidden rather than disabled: a row of
     * keys that types nothing looks like a session that has stopped responding.
     */
    void hideAll() {
        setKeyboardVisible(false);
        overlay.setVisible(false);
        overlayHiddenByKeyboard = false;
    }

    /** The extension row's model changed, from here or from its own ✕. */
    void keyboardChanged() {
        if (keyboardShown != keyboard.visible()) {
            keyboardShown = keyboard.visible();
            sync();
            listener.chromeChanged();
        }
    }

    /** The overlay's model changed. It has no IME to sync, so only the listener is called. */
    void overlayChanged() {
        if (overlayShown != overlay.visible()) {
            overlayShown = overlay.visible();
            listener.chromeChanged();
        }
    }

    /**
     * Show or hide the system keyboard to match the extension row.
     *
     * <p>Called on every focus gain as well as on every change, in both
     * directions and unconditionally. When shown: the IME closes on an app
     * switch and does not come back on its own, leaving the row with nothing
     * under it. When hidden: the row may have been hidden while another window
     * had the focus (turning on view-only from the connection panel), and an
     * IMM call made then is dropped, leaving the soft keyboard up without the
     * row.
     */
    void sync() {
        final InputMethodManager imm = view.getContext().getSystemService(InputMethodManager.class);
        if (keyboard.visible()) {
            view.requestFocus();
            // Requested and not yet shown: until the insets say it is up, an
            // inset saying it is down is stale.
            imeRequested = true;
            imm.showSoftInput(view, 0);
        } else {
            imeRequested = false;
            imm.hideSoftInputFromWindow(view.getWindowToken(), 0);
            if (overlayHiddenByKeyboard) {
                overlayHiddenByKeyboard = false;
                overlay.setVisible(true);
            }
        }
    }

    /**
     * Track the IME's height, so the extension keyboard sits on top of it
     * rather than behind it and the desktop is inset by both. A soft keyboard
     * dismissed with the back gesture also hides the extension row.
     *
     * @return whether anything changed, so the caller can update the insets
     * once for this and its own changes together
     */
    boolean imeInsets(WindowInsets insets) {
        final int ime = insets.getInsets(WindowInsets.Type.ime()).bottom;
        // Whether it is up, checked separately from how much of this window it
        // covers. In multi-window the two differ: the window is resized around
        // the IME rather than overlapped by it, so the inset stays 0 while it
        // is showing and never drops on dismissal. Detecting the dismissal from
        // the height alone left the extension row up after a back gesture
        // closed the system keyboard.
        final boolean up = insets.isVisible(WindowInsets.Type.ime());
        if (ime == imeHeight && up == imeUp) {
            return false;
        }
        // Only while this window has focus and no IME request is outstanding.
        // An IME also closes on an app switch, and a sheet over this screen
        // takes the focus and the IME with it; treating either as a dismissal
        // would hide the extension row, and the second is a race between two
        // windows, so closing the connection panel would bring back the system
        // keyboard without the row. The back gesture this rule handles happens
        // with the session focused and no request outstanding.
        final boolean closed = imeUp && !up && view.hasWindowFocus() && !imeRequested;
        if (up) {
            imeRequested = false;
        }
        imeHeight = ime;
        imeUp = up;
        keyboard.setBottomOffset(ime);
        if (closed && keyboard.visible()) {
            setKeyboardVisible(false);
        }
        return true;
    }
}
