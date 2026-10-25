// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop;

import android.content.res.Resources;
import android.view.View;
import android.view.ViewGroup;

import java.util.ArrayList;
import java.util.List;

import com.google.android.material.textfield.TextInputLayout;

/**
 * Rows grouped onto a container with large corners, the shape this app's
 * settings, panel and editor are all built from.
 *
 * <p>A row's position in its group (first, middle, last, or a group of one)
 * decides its background, so this is the only place that knows the four
 * drawables, and both renderers use it: the settings tree's adapter binds one
 * row at a time, and the hand-built sheets pass a whole container.
 *
 * <p>A row that is already its own container is left out of the group: an
 * outlined text field has its own box and corners, and a container behind it
 * would add a second set. It keeps the group's inset, gets a wider gap, and
 * ends the group on either side of itself.
 */
final class RowGroups {

    private RowGroups() {
    }

    /** The background for a row, given the ends of the group it is in. */
    static int background(boolean first, boolean last) {
        if (first) {
            return last ? R.drawable.bg_group_single : R.drawable.bg_group_top;
        }
        return last ? R.drawable.bg_group_bottom : R.drawable.bg_group_middle;
    }

    /** The gap between two rows of one group. */
    static int gap(View v) {
        return v.getResources().getDimensionPixelSize(R.dimen.group_gap);
    }

    /** The gap between one group and the next, where no heading separates them. */
    static int spacing(View v) {
        return v.getResources().getDimensionPixelSize(R.dimen.group_spacing);
    }

    /**
     * One row: its background, the inset either side, and the gap under it.
     * Every call sets all three, since a recycled row still has the values from
     * its previous position.
     */
    static void row(View view, boolean first, boolean last, int gapBelow) {
        view.setBackgroundResource(background(first, last));
        margins(view, gapBelow);
    }

    /** A row with no group: no background, no inset, and no gap of its own. */
    static void plain(View view) {
        view.setBackground(null);
        final ViewGroup.MarginLayoutParams lp = params(view);
        if (lp != null) {
            lp.setMarginStart(0);
            lp.setMarginEnd(0);
            lp.bottomMargin = 0;
            view.setLayoutParams(lp);
        }
    }

    /**
     * Every child of a container as one group, in the order they were added.
     * Called after the rows are built, and again whenever they are rebuilt.
     *
     * <p>A hidden row is not in the group: an option shown only for some value
     * of another option would otherwise leave a gap where the group's corners
     * do not meet.
     */
    static void apply(ViewGroup container) {
        final Resources res = container.getResources();
        final int gap = res.getDimensionPixelSize(R.dimen.group_gap);
        final int fieldGap = res.getDimensionPixelSize(R.dimen.group_gap_field);
        final List<View> shown = new ArrayList<>();
        for (int i = 0; i < container.getChildCount(); i++) {
            if (container.getChildAt(i).getVisibility() != View.GONE) {
                shown.add(container.getChildAt(i));
            }
        }
        final int n = shown.size();
        for (int i = 0; i < n; i++) {
            final View view = shown.get(i);
            final boolean own = ownContainer(view);
            final boolean first = i == 0 || ownContainer(shown.get(i - 1));
            final boolean last = i == n - 1 || ownContainer(shown.get(i + 1));
            if (own) {
                view.setBackground(null);
            } else {
                view.setBackgroundResource(background(first, last));
            }
            // No gap after the last row; the container's own margin separates
            // it from what follows, another section or the end of the sheet.
            margins(view, i == n - 1 ? 0 : (own || ownContainer(shown.get(i + 1))
                    ? fieldGap : gap));
        }
    }

    private static boolean ownContainer(View view) {
        return view instanceof TextInputLayout;
    }

    private static void margins(View view, int gapBelow) {
        final ViewGroup.MarginLayoutParams lp = params(view);
        if (lp == null) {
            return;
        }
        final int inset = view.getResources().getDimensionPixelSize(R.dimen.group_inset);
        lp.setMarginStart(inset);
        lp.setMarginEnd(inset);
        lp.bottomMargin = gapBelow;
        view.setLayoutParams(lp);
    }

    private static ViewGroup.MarginLayoutParams params(View view) {
        return view.getLayoutParams() instanceof ViewGroup.MarginLayoutParams lp ? lp : null;
    }
}
