// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop.backend;

import android.content.Context;
import android.content.SharedPreferences;

import java.util.ArrayList;
import java.util.List;
import java.util.Map;

/**
 * Which certificate each address is supposed to have: where the phone keeps
 * its pins, and nothing about what they mean.
 *
 * <p>A remote desktop server's certificate is one it made for itself on first
 * start, and no authority vouches for it, so the identity is the key, as it is
 * over SSH: the fingerprint is pinned against the address the first time
 * somebody accepts it, and after that a connection is silent while it matches
 * and loud when it does not. That rule — the key an address is filed under,
 * how two fingerprints compare, what is asked when they do not, and when an
 * answer is pinned — is {@code api::trust} in {@code backends/api},
 * which the Linux client decides by too; {@link NativeBackend}'s JNI layer
 * applies it, and reads and writes this store to do so.
 *
 * <p>One preference per pin, the key as filed (the address folded for case, or
 * a RustDesk id with its network) to the fingerprint as the backend handed it
 * over. That is the format every pin made on this phone is already in, which
 * is why it is kept as it is.
 */
public final class KnownHosts {

    private static final String FILE = "known_hosts";

    private KnownHosts() {
    }

    private static SharedPreferences prefs(Context ctx) {
        return ctx.getApplicationContext().getSharedPreferences(FILE, Context.MODE_PRIVATE);
    }

    /**
     * Every pin, flattened: key, fingerprint, key, fingerprint. Anything in the
     * file that is not a string is not a pin.
     */
    public static String[] pins(Context ctx) {
        final List<String> flat = new ArrayList<>();
        for (Map.Entry<String, ?> e : prefs(ctx).getAll().entrySet()) {
            if (e.getValue() instanceof String pin) {
                flat.add(e.getKey());
                flat.add(pin);
            }
        }
        return flat.toArray(new String[0]);
    }

    /** {@code fingerprint} filed under {@code key}, which is already a key as filed. */
    public static void put(Context ctx, String key, String fingerprint) {
        prefs(ctx).edit().putString(key, fingerprint).apply();
    }

    /**
     * Every pin here, gone: the next connection to each of them asks again, as
     * a first one does.
     *
     * <p>Only this store. A backend whose library keeps its own is reached
     * through {@link BackendProvider#forgetHosts}, which is what
     * {@link Backends#forgetHosts} exists to call alongside this.
     */
    public static void clear(Context ctx) {
        prefs(ctx).edit().clear().apply();
    }
}
