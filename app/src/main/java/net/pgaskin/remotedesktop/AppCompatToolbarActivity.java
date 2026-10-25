// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop;

import android.os.Bundle;
import android.view.View;
import android.view.ViewGroup;
import android.widget.TextView;

import androidx.appcompat.app.AppCompatActivity;
import androidx.core.graphics.Insets;
import androidx.core.view.ViewCompat;
import androidx.core.view.WindowCompat;
import androidx.core.view.WindowInsetsCompat;
import androidx.fragment.app.Fragment;

/**
 * A header over a fragment, with the system bars kept out of both.
 *
 * <p>The frame of the settings tree and the licence page. A settings screen has
 * no Save button because every row takes effect immediately; the connection
 * editor is a form with a Save button, so it is a dialog instead.
 */
public abstract class AppCompatToolbarActivity extends AppCompatActivity {

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        WindowCompat.setDecorFitsSystemWindows(getWindow(), false);
        setContentView(R.layout.activity_preferences);
        findViewById(R.id.back).setOnClickListener(
                v -> getOnBackPressedDispatcher().onBackPressed());
        setTitle(getTitle());
        ViewCompat.setOnApplyWindowInsetsListener(findViewById(R.id.root), (v, insets) -> {
            final Insets bars = insets.getInsets(WindowInsetsCompat.Type.systemBars());
            v.setPadding(bars.left, bars.top, bars.right, bars.bottom);
            return insets;
        });
    }

    /**
     * Also shows the title in the header, which is not a toolbar. The window
     * title is still set, for the recents list.
     */
    @Override
    public void setTitle(CharSequence title) {
        super.setTitle(title);
        final TextView view = findViewById(R.id.title);
        if (view != null) {
            view.setText(title);
        }
    }

    /** Put a view in the frame under the header, in place of whatever is there. */
    protected void setContent(View view) {
        final ViewGroup frame = findViewById(R.id.content);
        frame.removeAllViews();
        frame.addView(view, new ViewGroup.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT));
    }

    /** Put a fragment in the frame under the header. */
    protected void show(Fragment fragment) {
        getSupportFragmentManager().beginTransaction()
                .replace(R.id.content, fragment)
                .commit();
    }
}
