// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop.plugin;

import android.util.Log;

import org.json.JSONObject;

import java.io.File;
import java.io.FileOutputStream;
import java.io.IOException;
import java.io.InputStream;
import java.net.HttpURLConnection;
import java.net.URI;
import java.net.URL;

/**
 * Downloads an archive from a third-party server, the one route an add-on
 * offers that the app could not.
 *
 * <p>Nothing here checks what arrives: the pinned hashes do that, once the
 * file is on disk. Mirrors repack archives, so an archive's own hash
 * identifies a copy rather than a build.
 */
public final class Download {

    private static final String TAG = "PluginDownload";

    /** Some hosts refuse anything that is not a browser. */
    private static final String AGENT = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 "
            + "(KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";

    /** How a source's URL turns into the URL of the file itself. */
    public enum Kind {
        /** The URL is the file. */
        DIRECT,
        /**
         * An API that returns the path to one build: Aptoide's, queried for an
         * exact version code. It is the only mirror here with an API rather
         * than a page to scrape.
         */
        APTOIDE,
    }

    public record Source(Kind kind, String url) {
    }

    private Download() {
    }

    /** Download progress, for the screen to show. A negative total is unknown. */
    public interface Progress {
        void at(long done, long total);
    }

    /**
     * The server being contacted, for the screen to show.
     *
     * <p>The user should know who is sending a ten-megabyte download, especially
     * when a mirror is tried after another one failed, since the progress bar
     * looks the same either way. A host rather than a URL, because a status line
     * is not wide enough for a URL.
     */
    public interface Where {
        /** Asking a mirror's API which file it has. */
        void asking(String host);

        /** Fetching the archive itself. */
        void fetching(String host);
    }

    /**
     * Downloads from the first source that succeeds.
     *
     * <p>Every failure is reported, not only the last: two mirrors fail for
     * different reasons, and reporting only the second hides the error from the
     * first, which is usually the one that works. Each is also logged with its
     * URL, since the message on screen cannot include one.
     */
    public static void toFile(Iterable<Source> sources, File out, Progress progress, Where where)
            throws IOException {
        final StringBuilder failures = new StringBuilder();
        for (Source source : sources) {
            try {
                // The host that is queried and the host that serves the file
                // can differ, so the second is shown once it is known rather
                // than showing the first for both.
                final String url = resolve(source, progress, where);
                where.fetching(host(url));
                fetch(url, out, progress);
                return;
            } catch (IOException e) {
                Log.w(TAG, "fetching from " + source.url(), e);
                final String said = e.getMessage();
                if (failures.length() > 0) {
                    failures.append('\n');
                }
                failures.append(said == null || said.isEmpty() ? String.valueOf(e) : said);
            }
        }
        throw new IOException(failures.length() == 0
                ? "There is nowhere to fetch it from." : failures.toString());
    }

    /** The host in a URL, or the URL itself if it has none. */
    private static String host(String url) {
        try {
            final String host = URI.create(url).getHost();
            return host == null || host.isEmpty() ? url : host;
        } catch (IllegalArgumentException e) {
            return url;
        }
    }

    private static String resolve(Source source, Progress progress, Where where)
            throws IOException {
        if (source.kind() == Kind.DIRECT) {
            return source.url();
        }
        where.asking(host(source.url()));
        final File meta = File.createTempFile("meta", ".json");
        try {
            fetch(source.url(), meta, progress);
            final String path = new JSONObject(new String(
                    java.nio.file.Files.readAllBytes(meta.toPath()), "UTF-8"))
                    .getJSONObject("data").getJSONObject("file").getString("path");
            if (path.isEmpty()) {
                throw new IOException("The download service did not say where the file is.");
            }
            return path;
        } catch (org.json.JSONException e) {
            throw new IOException("The download service did not answer the way it is documented to.", e);
        } finally {
            //noinspection ResultOfMethodCallIgnored
            meta.delete();
        }
    }

    /**
     * The maximum archive size. The viewer's own APK is 11.7 MB, so no real
     * archive comes close; this guards against a server that responds with an
     * endless stream.
     */
    private static final long MAX_BYTES = 256L * 1024 * 1024;

    /**
     * Follows redirects manually, since HttpURLConnection does not follow one
     * across schemes. That is also why each hop's scheme is checked: following
     * redirects manually accepts whatever they name, and a mirror that
     * redirects to http would send the download unencrypted.
     */
    private static void fetch(String url, File out, Progress progress) throws IOException {
        String location = url;
        for (int hop = 0; hop < 5; hop++) {
            if (!location.regionMatches(true, 0, "https://", 0, 8)) {
                throw new IOException("The download was redirected to an insecure address.");
            }
            final HttpURLConnection conn =
                    (HttpURLConnection) new URL(location).openConnection();
            conn.setRequestProperty("User-Agent", AGENT);
            conn.setInstanceFollowRedirects(false);
            conn.setConnectTimeout(30_000);
            conn.setReadTimeout(120_000);
            try {
                final int code = conn.getResponseCode();
                if (code == 301 || code == 302 || code == 303 || code == 307 || code == 308) {
                    final String next = conn.getHeaderField("Location");
                    if (next == null) {
                        throw new IOException("The download was redirected to nowhere.");
                    }
                    location = URI.create(location).resolve(next).toString();
                    continue;
                }
                if (code != 200) {
                    throw new IOException("The download failed with HTTP " + code + ".");
                }
                // The size the server declares, used only for the progress
                // bar: the limit below counts the bytes that arrive.
                final long expected = conn.getContentLengthLong();
                try (InputStream in = conn.getInputStream();
                     FileOutputStream sink = new FileOutputStream(out)) {
                    final byte[] buffer = new byte[64 * 1024];
                    long got = 0;
                    long said = 0;
                    progress.at(0, expected);
                    for (int n; (n = in.read(buffer)) > 0; ) {
                        got += n;
                        if (got > MAX_BYTES) {
                            throw new IOException("The download is larger than this can accept.");
                        }
                        sink.write(buffer, 0, n);
                        // Every quarter megabyte rather than every buffer: each
                        // update is posted to the main thread, and a progress
                        // bar cannot show finer steps.
                        if (got - said >= 256 * 1024) {
                            said = got;
                            progress.at(got, expected);
                        }
                    }
                    progress.at(got, expected);
                }
                return;
            } finally {
                conn.disconnect();
            }
        }
        throw new IOException("The download was redirected too many times.");
    }
}
