// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop.plugin;

import android.annotation.SuppressLint;
import android.app.Activity;
import android.content.Context;
import android.content.Intent;
import android.net.ConnectivityManager;
import android.net.NetworkInfo;
import android.net.Uri;
import android.os.Bundle;
import android.provider.Settings;
import android.os.Handler;
import android.os.Looper;
import android.util.Log;
import android.graphics.Insets;
import android.view.View;
import android.view.WindowInsets;
import android.widget.Button;
import android.widget.ProgressBar;
import android.widget.TextView;

import java.io.File;
import java.io.IOException;
import java.io.InputStream;
import java.io.FileOutputStream;
import java.util.List;
import java.util.Map;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;

/**
 * The screen where an add-on obtains the libraries it holds.
 *
 * <p>It runs in the add-on's process, which is why it exists: package
 * visibility and network access are the add-on's, so the app's manifest needs
 * neither. There are two routes, both ending in the same check: an archive the
 * user picks, and one downloaded from a mirror.
 *
 * <p>A subclass supplies its explanation and the libraries it wants; how a
 * library is verified and stored is handled here.
 */
public abstract class SetupActivity extends Activity {

    private static final String TAG = "PluginSetup";

    /**
     * One acquisition at a time, for the life of the process rather than of a
     * screen.
     *
     * <p>With one worker per activity, a rotation part-way through an
     * acquisition would start a second one: {@code shutdownNow} interrupts a
     * thread blocked in a file copy without stopping it, and the recreated
     * screen offers the same buttons, so two threads could write one temporary
     * file. That could not produce an unverified file, since the hash would not
     * match, but the user would see two failures with no way to act on them.
     * The state that survives a rotation is kept in static fields here, and
     * {@link #onResume} draws the screen from it.
     */
    private static final ExecutorService WORKER = Executors.newSingleThreadExecutor(r -> {
        final Thread t = new Thread(r, "plugin-setup");
        t.setDaemon(true);
        return t;
    });
    private static final Handler MAIN = new Handler(Looper.getMainLooper());

    /**
     * The application's context, for looking up strings on the worker: a route
     * outlives a rotation, and looking up a string on a destroyed activity's
     * context can fail in ways that only show up much later.
     */
    @SuppressLint("StaticFieldLeak") // the application context, which does not leak
    private static Context strings;

    /**
     * The process's current state, all main-thread only: whether a route is
     * running, its current step, its progress, and the last finished route's
     * result. A screen is drawn from these rather than holding its own copy, so
     * it shows the right state after a rotation.
     */
    private static boolean busy;
    private static String doing;
    private static long done;
    private static long total;
    private static String failure;
    /** Whether that failure was this app being denied network access. */
    private static boolean denied;

    private TextView status;
    private TextView pins;
    private ProgressBar progress;
    private View buttons;
    private View fix;

    /** What this add-on is for, and where its library comes from. */
    protected abstract CharSequence explanation();

    /** Where a copy can be downloaded from, tried in order. */
    protected abstract List<Download.Source> downloads();

    /**
     * Name to SHA-256, for this device. Empty if there is no build for it, in
     * which case the screen offers nothing rather than showing an error.
     */
    protected abstract Map<String, String> wanted();

    /** The ABI the libraries are wanted for, or null if there is none. */
    protected abstract String abi();

    private final ActivityLauncher picker = new ActivityLauncher();

