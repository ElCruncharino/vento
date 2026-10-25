// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop;

import android.annotation.SuppressLint;
import android.app.Activity;
import android.view.LayoutInflater;
import android.view.MotionEvent;
import android.view.View;
import android.view.ViewGroup;
import android.widget.TextView;

import androidx.annotation.NonNull;
import androidx.core.graphics.Insets;
import androidx.core.view.ViewCompat;
import androidx.core.view.WindowInsetsCompat;
import androidx.recyclerview.widget.ItemTouchHelper;
import androidx.recyclerview.widget.LinearLayoutManager;
import androidx.recyclerview.widget.RecyclerView;

import com.google.android.material.bottomsheet.BottomSheetBehavior;
import com.google.android.material.bottomsheet.BottomSheetDialog;

import java.util.ArrayList;
import java.util.Collections;
import java.util.List;

/**
 * The pinned connections in their current order, for reordering by drag.
 *
 * <p>A sheet rather than a mode over the home screen. Pressing a card opens the
 * connection and a long press edits it, and the list is as often a grid as a
 * column: dragging there would need a third meaning for the same gesture, a
 * boundary around the pinned run so nothing lands outside it, and a meaning
 * for dragging sideways. The only question here is which comes first.
 *
 * <p>It holds a snapshot: the pinned connections as they were when it opened,
 * reordered locally and written whenever a move finishes. Written then rather
 * than on dismissal, because a sheet can be swiped away and this is not a form
 * with a save button.
 */
final class PinnedOrderPanel {

    private static final int MAX_WIDTH_DP = 560; // past which it is a centred card

    private final Activity activity;
    private final Runnable onChanged;
    private final BottomSheetDialog dialog;
    private final Adapter adapter = new Adapter();
    private final ItemTouchHelper drags;

    /** The pinned connections, in the order they are presently shown in. */
    private final List<Connection> items = new ArrayList<>();

    /**
     * @param onChanged run after the order is written, so the list behind the
     *                  sheet can show the new order
     */
    static PinnedOrderPanel show(Activity activity, Runnable onChanged) {
        final PinnedOrderPanel panel = new PinnedOrderPanel(activity, onChanged);
        panel.dialog.show();
        return panel;
    }

    // A sheet's root has no parent to be measured against: the dialog's window
    // supplies the layout parameters and the ones in the file are not read.
    @SuppressLint("InflateParams")
    private PinnedOrderPanel(Activity activity, Runnable onChanged) {
        this.activity = activity;
        this.onChanged = onChanged;

        for (Connection c : Connections.all(activity)) {
            if (c.pinned()) {
                items.add(c);
            }
        }

        dialog = new BottomSheetDialog(activity);
        final View content = LayoutInflater.from(dialog.getContext())
                .inflate(R.layout.sheet_pinned, null);
        dialog.setContentView(content);
        dialog.getBehavior().setMaxWidth(dp(MAX_WIDTH_DP));
        // Fully expanded with no collapsed state, as in ConnectionPanel: half
        // of the pinned list is not useful for ordering it.
        dialog.getBehavior().setSkipCollapsed(true);
        dialog.getBehavior().setState(BottomSheetBehavior.STATE_EXPANDED);
        ViewCompat.setOnApplyWindowInsetsListener(content, (v, insets) -> {
            final Insets bars = insets.getInsets(WindowInsetsCompat.Type.systemBars());
            v.setPadding(v.getPaddingLeft(), v.getPaddingTop(), v.getPaddingRight(),
                    bars.bottom);
            return insets;
        });

        content.findViewById(R.id.close).setOnClickListener(v -> dialog.dismiss());

        final RecyclerView list = content.findViewById(R.id.list);
        list.setLayoutManager(new LinearLayoutManager(activity));
        list.setAdapter(adapter);
        drags = new ItemTouchHelper(new Callback());
        drags.attachToRecyclerView(list);
    }

    boolean isShowing() {
        return dialog.isShowing();
    }

    void dismiss() {
        dialog.dismiss();
    }

    private int dp(int value) {
        return Math.round(value * activity.getResources().getDisplayMetrics().density);
    }

    /**
     * Writes the current order and notifies the screen behind.
     *
     * <p>Only the pinned ids are passed, so {@link Connections#reorder} moves
     * them among the positions they already occupy and leaves the rest of the
     * list unchanged.
     */
    private void commit() {
        final List<String> order = new ArrayList<>(items.size());
        for (Connection c : items) {
            order.add(c.id());
        }
        Connections.reorder(activity, order);
        if (onChanged != null) {
            onChanged.run();
        }
    }

