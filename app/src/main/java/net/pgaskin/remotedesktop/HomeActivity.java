// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop;

import android.annotation.SuppressLint;
import android.content.Intent;
import android.content.res.ColorStateList;
import android.graphics.Bitmap;
import android.os.Bundle;
import android.view.LayoutInflater;
import android.view.MenuItem;
import android.view.View;
import android.view.ViewGroup;
import android.widget.ImageView;
import android.widget.TextView;

import androidx.annotation.NonNull;
import androidx.appcompat.app.AppCompatActivity;
import androidx.core.graphics.Insets;
import androidx.core.view.ViewCompat;
import androidx.core.view.WindowCompat;
import androidx.core.view.WindowInsetsCompat;
import androidx.recyclerview.widget.GridLayoutManager;
import androidx.recyclerview.widget.RecyclerView;

import com.google.android.material.appbar.MaterialToolbar;
import com.google.android.material.color.MaterialColors;
import com.google.android.material.dialog.MaterialAlertDialogBuilder;
import com.google.android.material.floatingactionbutton.FloatingActionButton;

import net.pgaskin.remotedesktop.backend.Backend;
import net.pgaskin.remotedesktop.backend.Backends;

import java.util.ArrayList;
import java.util.HashMap;
import java.util.HashSet;
import java.util.List;
import java.util.Map;
import java.util.Set;

/**
 * The home screen: one saved connection per item, plus the add button and the
 * bar's actions.
 *
 * <p>An item is a card or a row, chosen from the bar and remembered. Cards let
 * the user recognise a machine by its screen; rows make it easier to find one
 * among twenty, since a name and address on their own line are easier to read.
 * Both use the same bind (the two layouts share their ids), so the mode only
 * decides the picture size and whether the items are joined into a group of
 * rows.
 */
public final class HomeActivity extends AppCompatActivity {

    private RecyclerView list;
    private View empty;
    private MaterialToolbar toolbar;
    private FloatingActionButton add;
    private final Adapter adapter = new Adapter();

    /** Rows rather than cards. Persisted, because it is a preference and not a mode. */
    private boolean listView;

    /**
     * The bar's own height, without the status bar: the theme's
     * {@code actionBarSize}, read from the layout rather than resolved again
     * here, since every other header uses that same attribute and Material 3's
     * value differs from appcompat's.
     */
    private int barHeight;

    /** The editor, while one is open; a dialog does not survive a rotation. */
    private ConnectionEditorPanel editor;

    /**
     * The reorder sheet, likewise. Unlike the editor's half-typed form, none of
     * its state needs saving across a rotation: it reads the saved connections
     * and writes each change immediately, so the bundle only records that it
     * was open.
     */
    private PinnedOrderPanel reorder;

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        WindowCompat.setDecorFitsSystemWindows(getWindow(), false);
        setContentView(R.layout.activity_home);
        toolbar = findViewById(R.id.toolbar);
        list = findViewById(R.id.list);
        empty = findViewById(R.id.empty);
        add = findViewById(R.id.add);
        add.setOnClickListener(v -> addConnection());
        empty.findViewById(R.id.playground).setOnClickListener(
                v -> startActivity(new Intent(this, PlaygroundActivity.class)));

        listView = AppSettings.listView(this);
        barHeight = toolbar.getLayoutParams().height;
        final GridLayoutManager grid = new GridLayoutManager(this, spanCount());
        // A plugin's card is a notice rather than a list item, so it takes the
        // whole width regardless of the view mode and column count.
        grid.setSpanSizeLookup(new GridLayoutManager.SpanSizeLookup() {
            @Override
            public int getSpanSize(int position) {
                return position < adapter.cards.size() ? grid.getSpanCount() : 1;
            }
        });
        list.setLayoutManager(grid);
        // Every change here is a full reload, and the only visible one is the
        // switch between cards and rows, where an animation morphing one into
        // the other would only distract.
        list.setItemAnimator(null);
        list.setAdapter(adapter);
        liftToolbarOnScroll();
        applyInsets();
        setUpToolbar();