    @Override
    protected void onCreate(Bundle state) {
        super.onCreate(state);
        setContentView(R.layout.plugin_setup);
        strings = getApplicationContext();

        // The window is edge to edge and this screen has no action bar for a
        // title, so both are handled here: the manifest label at the top, and
        // the system bars kept out of the content.
        final View content = findViewById(R.id.plugin_content);
        content.setOnApplyWindowInsetsListener((v, insets) -> {
            final Insets bars = insets.getInsets(
                    WindowInsets.Type.systemBars() | WindowInsets.Type.displayCutout());
            v.setPadding(bars.left, bars.top, bars.right, bars.bottom);
            return insets;
        });
        ((TextView) findViewById(R.id.plugin_title)).setText(getTitle());
        ((TextView) findViewById(R.id.plugin_explanation)).setText(explanation());
        status = findViewById(R.id.plugin_status);
        pins = findViewById(R.id.plugin_pins);
        progress = findViewById(R.id.plugin_progress);
        buttons = findViewById(R.id.plugin_buttons);
        fix = findViewById(R.id.plugin_fix_network);
        fix.setOnClickListener(v -> offerNetwork());
        readColors();

        // No pinned build is for this phone. Every route below would fail, so
        // none is offered; the explanation stays, since it describes the add-on
        // either way.
        nothingForThisDevice = wanted().isEmpty();
        if (nothingForThisDevice) {
            buttons.setVisibility(View.GONE);
            show(getString(R.string.plugin_wrong_device), false);
            return;
        }

        pins.setText(pins());
        pins.setVisibility(View.VISIBLE);

        findViewById(R.id.plugin_from_file).setOnClickListener(v -> picker.launch(
                new Intent(Intent.ACTION_OPEN_DOCUMENT)
                        .addCategory(Intent.CATEGORY_OPENABLE)
                        .setType("*/*")));

        findViewById(R.id.plugin_from_download).setOnClickListener(v -> run(() -> {
            final File apk = File.createTempFile("download", ".apk", getCacheDir());
            try {
                requireNetwork();
                // Each host is shown as it is contacted rather than up front,
                // since which mirror answers is not known in advance.
                Download.toFile(downloads(), apk, SetupActivity::progress, new Download.Where() {
                    @Override
                    public void asking(String host) {
                        say(R.string.plugin_asking, host);
                    }

                    @Override
                    public void fetching(String host) {
                        say(R.string.plugin_downloading, host);
                    }
                });
                return take(apk);
            } finally {
                //noinspection ResultOfMethodCallIgnored
                apk.delete();
            }
        }));

        picker.onResult(uri -> run(() -> {
            final File apk = File.createTempFile("picked", ".apk", getCacheDir());
            try {
                say(R.string.plugin_reading);
                try (InputStream in = getContentResolver().openInputStream(uri);
                     FileOutputStream out = new FileOutputStream(apk)) {
                    if (in == null) {
                        throw new IOException(getString(R.string.plugin_unreadable));
                    }
                    Hashes.copy(in, out);
                }
                return take(apk);
            } finally {
                //noinspection ResultOfMethodCallIgnored
                apk.delete();
            }
        }));
    }

    /**
     * Detects a network denial that a download would otherwise report as a DNS
     * failure.
     *
     * <p>An app whose network access has been switched off cannot resolve any
     * host: every lookup fails within milliseconds with "unable to resolve
     * host", which blames DNS for an unrelated problem. Only one API still
     * reports the state: {@code getActiveNetwork} is null either way, and only
     * the deprecated {@code NetworkInfo} distinguishes being blocked from having
     * no network.
     *
     * <p>Access cannot be requested. The policy can only be changed with a
     * signature permission, so there is no request to make; the user has to
     * switch it on in Settings, which {@link #offerNetwork} opens.
     */
    @SuppressWarnings("deprecation")
    private void requireNetwork() throws IOException {
        final NetworkInfo info =
                getSystemService(ConnectivityManager.class).getActiveNetworkInfo();
        if (info != null && info.getDetailedState() == NetworkInfo.DetailedState.BLOCKED) {
            denied = true;
            throw new IOException(getString(R.string.plugin_no_network));
        }
    }

    /** Opens the Settings page with this app's network switch. */
    private void offerNetwork() {
        startActivity(new Intent(
                Settings.ACTION_IGNORE_BACKGROUND_DATA_RESTRICTIONS_SETTINGS,
                Uri.fromParts("package", getPackageName(), null)));
    }