    /**
     * Moves a row one place up or down: the screen reader's alternative to
     * dragging, which needs to see where the rows are.
     */
    private boolean move(RecyclerView.ViewHolder holder, int by) {
        final int from = holder.getBindingAdapterPosition();
        final int to = from + by;
        if (from == RecyclerView.NO_POSITION || to < 0 || to >= items.size()) {
            return false;
        }
        Collections.swap(items, from, to);
        adapter.notifyItemMoved(from, to);
        commit();
        return true;
    }

    // ---- the list -----------------------------------------------------------

    private final class Adapter extends RecyclerView.Adapter<Holder> {

        // The handle below is not a click and must not become one: it hands the
        // gesture to the touch helper. Users who cannot drag get the two
        // accessibility actions instead.
        @SuppressLint("ClickableViewAccessibility")
        @NonNull
        @Override
        public Holder onCreateViewHolder(@NonNull ViewGroup parent, int viewType) {
            final Holder h = new Holder(LayoutInflater.from(parent.getContext())
                    .inflate(R.layout.item_pinned, parent, false));
            // Set here rather than on every bind: each row is its own
            // container, so its margins do not depend on its position.
            final ViewGroup.MarginLayoutParams lp =
                    (ViewGroup.MarginLayoutParams) h.itemView.getLayoutParams();
            final int inset = parent.getResources().getDimensionPixelSize(R.dimen.group_inset);
            lp.setMarginStart(inset);
            lp.setMarginEnd(inset);
            lp.bottomMargin =
                    parent.getResources().getDimensionPixelSize(R.dimen.group_gap_field);
            h.itemView.setLayoutParams(lp);

            // Added once per view rather than per bind, since a second copy of
            // an action is a second entry in the menu a screen reader reads out.
            ViewCompat.addAccessibilityAction(h.itemView,
                    activity.getString(R.string.home_reorder_up), (v, args) -> move(h, -1));
            ViewCompat.addAccessibilityAction(h.itemView,
                    activity.getString(R.string.home_reorder_down), (v, args) -> move(h, 1));

            h.drag.setOnTouchListener((v, event) -> {
                if (event.getActionMasked() == MotionEvent.ACTION_DOWN) {
                    drags.startDrag(h);
                }
                // Not consumed: the handle only starts the drag, and the touch
                // helper handles the gesture from then on.
                return false;
            });
            return h;
        }

        @Override
        public void onBindViewHolder(@NonNull Holder h, int position) {
            final Connection c = items.get(position);
            h.title.setText(c.title());
            h.subtitle.setText(c.subtitle());
            // GONE rather than the home screen's INVISIBLE: these rows have
            // nothing beside them to line up with, so a connection with no name
            // is a shorter row.
            h.subtitle.setVisibility(c.subtitle().isEmpty() ? View.GONE : View.VISIBLE);
        }

        @Override
        public int getItemCount() {
            return items.size();
        }
    }

    private static final class Holder extends RecyclerView.ViewHolder {
        final TextView title;
        final TextView subtitle;
        final View drag;

        Holder(View v) {
            super(v);
            title = v.findViewById(R.id.title);
            subtitle = v.findViewById(R.id.subtitle);
            drag = v.findViewById(R.id.drag);
        }
    }

    /**
     * Up and down only. No swipe: the only meaning it could have is unpinning,
     * which is the editor's toggle.
     */
    private final class Callback extends ItemTouchHelper.SimpleCallback {

        /** Whether this drag has moved anything, so a press that goes nowhere
         * does not rewrite the file. */
        private boolean moved;

        Callback() {
            super(ItemTouchHelper.UP | ItemTouchHelper.DOWN, 0);
        }

        @Override
        public boolean onMove(@NonNull RecyclerView list, @NonNull RecyclerView.ViewHolder from,
                              @NonNull RecyclerView.ViewHolder to) {
            final int a = from.getBindingAdapterPosition();
            final int b = to.getBindingAdapterPosition();
            if (a == RecyclerView.NO_POSITION || b == RecyclerView.NO_POSITION) {
                return false;
            }
            Collections.swap(items, a, b);
            adapter.notifyItemMoved(a, b);
            moved = true;
            return true;
        }

        @Override
        public void onSwiped(@NonNull RecyclerView.ViewHolder holder, int direction) {
        }

        /**
         * The end of the gesture, when the order is written: a drag passes
         * every row between its start and end, and writing per step would
         * rewrite the file and republish the launcher's shortcuts once per
         * row passed.
         */
        @Override
        public void clearView(@NonNull RecyclerView list,
                              @NonNull RecyclerView.ViewHolder holder) {
            super.clearView(list, holder);
            if (moved) {
                moved = false;
                commit();
            }
        }
    }
}
