// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop.plugin;

import java.io.File;
import java.io.FileInputStream;
import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.security.DigestOutputStream;
import java.security.MessageDigest;
import java.security.NoSuchAlgorithmException;

/**
 * SHA-256, which makes loading a third-party binary a yes-or-no decision: a
 * copy either is the build the declarations were read from or it is not.
 */
public final class Hashes {

    private Hashes() {
    }

    private static MessageDigest sha256() {
        try {
            return MessageDigest.getInstance("SHA-256");
        } catch (NoSuchAlgorithmException e) {
            throw new AssertionError(e); // required of every implementation
        }
    }

    public static String of(File file) throws IOException {
        try (InputStream in = new FileInputStream(file)) {
            return of(in);
        }
    }

    public static String of(InputStream in) throws IOException {
        final MessageDigest digest = sha256();
        final byte[] buffer = new byte[64 * 1024];
        for (int n; (n = in.read(buffer)) > 0; ) {
            digest.update(buffer, 0, n);
        }
        return hex(digest.digest());
    }

    /** Copies, and returns the SHA-256 of what was copied. */
    public static String copy(InputStream in, OutputStream out) throws IOException {
        return copy(in, out, Long.MAX_VALUE);
    }

    /**
     * Copies at most {@code limit} bytes, and fails rather than truncating:
     * these streams come from third-party archives or servers, and a
     * compressed entry can declare one size and deliver another.
     */
    public static String copy(InputStream in, OutputStream out, long limit) throws IOException {
        final MessageDigest digest = sha256();
        long total = 0;
        try (DigestOutputStream sink = new DigestOutputStream(out, digest)) {
            final byte[] buffer = new byte[64 * 1024];
            for (int n; (n = in.read(buffer)) > 0; ) {
                total += n;
                if (total > limit) {
                    throw new IOException("That is larger than this can accept.");
                }
                sink.write(buffer, 0, n);
            }
            sink.flush();
        }
        return hex(digest.digest());
    }

    private static String hex(byte[] bytes) {
        final StringBuilder sb = new StringBuilder(bytes.length * 2);
        for (byte b : bytes) {
            sb.append(Character.forDigit((b >> 4) & 0xf, 16));
            sb.append(Character.forDigit(b & 0xf, 16));
        }
        return sb.toString();
    }
}