        // An editor open across a rotation: the dialog was destroyed with the
        // window, and the form's contents are in the bundle.
        final Bundle sheet = savedInstanceState == null
                ? null : savedInstanceState.getBundle("editor");
        if (sheet != null) {
            editor = ConnectionEditorPanel.restore(this, sheet, adapter::reload);
        }
        if (savedInstanceState != null && savedInstanceState.getBoolean("reorder")) {
            reorderPinned();
        }
    }

    @Override
    protected void onSaveInstanceState(Bundle out) {
        super.onSaveInstanceState(out);
        if (editor != null && editor.isShowing()) {
            out.putBundle("editor", editor.saveState());
        }
        out.putBoolean("reorder", reorder != null && reorder.isShowing());
    }

    @Override
    protected void onDestroy() {
        // A dialog outliving its activity is a leaked window, and this screen
        // is recreated on every rotation.
        if (editor != null) {
            editor.dismiss();
            editor = null;
        }
        if (reorder != null) {
            reorder.dismiss();
            reorder = null;
        }
        super.onDestroy();
    }

    @Override
    protected void onResume() {
        super.onResume();
        adapter.reload();
    }

    /**
     * Updates the connected badges while this screen is visible. A session can
     * start or end while the list is shown (in the window beside this one, or
     * from the notification), and the card would otherwise keep showing a
     * connection that ended.
     *
     * <p>Posted rather than applied immediately: this can arrive during a
     * scroll, and a RecyclerView must not be notified of changes during
     * layout.
     */
    private final Sessions.Watcher sessionWatcher =
            () -> list.post(adapter::notifyDataSetChanged);

    @Override
    protected void onStart() {
        super.onStart();
        Sessions.addWatcher(sessionWatcher);
    }

    @Override
    protected void onStop() {
        Sessions.removeWatcher(sessionWatcher);
        super.onStop();
    }

    /**
     * As many columns of about 240 dp as fit, and never fewer than two: a
     * roughly square card is scanned rather than read, and one per row on a
     * phone would be a very large button.
     *
     * <p>240 rather than 190, because with rounding, 190 gives a third column
     * from 475 dp, which several ordinary phones are, and three cards across a
     * phone are too small to show the desktop. A third column needs 600 dp (a
     * tablet or an unfolded foldable); until then the cards grow.
     *
     * <p>A row is read left to right, so it needs twice the width and can be
     * the only thing across a phone: 480 dp gives a second column only on a
     * tablet. A single 900 dp row with a name at one end and a badge at the
     * other would no longer read as a list.
     */
    private int spanCount() {
        final float dp = getResources().getDisplayMetrics().widthPixels
                / getResources().getDisplayMetrics().density;
        return listView ? Math.max(1, Math.round(dp / 480f))
                : Math.max(2, Math.round(dp / 240f));
    }

    /**
     * The padding either side of the list: 8 dp for cards, which add their own
     * 8 dp margin, and zero for rows, which get the same 16 dp group inset as
     * every other group of rows in this app.
     */
    private int listSidePadding() {
        return listView ? 0 : Math.round(8 * getResources().getDisplayMetrics().density);
    }

    /** 8 dp under the bar for rows; a card's own margin provides it for cards. */
    private int listTopPadding() {
        return listView ? Math.round(8 * getResources().getDisplayMetrics().density) : 0;
    }

    /**
     * The list scrolls behind the bar, so its padding, not the layout, keeps the
     * first card clear of it.
     */
    private void applyInsets() {
        ViewCompat.setOnApplyWindowInsetsListener(findViewById(R.id.root), (v, insets) -> {
            final Insets bars = insets.getInsets(WindowInsetsCompat.Type.systemBars());
            // The bar's own end padding, because menu items are not content: a
            // toolbar's content inset moves the title but not the buttons, which
            // are laid out from its padding. 8 dp mirrors the title's inset for
            // the last icon, whose cell is 12 dp wider than the glyph on each
            // side.
            final int menuInset = Math.round(8 * getResources().getDisplayMetrics().density);
            toolbar.setPadding(bars.left, bars.top, bars.right + menuInset, 0);
            toolbar.getLayoutParams().height = bars.top + barHeight;
            // The FAB sits over the list, so the bottom padding includes its
            // height and margins as well as the navigation bar; otherwise the
            // button would cover the last item.
            final int fab = Math.round(72 * getResources().getDisplayMetrics().density);
            list.setPadding(listSidePadding(),
                    toolbar.getLayoutParams().height + listTopPadding(),
                    listSidePadding(), bars.bottom + fab);
            final ViewGroup.MarginLayoutParams lp =
                    (ViewGroup.MarginLayoutParams) add.getLayoutParams();
            final int margin = Math.round(16 * getResources().getDisplayMetrics().density);
            lp.bottomMargin = bars.bottom + margin;
            lp.rightMargin = bars.right + margin;
            lp.leftMargin = bars.left + margin;
            add.setLayoutParams(lp);
            return insets;
        });
    }

    /**
     * The bar gets its shadow as soon as anything is behind it. Its background
     * is the window's, so at rest nothing needs separating, and once a card has
     * scrolled under it, the shadow is the only sign of it.
     */
    private void liftToolbarOnScroll() {
        final float lifted = getResources().getDisplayMetrics().density * 4f;
        list.addOnScrollListener(new RecyclerView.OnScrollListener() {
            @Override
            public void onScrolled(@NonNull RecyclerView v, int dx, int dy) {
                toolbar.setElevation(v.canScrollVertically(-1) ? lifted : 0f);
            }
        });
    }

    // ---- the toolbar --------------------------------------------------------

    private void setUpToolbar() {
        toolbar.setNavigationIcon(null);
        toolbar.setTitle(R.string.app_name);
        toolbar.setBackgroundColor(MaterialColors.getColor(toolbar,
                com.google.android.material.R.attr.colorSurfaceContainer));
        toolbar.getMenu().clear();
        toolbar.inflateMenu(R.menu.home);
        showViewMode();
        toolbar.setOnMenuItemClickListener(item -> {
            final int id = item.getItemId();
            if (id == R.id.action_view_mode) {
                toggleViewMode();
                return true;
            }
            if (id == R.id.action_reorder) {
                reorderPinned();
                return true;
            }
            if (id == R.id.action_settings) {
                startActivity(new Intent(this, SettingsActivity.class));
                return true;
            }
            return false;
        });
    }

    /** The button shows and is titled with the mode it switches to. */
    private void showViewMode() {
        final MenuItem item = toolbar.getMenu().findItem(R.id.action_view_mode);
        item.setIcon(listView ? R.drawable.ic_view_grid : R.drawable.ic_view_list);
        item.setTitle(listView ? R.string.home_view_grid : R.string.home_view_list);
    }

    /**
     * Reordering is offered once there are at least two pinned connections. One
     * has no order, and with none the sheet would be empty; an item that is
     * always shown but usually does nothing is worse than one that appears when
     * it is useful.
     *
     * <p>Counted from the front of the list, since {@link Connections#all}
     * sorts pinned connections first.
     */
    private void showReorder() {
        int pinned = 0;
        for (Connection c : adapter.items) {
            if (!c.pinned()) {
                break;
            }
            pinned++;
        }
        toolbar.getMenu().findItem(R.id.action_reorder).setVisible(pinned > 1);
    }

    /**
     * Switches between cards and rows, which rebinds everything: the item type
     * and the span count both change.
     *
     * <p>So the first visible item is recorded and scrolled back to afterwards.
     * A full rebind starts the list at the top, which would lose the
     * connection the user was looking at.
     */
    // Nothing more specific applies: every item's view type changes, so no bound
    // view survives the switch.
    @SuppressLint("NotifyDataSetChanged")
    private void toggleViewMode() {
        final GridLayoutManager grid = (GridLayoutManager) list.getLayoutManager();
        final int firstVisible = grid.findFirstVisibleItemPosition();
        listView = !listView;
        AppSettings.setListView(this, listView);
        showViewMode();
        grid.setSpanCount(spanCount());
        list.setPadding(listSidePadding(),
                toolbar.getLayoutParams().height + listTopPadding(),
                listSidePadding(), list.getPaddingBottom());
        adapter.notifyDataSetChanged();
        if (firstVisible != RecyclerView.NO_POSITION) {
            grid.scrollToPosition(firstVisible);
        }
    }

    // ---- actions ------------------------------------------------------------

    private void addConnection() {
        if (Backends.ids().isEmpty()) {
            new MaterialAlertDialogBuilder(this, R.style.ThemeOverlay_RemoteDesktop_Dialog)
                    .setMessage(R.string.session_no_backend)
                    .setPositiveButton(android.R.string.ok, null)
                    .show();
            return;
        }
        editConnection(null);
    }

    /**
     * The editor is a dialog over this screen rather than a separate activity,
     * so a save has to be reflected in the list behind it directly rather than
     * on {@link #onResume}, which a dialog does not trigger.
     */
    private void editConnection(String id) {
        editor = ConnectionEditorPanel.show(this, id, adapter::reload);
    }

    /**
     * The pinned connections in a sheet for reordering them by drag (see
     * {@link PinnedOrderPanel} for why the drag is not on the cards
     * themselves). The list behind it reloads on every write, since it is the
     * list being rearranged.
     */
    private void reorderPinned() {
        reorder = PinnedOrderPanel.show(this, adapter::reload);
    }

    /**
     * Opens the session in its own window and closes this one.
     *
     * <p>The list is only an entry point: left open behind the session, the
     * launcher icon would return to whichever window is already open, and
     * choosing a second machine would need a way back out of the first.
     * Finishing it means the icon always opens the list, and since a session's
     * window is a document task, the list opened from it can start a second
     * session beside the first rather than replacing it.
     *
     * <p>The task is removed along with the activity: a finished activity
     * leaves its task in the recents list, where it would remain as an empty
     * window.
     *
     * <p>The intent comes from {@link SessionActivity#intentFor}, shared rather
     * than built here because a launcher shortcut opens the same connection and
     * the two must be the same document.
     */
    private void openConnection(Connection conn) {
        startActivity(SessionActivity.intentFor(this, conn.id()));
        finishAndRemoveTask();
    }

    // ---- the list -----------------------------------------------------------

    private final class Adapter extends RecyclerView.Adapter<RecyclerView.ViewHolder> {

        private final List<Connection> items = new ArrayList<>();
        /**
         * The plugin notices, above the connections. Recomputed with the list,
         * since any of them can change while the app is in the background, as
         * when a plugin is uninstalled from its card.
         */
        private final List<Plugins.Card> cards = new ArrayList<>();
        /**
         * Decoded previews, by connection id, each with the
         * {@link Connections#previewVersion} it was decoded at.
         *
         * <p>A preview is an encrypted PNG, so each costs a keystore round trip
         * and a decode, done off the main thread and cached, because a grid
         * rebinds whenever it is scrolled and the list survives leaving this
         * screen. The version keeps the cache correct: returning from a session
         * re-decodes only that session's card.
         *
         * <p>A null bitmap is a cached result rather than a miss (many
         * connections have no preview), so this holds the pair rather than the
         * bitmap.
         */
        private final Map<String, Preview> previews = new HashMap<>();

        /** @param bitmap null for a connection with no preview taken */
        private record Preview(int version, Bitmap bitmap) {
        }

        /** Previews being decoded, so each is requested only once. */
        private final Set<String> decoding = new HashSet<>();

        // Deliberately notifyDataSetChanged, twice: a card's content comes from
        // three independent sources (the record, the live session and the
        // preview), and a diff cannot account for that.
        @SuppressLint("NotifyDataSetChanged")
        void reload() {
            items.clear();
            items.addAll(Connections.all(HomeActivity.this));
            // Based on the connections alone: "no connections" is still true
            // with a plugin card above it, and setting up a plugin is often
            // the first thing to do.
            empty.setVisibility(items.isEmpty() ? View.VISIBLE : View.GONE);
            // Whether there is an order to change depends on the list, so it is
            // updated whenever the list is read.
            showReorder();
            notifyDataSetChanged();
            // The plugin cards arrive asynchronously: checking whether a plugin
            // is set up can mean hashing a library, and the connections should
            // not wait for that. Where the answers are cached it is this frame.
            Plugins.cards(HomeActivity.this, found -> {
                cards.clear();
                cards.addAll(found);
                notifyDataSetChanged();
            });
        }

        /**
         * This connection's preview if it is decoded and current, else null,
         * which is also the result for a connection with no preview, so the
         * card draws the placeholder in both cases.
         */
        private Bitmap preview(String id) {
            final int version = Connections.previewVersion(id);
            final Preview have = previews.get(id);
            if (have != null && have.version() == version) {
                return have.bitmap();
            }
            if (decoding.add(id)) {
                Connections.readThumbnail(HomeActivity.this, id, bmp -> {
                    decoding.remove(id);
                    previews.put(id, new Preview(version, bmp));
                    // Only this card rather than the whole list: a decode that
                    // finishes during a scroll should not rebind every other
                    // card.
                    final int at = indexOf(id);
                    if (at >= 0) {
                        notifyItemChanged(cards.size() + at);
                    }
                });
            }
            return null;
        }

        private int indexOf(String id) {
            for (int i = 0; i < items.size(); i++) {
                if (items.get(i).id().equals(id)) {
                    return i;
                }
            }
            return -1;
        }

        /**
         * The view mode is a property of the screen rather than of an item, but
         * it is still the view type, so that a recycled card view can never be
         * bound as a row.
         */
        @Override
        public int getItemViewType(int position) {
            if (position < cards.size()) {
                return 2;
            }
            return listView ? 1 : 0;
        }

        @NonNull
        @Override
        public RecyclerView.ViewHolder onCreateViewHolder(@NonNull ViewGroup parent, int viewType) {
            final LayoutInflater inflater = LayoutInflater.from(parent.getContext());
            if (viewType == 2) {
                return new PluginHolder(inflater.inflate(R.layout.item_plugin, parent, false));
            }
            return new Holder(inflater.inflate(
                    viewType == 1 ? R.layout.item_connection_row : R.layout.item_connection,
                    parent, false));
        }

        @Override
        public void onBindViewHolder(@NonNull RecyclerView.ViewHolder holder, int position) {
            if (holder instanceof PluginHolder p) {
                bindCard(p, position, cards.get(position));
                return;
            }
            final Holder h = (Holder) holder;
            final int index = position - cards.size();
            final Connection c = items.get(index);
            h.title.setText(c.title());
            h.subtitle.setText(c.subtitle());
            // INVISIBLE, not GONE: a connection with no name has one line and
            // the one beside it may have two, and cards of different heights in
            // one grid row would leave a ragged edge.
            h.subtitle.setVisibility(c.subtitle().isEmpty() ? View.INVISIBLE : View.VISIBLE);

            // Hidden when there is only one backend: a label that is the same
            // on every card adds nothing.
            final String backend = Backends.shortName(c.backendId());
            h.badge.setText(backend);
            h.badge.setVisibility(Backends.ids().size() > 1 && !backend.isEmpty()
                    ? View.VISIBLE : View.GONE);

            final Bitmap thumb = preview(c.id());
            if (thumb != null) {
                h.preview.setImageBitmap(thumb);
                h.preview.setImageTintList(null);
                h.preview.setScaleType(ImageView.ScaleType.CENTER_CROP);
            } else {
                // No preview yet: the band's own colour and one faint icon at
                // its natural size in the middle. CENTER, not the CENTER_CROP a
                // preview uses: cropping the glyph to fill the band leaves two
                // grey bars that are not recognisable as a computer.
                h.preview.setImageResource(R.drawable.ic_desktop);
                h.preview.setImageTintList(ColorStateList.valueOf(
                        MaterialColors.compositeARGBWithAlpha(
                                MaterialColors.getColor(h.preview,
                                        com.google.android.material.R.attr.colorOnSurface),
                                (int) (0.20f * 255))));
                h.preview.setScaleType(ImageView.ScaleType.CENTER);
            }

            // The only thing on a card about the current state rather than the
            // saved connection. Tinted here rather than in the drawable so it
            // uses the theme's accent with the device's dynamic colours, and so
            // the other two badges keep sharing one drawable.
            final Session live = Sessions.byKey(Sessions.keyFor(c.id()));
            if (live != null && live.alive()) {
                h.connected.setText(live.state() == Backend.State.CONNECTED
                        ? R.string.home_connected : R.string.session_connecting);
                h.connected.setBackgroundTintList(ColorStateList.valueOf(
                        MaterialColors.getColor(h.connected,
                                androidx.appcompat.R.attr.colorPrimary)));
                h.connected.setTextColor(MaterialColors.getColor(h.connected,
                        com.google.android.material.R.attr.colorOnPrimary));
                h.connected.setVisibility(View.VISIBLE);
            } else {
                h.connected.setVisibility(View.GONE);
            }

            h.pin.setVisibility(c.pinned() ? View.VISIBLE : View.GONE);

            if (listView) {
                groupRow(h.card, index);
            }

            h.card.setOnClickListener(v -> openConnection(c));
            // Straight to the editor rather than a selection mode: edit, delete
            // and pin are all in the editor, so such a mode would only be an
            // extra step in front of it.
            h.card.setOnLongClickListener(v -> {
                editConnection(c.id());
                return true;
            });
        }

        /**
         * One plugin's card: the plugin, what is wrong or missing, the error it
         * reported, and the one or two actions for it. Uninstall is on every
         * card about a package; the restart card is about all plugins and has
         * the restart button instead.
         */
        private void bindCard(PluginHolder h, int position, Plugins.Card card) {
            spaceCard(h.itemView, position);
            h.title.setText(card.title());
            h.message.setText(card.message());
            h.detail.setText(card.detail());
            h.detail.setVisibility(card.detail() == null || card.detail().isEmpty()
                    ? View.GONE : View.VISIBLE);
            h.setUp.setVisibility(card.kind() == Plugins.Kind.SETUP ? View.VISIBLE : View.GONE);
            h.restart.setVisibility(card.kind() == Plugins.Kind.RESTART
                    ? View.VISIBLE : View.GONE);
            h.update.setVisibility(card.updateUrl() == null ? View.GONE : View.VISIBLE);
            h.uninstall.setVisibility(card.packageName() == null ? View.GONE : View.VISIBLE);
            h.update.setOnClickListener(v -> Plugins.update(HomeActivity.this, card.updateUrl()));
            h.setUp.setOnClickListener(v -> Plugins.setup(HomeActivity.this, card.backendId()));
            h.restart.setOnClickListener(v -> Plugins.restart(HomeActivity.this));
            h.uninstall.setOnClickListener(
                    v -> Plugins.uninstall(HomeActivity.this, card.packageName()));
        }

        @Override
        public int getItemCount() {
            return cards.size() + items.size();
        }

        /**
         * A plugin card's margins, which have to suit both view modes.
         *
         * <p>It spans the full width in either mode, so it has to line up with
         * both: a row is inset 16 dp with the list flush to the screen, and a
         * card has its own 8 dp margin with the list padded by 8. The inset is
         * therefore the group inset minus the list's padding, 16 dp either way.
         *
         * <p>The vertical gap is the one between two groups of rows, since a
         * plugin card is a notice rather than a list item. Above the first and
         * below each one, minus what the neighbour already provides: the
         * list's own top padding, and a connection card's own margin in card
         * mode.
         */
        private void spaceCard(View card, int position) {
            final int inset = getResources().getDimensionPixelSize(R.dimen.group_inset);
            final int spacing = getResources().getDimensionPixelSize(R.dimen.group_spacing);
            final int own = getResources().getDimensionPixelSize(R.dimen.card_margin);
            final ViewGroup.MarginLayoutParams lp =
                    (ViewGroup.MarginLayoutParams) card.getLayoutParams();
            lp.setMarginStart(inset - listSidePadding());
            lp.setMarginEnd(inset - listSidePadding());
            lp.topMargin = position == 0 ? spacing - listTopPadding() : 0;
            lp.bottomMargin = position == cards.size() - 1 && !listView
                    ? spacing - own : spacing;
            card.setLayoutParams(lp);
        }

        /**
         * A row's place in the group formed by the whole list: its background,
         * its inset and the gap under it, as for a settings row.
         *
         * <p>The ends are grid rows rather than items, since a wide enough
         * screen puts two rows side by side: the first span's worth are the top
         * of the group, and the items in the last grid row are its bottom.
         */
        private void groupRow(View row, int position) {
            final int span = ((GridLayoutManager) list.getLayoutManager()).getSpanCount();
            final int last = items.size() - 1;
            final boolean bottom = position >= last - last % span;
            RowGroups.row(row, position < span, bottom, bottom ? 0 : RowGroups.gap(row));
        }
    }

    /**
     * One item, in either view mode: the two layouts have the same ids so that
     * one bind fills both, and the live badge, the part most likely to diverge,
     * is written once.
     */
    private static final class Holder extends RecyclerView.ViewHolder {
        final View card;
        final ImageView preview;
        final ImageView pin;
        final TextView badge;
        final TextView connected;
        final TextView title;
        final TextView subtitle;

        Holder(View v) {
            super(v);
            card = v.findViewById(R.id.card);
            preview = v.findViewById(R.id.preview);
            pin = v.findViewById(R.id.pin);
            badge = v.findViewById(R.id.badge);
            connected = v.findViewById(R.id.connected);
            title = v.findViewById(R.id.title);
            subtitle = v.findViewById(R.id.subtitle);
        }
    }

    /** A plugin's card, which shares only the list with a connection's. */
    private static final class PluginHolder extends RecyclerView.ViewHolder {
        final TextView title;
        final TextView message;
        final TextView detail;
        final View setUp;
        final View restart;
        final View update;
        final View uninstall;

        PluginHolder(View v) {
            super(v);
            title = v.findViewById(R.id.title);
            message = v.findViewById(R.id.message);
            detail = v.findViewById(R.id.detail);
            setUp = v.findViewById(R.id.setup);
            restart = v.findViewById(R.id.restart);
            update = v.findViewById(R.id.update);
            uninstall = v.findViewById(R.id.uninstall);
        }
    }
}
