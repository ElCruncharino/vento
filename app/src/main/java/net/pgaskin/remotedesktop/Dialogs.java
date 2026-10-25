// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop;

import android.content.Context;
import android.content.DialogInterface;
import android.content.res.TypedArray;
import android.util.TypedValue;
import android.view.ContextThemeWrapper;

import androidx.appcompat.app.AlertDialog;

import com.google.android.material.checkbox.MaterialCheckBox;
import com.google.android.material.dialog.MaterialAlertDialogBuilder;

import java.util.ArrayList;
import java.util.List;

/**
 * Shared dialog plumbing: the theme every dialog is built with, the checkbox
 * and padding that have to match a dialog's own metrics, and a tracker that
 * dismisses open dialogs.
 */
final class Dialogs {

    private Dialogs() {
    }

    /**
     * A builder using this app's dialog theme, which matters:
     * {@code ThemeOverlay.RemoteDesktop.Dialog} keeps an alert from being drawn
     * in two tones with a hard edge across it. Named once here so that no call
     * site can leave it out.
     */
    static MaterialAlertDialogBuilder builder(Context ctx) {
        return new MaterialAlertDialogBuilder(ctx, R.style.ThemeOverlay_RemoteDesktop_Dialog);
    }

    /**
     * A checkbox for a dialog, built with the dialog's theme.
     *
     * <p>A plain {@code android.widget.CheckBox} built from an activity is the
     * framework's, not Material's: different metrics, and a tint from the
     * activity rather than from the dialog surface. The box is drawn at the
     * view's left edge with the label after it, so the padding that aligns it
     * with the dialog's text belongs on the container; padding here would move
     * the box and leave the label where it was.
     */
    static MaterialCheckBox checkBox(Context ctx, int text) {
        final MaterialCheckBox box = new MaterialCheckBox(
                new ContextThemeWrapper(ctx, R.style.ThemeOverlay_RemoteDesktop_Dialog));
        box.setText(text);
        box.setPadding(0, box.getPaddingTop(), box.getPaddingRight(), box.getPaddingBottom());
        return box;
    }

    /** What a dialog indents its title and message by, so a view can match. */
    static int padding(Context ctx) {
        final TypedArray a = new ContextThemeWrapper(ctx, R.style.ThemeOverlay_RemoteDesktop_Dialog)
                .obtainStyledAttributes(new int[]{androidx.appcompat.R.attr.dialogPreferredPadding});
        final int pad = a.getDimensionPixelSize(0, dp(ctx, 24));
        a.recycle();
        return pad;
    }

    static int dp(Context ctx, float v) {
        return Math.round(TypedValue.applyDimension(TypedValue.COMPLEX_UNIT_DIP, v,
                ctx.getResources().getDisplayMetrics()));
    }

    /**
     * Every dialog a screen has shown, so that its {@code onDestroy} can dismiss
     * them: one still showing when its activity is destroyed is a leaked window.
     */
    static final class Tracker {

        private final List<AlertDialog> open = new ArrayList<>();

        AlertDialog track(AlertDialog d) {
            return track(d, null);
        }

        /**
         * A dialog has exactly <em>one</em> dismiss listener, and this method
         * sets it, so a caller that needs to know when its dialog is dismissed
         * passes the work in here. Setting its own would replace this one or be
         * replaced by it, depending on the order.
         */
        AlertDialog track(AlertDialog d, DialogInterface.OnDismissListener also) {
            open.add(d);
            d.setOnDismissListener(x -> {
                open.remove(d);
                if (also != null) {
                    also.onDismiss(x);
                }
            });
            return d;
        }

        void dismissAll() {
            for (AlertDialog d : new ArrayList<>(open)) {
                d.dismiss();
            }
            open.clear();
        }
    }
}
