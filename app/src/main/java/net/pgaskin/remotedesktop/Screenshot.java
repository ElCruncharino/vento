// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop;

import android.app.Activity;
import android.content.ClipData;
import android.content.Context;
import android.content.Intent;
import android.graphics.Bitmap;
import android.net.Uri;
import android.os.Handler;
import android.os.Looper;
import android.util.Log;
import android.widget.Toast;

import androidx.core.content.FileProvider;

import net.pgaskin.remotedesktop.backend.Backend;

import java.io.File;
import java.io.FileOutputStream;
import java.io.IOException;
import java.time.LocalDateTime;
import java.time.format.DateTimeFormatter;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;

/**
 * A screenshot of the desktop, sent to the share sheet.
 *
 * <p>It captures the whole desktop at the far end's size, not the phone's view
 * of it (a zoomed viewport onto part of it, with a cursor drawn by this app),
 * which would show the client rather than the remote machine.
 *
 * <p>No backend changes are needed for this: {@link Backend#thumbnail} already
 * returns the whole framebuffer scaled to fit a bound, so passing the desktop's
 * own size gives a scale of one. The home card preview uses the same call.
 *
 * <p>The file is written to this app's cache and shared as a read grant on one
 * URI, so only the chosen app can read it: not the gallery, the media scanner,
 * or any other app. The grant ends with the receiving task, and the file is
 * deleted the next time this runs.
 */
final class Screenshot {

    private static final String TAG = "Screenshot";

    /** Under the cache, and the only directory {@code xml/shared_files} exposes. */
    private static final String DIR = "screenshots";

    /** Sortable and locale-independent, since it goes in a file name. */
    private static final DateTimeFormatter STAMP =
            DateTimeFormatter.ofPattern("yyyyMMdd-HHmmss");

    /**
     * A background thread. A 4K desktop is 33 MB of bitmap and PNG encoding is
     * slow; on the main thread it would freeze the UI, on the screen where a
     * stall looks like a dropped connection.
     */
    private static final ExecutorService writer = Executors.newSingleThreadExecutor(r -> {
        final Thread t = new Thread(r, "screenshot-writer");
        t.setDaemon(true);
        return t;
    });

    private static final Handler main = new Handler(Looper.getMainLooper());

    private Screenshot() {
    }

    /**
     * Capture the session and open the share sheet on it. Returns at once; the
     * encoding runs on the writer thread and the sheet opens when it is done.
     *
     * <p>The capture runs here rather than on the writer thread because the
     * {@link Backend} contract only promises calls from the main thread, which
     * is also where the rest of the panel calls it.
     */
    static void share(Activity activity, Session session) {
        final Bitmap shot = capture(session);
        if (shot == null) {
            failed(activity);
            return;
        }
        final String name = fileName(session);
        // The application context, so an encode that outlives the activity does
        // not keep it alive.
        final Context app = activity.getApplicationContext();
        writer.execute(() -> {
            final Uri uri = write(app, shot, name);
            shot.recycle();
            main.post(() -> {
                // The session screen can be closed while the screenshot is
                // still being encoded.
                if (activity.isFinishing() || activity.isDestroyed()) {
                    return;
                }
                if (uri == null) {
                    failed(activity);
                } else {
                    send(activity, uri, name);
                }
            });
        });
    }

    /**
     * The framebuffer at 1:1, or null if there is nothing to capture (the
     * session is still connecting, or has closed since the panel opened).
     *
     * <p>Every backend's {@code thumbnail} derives its scale step from the bound
     * it is given, so the desktop's own size gives a step of one regardless of
     * the protocol.
     */
    private static Bitmap capture(Session session) {
        if (session.isClosed()) {
            return null;
        }
        final Backend backend = session.backend();
        final int w = backend.desktopWidth();
        final int h = backend.desktopHeight();
        if (w <= 0 || h <= 0) {
            return null;
        }
        try {
            return session.thumbnail(w, h);
        } catch (OutOfMemoryError e) {
            // A desktop at 1:1 is four bytes per pixel and a phone's heap is
            // small: 3840x2400 is 35 MB in one allocation, and not catching it
            // would crash the session. Caught rather than prevented by a size
            // limit because the available heap cannot be computed in advance.
            Log.w(TAG, "not enough memory for a " + w + "x" + h + " screenshot", e);
            return null;
        }
    }

    /**
     * Write the screenshot, deleting the previous one: a share hands a copy to
     * another app, so the file left here is not meant to be kept. It is cleared
     * before the next write rather than after sharing because the receiving app
     * may not have read it yet.
     */
    private static Uri write(Context ctx, Bitmap shot, String name) {
        final File dir = new File(ctx.getCacheDir(), DIR);
        clear(dir);
        if (!dir.isDirectory() && !dir.mkdirs()) {
            Log.w(TAG, "could not create " + dir);
            return null;
        }
        final File file = new File(dir, name);
        try (FileOutputStream out = new FileOutputStream(file)) {
            // PNG rather than JPEG: a desktop is mostly text, which chroma
            // subsampling blurs.
            if (!shot.compress(Bitmap.CompressFormat.PNG, 100, out)) {
                Log.w(TAG, "could not encode the screenshot");
                file.delete();
                return null;
            }
        } catch (IOException e) {
            Log.w(TAG, "writing the screenshot", e);
            file.delete();
            return null;
        }
        try {
            return FileProvider.getUriForFile(ctx, ctx.getPackageName() + ".files", file);
        } catch (IllegalArgumentException e) {
            // The provider paths in xml/shared_files do not include this
            // directory, which is a build mistake rather than a runtime state.
            Log.w(TAG, "the screenshot is not under a shared path", e);
            file.delete();
            return null;
        }
    }

    private static void clear(File dir) {
        final File[] old = dir.listFiles();
        if (old != null) {
            for (File f : old) {
                if (!f.delete()) {
                    Log.w(TAG, "could not delete " + f);
                }
            }
        }
    }

    /**
     * Open the share sheet, without a title: the system has not shown one since
     * Android 13. It shows a preview of the image instead, which is why the URI
     * is set in the clip data as well as the extra rather than relying on the
     * system to copy it there.
     */
    private static void send(Activity activity, Uri uri, String name) {
        final Intent send = new Intent(Intent.ACTION_SEND);
        send.setType("image/png");
        send.putExtra(Intent.EXTRA_STREAM, uri);
        send.setClipData(ClipData.newUri(activity.getContentResolver(), name, uri));
        send.addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION);
        activity.startActivity(Intent.createChooser(send, null));
    }

    /**
     * The file name: the connection title and the time. The desktop name is not
     * used even though the panel has one, because it comes from the far end and
     * could contain anything.
     */
    private static String fileName(Session session) {
        final StringBuilder b = new StringBuilder();
        final String title = session.title();
        for (int i = 0; title != null && i < title.length() && b.length() < 40; i++) {
            final char c = title.charAt(i);
            if (c >= 'a' && c <= 'z' || c >= 'A' && c <= 'Z' || c >= '0' && c <= '9') {
                b.append(c);
            } else if (b.length() > 0 && b.charAt(b.length() - 1) != '-') {
                b.append('-');
            }
        }
        while (b.length() > 0 && b.charAt(b.length() - 1) == '-') {
            b.setLength(b.length() - 1);
        }
        if (b.length() == 0) {
            b.append("desktop");
        }
        return b + "-" + STAMP.format(LocalDateTime.now()) + ".png";
    }

    private static void failed(Activity activity) {
        Toast.makeText(activity, R.string.screenshot_failed, Toast.LENGTH_SHORT).show();
    }
}