    /** The part both routes share: extracting and checking the archive's libraries. */
    private Map<String, String> take(File apk) throws IOException {
        say(R.string.plugin_opening);
        progress(0, -1);
        // Each library is shown as it is extracted: it is hashed while it is
        // written, so the line also shows which one is being verified.
        return Apks.take(this, List.of(apk), abi(), wanted(),
                name -> say(R.string.plugin_extracting, name));
    }

    /**
     * The small print below the status line: where in the archive each library
     * is looked for, and the hash it must match.
     *
     * <p>This screen claims to keep a copy only if it is the build these
     * declarations were read from, and an unverifiable claim is worth little.
     * So the pins are shown on the screen, where a user who has unzipped and
     * hashed their own copy of the APK can compare them.
     */
    private CharSequence pins() {
        final StringBuilder sb = new StringBuilder();
        for (Map.Entry<String, String> e : wanted().entrySet()) {
            if (sb.length() > 0) {
                sb.append('\n');
            }
            sb.append("lib/").append(abi()).append('/').append(e.getKey());
            // A SHA-256 is 64 characters, too wide for a phone: split in half
            // here rather than left to wrap at an arbitrary point.
            final String sha256 = e.getValue();
            for (int at = 0; at < sha256.length(); at += 32) {
                sb.append("\n  ").append(sha256, at, Math.min(at + 32, sha256.length()));
            }
        }
        return sb;
    }

    /** No pinned build is for this phone: see {@link #onCreate}. */
    private boolean nothingForThisDevice;

    /** The screen a finished route reports to: whichever one is resumed. */
    @SuppressLint("StaticFieldLeak") // cleared in onPause
    private static SetupActivity onScreen;

    @Override
    protected void onResume() {
        super.onResume();
        if (nothingForThisDevice) {
            return;
        }
        if (LibraryStore.has(this, wanted())) {
            // Setup is already complete, possibly from an earlier visit or a
            // route that finished while this screen was being recreated.
            setResult(RESULT_OK);
            finish();
            return;
        }
        onScreen = this;
        // A rotation does not stop a running route, so this screen shows the
        // process's current state rather than starting fresh, drawn from the
        // static fields.
        working(busy);
        show(busy ? doing : failure, !busy);
        fix.setVisibility(!busy && denied ? View.VISIBLE : View.GONE);
        if (busy) {
            progress.setIndeterminate(total <= 0);
            if (total > 0) {
                progress.setMax(1000);
                progress.setProgress((int) (done * 1000 / total));
            }
        }
    }

    @Override
    protected void onPause() {
        super.onPause();
        if (onScreen == this) {
            onScreen = null;
        }
    }

    /** An acquisition route, run off the main thread. */
    private interface Route {
        /** What is still missing afterwards; empty means done. */
        Map<String, String> run() throws Exception;
    }

    private void run(Route route) {
        busy = true;
        failure = null;
        denied = false;
        doing = null;
        done = 0;
        total = -1;
        working(true);
        WORKER.execute(() -> {
            String said = null;
            try {
                final Map<String, String> missing = route.run();
                if (!missing.isEmpty()) {
                    said = strings.getString(R.string.plugin_not_in_it);
                }
            } catch (Throwable t) {
                Log.w(TAG, "acquiring: " + t, t);
                // The exception's message if it has one: this is how "not the
                // build this plugin was written for" reaches the user. Otherwise
                // (a failed socket often has no useful message) the generic
                // one, since a class name on screen does not help.
                final String message = t.getMessage();
                said = message == null || message.isEmpty()
                        ? strings.getString(R.string.plugin_failed) : message;
            }
            final String result = said;
            MAIN.post(() -> {
                busy = false;
                failure = result;
                doing = null;
                // The currently resumed screen, which after a rotation is not
                // the one that started this, and after a kill is none.
                final SetupActivity host = onScreen;
                if (host == null) {
                    return;
                }
                host.working(false);
                if (result == null) {
                    host.setResult(RESULT_OK);
                    host.finish();
                } else {
                    host.show(result, false);
                    host.fix.setVisibility(denied ? View.VISIBLE : View.GONE);
                }
            });
        });
    }

