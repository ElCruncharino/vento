// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop.backend;

import android.content.Context;
import android.content.SharedPreferences;

import java.util.Locale;
import java.util.Map;

/**
 * Which certificate each address is supposed to have.
 *
 * <p>A remote desktop server's certificate is one it made for itself on first
 * start. No authority vouches for it and none ever will, so verifying it
 * against a root store would reject every server anybody actually runs — and a
 * client that then offered "connect anyway" would have taught its user to click
 * through the only check it has.
 *
 * <p>So the identity is the key, as it is over SSH: the fingerprint is pinned
 * against the address the first time somebody accepts it, and after that a
 * connection is silent while it matches and loud when it does not. That is what
 * makes the loud case mean something — a prompt that appears every time is a
 * prompt nobody reads.
 *
 * <p>It is deliberately keyed on the <em>address as written</em>, not on the
 * connection record: a session opened from the command line has no record, two
 * records may point at one machine, and the thing being identified is the
 * machine. Writing the same host two different ways makes two entries, which is
 * also what SSH does about {@code host} and its IP address.
 *
 * <p>Backend-neutral on purpose: RDP's certificate prompt is the same question
 * about a different protocol.
 */
public final class KnownHosts {

    private static final String FILE = "known_hosts";

    private KnownHosts() {
    }

    private static SharedPreferences prefs(Context ctx) {
        return ctx.getApplicationContext().getSharedPreferences(FILE, Context.MODE_PRIVATE);
    }

    /**
     * Folded for case and nothing else, which is right twice over: a name is
     * case-insensitive in DNS and an IPv6 literal is case-insensitive in its
     * own notation, so {@code [FE80::1]} and {@code [fe80::1]} are one machine
     * rather than two prompts.
     *
     * <p>What it does <em>not</em> fold is a default port left off or an
     * address written a shorter way, so one machine can still have two entries.
     * Filling a port in needs the protocol's own default, which is below this
     * class, and a backend that wants a key of its own already picks one.
     */
    public static String key(String address) {
        return address == null ? "" : address.trim().toLowerCase(Locale.ROOT);
    }

    /** What this address is pinned to, or null if it has never been accepted. */
    public static String pinned(Context ctx, String address) {
        return prefs(ctx).getString(key(address), null);
    }

    public static void pin(Context ctx, String address, String fingerprint) {
        prefs(ctx).edit().putString(key(address), fingerprint).apply();
    }

    public static void forget(Context ctx, String address) {
        prefs(ctx).edit().remove(key(address)).apply();
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

    /**
     * Whether this certificate is the one this address was accepted with, in
     * which case nothing is asked. A match being silent is the point of
     * pinning: a prompt that appears every time is a prompt nobody reads.
     */
    public static boolean matches(Context ctx, String address, String fingerprint) {
        return verdict(prefs(ctx).getAll(), address, fingerprint).matches();
    }

    /**
     * What a fingerprint comes to against a store's pins, as filed: whether it
     * matches, and if not, why it is being asked about — and with which other
     * address, where it is pinned to one. The whole of the rule, with the store
     * taken out of it, so that {@code backends/vectors/known_hosts.json} can
     * hold it to the Linux client's {@code known_hosts::verdict}.
     *
     * @param identity        {@code null} when it matches
     * @param matchingAddress the key it is pinned under elsewhere, or null
     */
    public record Verdict(Prompt.Trust.Identity identity, String matchingAddress) {
        public boolean matches() {
            return identity == null;
        }
    }

    public static Verdict verdict(Map<String, ?> pins, String address, String fingerprint) {
        final Object pin = pins.get(key(address));
        if (pin instanceof String p) {
            return canonical(p).equals(canonical(fingerprint))
                    ? new Verdict(null, null)
                    : new Verdict(Prompt.Trust.Identity.CHANGED, null);
        }
        final String elsewhere = otherAddressFor(pins, fingerprint, address);
        return elsewhere != null
                ? new Verdict(Prompt.Trust.Identity.MATCHES_ANOTHER, elsewhere)
                : new Verdict(Prompt.Trust.Identity.NEW, null);
    }

    /**
     * The hex digits alone, upper case — what two fingerprints are compared as.
     *
     * <p>A fingerprint is printed rather than defined: one library hands over
     * {@code 66:CC:ED:…} and another the same bytes as {@code 66:cc:ed:…}, and
     * this store is keyed on the machine rather than on what reached it, so a
     * second client asking about a server the first has pinned must not be told
     * its identity has changed. Separators go with the case for the same reason
     * — nothing about how a library spells a digest is part of the digest.
     * ASCII digits only: {@code Character.digit} would take a full-width one
     * too, and the Linux client does not.
     */
    public static String canonical(String fingerprint) {
        if (fingerprint == null) {
            return "";
        }
        final StringBuilder out = new StringBuilder(fingerprint.length());
        for (int i = 0; i < fingerprint.length(); i++) {
            final char c = fingerprint.charAt(i);
            if (c < 0x80 && Character.digit(c, 16) >= 0) {
                out.append(Character.toUpperCase(c));
            }
        }
        return out.toString();
    }

    /**
     * Put the question to somebody, and pin the answer if they say to remember
     * it. Main thread; {@code answer} is called there too, once.
     *
     * <p>The interesting distinction is between "never seen" and "seen, and
     * different now" — {@link Prompt.Trust.Identity} carries both, and a screen
     * says the second one loudly. Every backend that hands over a fingerprint
     * asks exactly this, which is why it is here rather than three times over;
     * a backend whose library does its own asking (TigerVNC's) is not a caller.
     */
    public static void ask(Context ctx, String address, String fingerprint,
                           Prompt.Handler handler, java.util.function.Consumer<Boolean> answer) {
        final Verdict verdict = verdict(prefs(ctx).getAll(), address, fingerprint);
        // Asked all the same where it matches, which no caller does: they ask
        // after matches() said no, and the store can only have moved since.
        final Prompt.Trust.Identity identity = verdict.matches()
                ? Prompt.Trust.Identity.OK : verdict.identity();
        final Prompt.Trust prompt = new Prompt.Trust(address,
                Prompt.Trust.Encryption.ENCRYPTED, identity, fingerprint, "",
                verdict.matchingAddress()) {
            @Override
            protected void deliver(boolean accept, boolean remember) {
                if (accept && remember) {
                    pin(ctx, address, fingerprint);
                }
                answer.accept(accept);
            }
        };
        if (handler == null) {
            prompt.cancel();
        } else {
            handler.trust(prompt);
        }
    }

    /**
     * Some other address this certificate is already pinned to, if there is one.
     *
     * <p>Worth saying rather than reporting a bare "never seen before": one
     * machine reached by name and by number, or moved to a new address, is the
     * ordinary explanation, and it is a different thing from a fingerprint that
     * has changed underneath an address that has not.
     */
    private static String otherAddressFor(Map<String, ?> pins, String fingerprint, String except) {
        if (fingerprint == null || fingerprint.isEmpty()) {
            return null;
        }
        final String skip = key(except);
        final String wanted = canonical(fingerprint);
        for (Map.Entry<String, ?> e : pins.entrySet()) {
            if (!e.getKey().equals(skip) && e.getValue() instanceof String pin
                    && wanted.equals(canonical(pin))) {
                return e.getKey();
            }
        }
        return null;
    }
}