    /**
     * How long the worker pauses before and after showing each step.
     *
     * <p>Most steps are quick (an archive already in the cache is opened and
     * both libraries extracted in well under a second), and a line shown too
     * briefly cannot be read. Half a second before and after keeps each step on
     * screen for at least a second.
     */
    private static final long LINGER = 500;

    /**
     * Shows the route's current step on the line above the progress bar.
     * Called from the worker, so it goes through the field a rotation redraws
     * from, and it deliberately blocks the worker for {@link #LINGER} before and
     * after, which is why it is never called from the main thread.
     */
    private static void say(int what, Object... args) {
        final String message = strings.getString(what, args);
        linger();
        MAIN.post(() -> {
            doing = message;
            final SetupActivity host = onScreen;
            if (host != null && busy) {
                host.show(message, true);
            }
        });
        linger();
    }

    private static void linger() {
        try {
            Thread.sleep(LINGER);
        } catch (InterruptedException e) {
            // Nothing interrupts this worker, but if a route is being stopped it
            // should stop rather than sleep again.
            Thread.currentThread().interrupt();
        }
    }

    /**
     * Download progress, when the server reported a size. A negative total means
     * it did not, and the bar stays indeterminate.
     */
    private static void progress(long at, long of) {
        MAIN.post(() -> {
            done = at;
            total = of;
            final SetupActivity host = onScreen;
            if (host == null || !busy) {
                return;
            }
            host.progress.setIndeterminate(of <= 0);
            if (of > 0) {
                host.progress.setMax(1000);
                host.progress.setProgress((int) (at * 1000 / of));
            }
        });
    }

    /**
     * The single status line, in one of two colours: progress in ordinary text
     * and failures as an error. Only one is ever shown.
     */
    private void show(String message, boolean working) {
        if (message == null || message.isEmpty()) {
            status.setVisibility(View.GONE);
            return;
        }
        status.setTextColor(working ? ordinaryColor : errorColor);
        status.setText(message);
        status.setVisibility(View.VISIBLE);
    }

    /** Both from the theme; the layout sets the error colour. */
    private int errorColor;
    private int ordinaryColor;

    private void readColors() {
        errorColor = status.getCurrentTextColor();
        final android.util.TypedValue v = new android.util.TypedValue();
        getTheme().resolveAttribute(android.R.attr.textColorSecondary, v, true);
        ordinaryColor = v.resourceId != 0 ? getColor(v.resourceId) : v.data;
    }

    private void working(boolean busy) {
        progress.setVisibility(busy ? View.VISIBLE : View.GONE);
        buttons.setVisibility(busy ? View.GONE : View.VISIBLE);
        if (busy) {
            progress.setIndeterminate(true);
        }
    }

    /**
     * The document picker, without androidx: one request code and one callback.
     * A result survives the activity being recreated behind the picker because
     * {@link #onResume} checks the store again rather than trusting a field.
     */
    private final class ActivityLauncher {

        private static final int REQUEST = 1;

        private java.util.function.Consumer<Uri> onResult;

        void launch(Intent intent) {
            startActivityForResult(intent, REQUEST);
        }

        void onResult(java.util.function.Consumer<Uri> handler) {
            onResult = handler;
        }

        void deliver(int request, int result, Intent data) {
            if (request == REQUEST && result == RESULT_OK && data != null
                    && data.getData() != null && onResult != null) {
                onResult.accept(data.getData());
            }
        }
    }

    @Override
    protected void onActivityResult(int request, int result, Intent data) {
        super.onActivityResult(request, result, data);
        picker.deliver(request, result, data);
    }
}
