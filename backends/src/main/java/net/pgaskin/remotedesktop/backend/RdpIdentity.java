// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop.backend;

/**
 * RDP's user name and domain, as the two RDP backends read them from what was
 * typed. Here rather than in each, because the rule for a prompt's answer is
 * one rule, and two copies of it are how one of them came to drop the
 * session's domain.
 */
public final class RdpIdentity {
    private RdpIdentity() {
    }

    /**
     * {@code DOMAIN&#92;user}, {@code user@domain} or a bare name, as
     * {@code {domain, user}}.
     *
     * <p>Both forms, because both are typed: the backslash is what Windows
     * shows and the {@code @} is what a UPN looks like. A bare name means the
     * machine's own accounts, which is the ordinary case for anything not in a
     * domain.
     */
    public static String[] split(String value) {
        final String name = value == null ? "" : value.trim();
        final int slash = name.indexOf('\\');
        if (slash >= 0) {
            return new String[]{name.substring(0, slash), name.substring(slash + 1)};
        }
        final int at = name.indexOf('@');
        if (at > 0) {
            return new String[]{name.substring(at + 1), name.substring(0, at)};
        }
        return new String[]{"", name};
    }

    /**
     * A credentials prompt's answer as the {@code {domain, user}} to send,
     * against the session's own {@code domain} and {@code user} (already
     * split by {@link #split}).
     *
     * <p>An empty name is the session's, domain and all. A name typed with a
     * domain is that one. A bare name keeps the session's domain, since the
     * prompt has no box for one: a connection saved as {@code CORP&#92;proto}
     * and answered with {@code alice} means {@code CORP&#92;alice}, not a local
     * alice. A local account on a machine that has a domain is {@code
     * .&#92;alice}, as Windows writes it, and {@code .} is what is sent.
     */
    public static String[] answer(String typed, String domain, String user) {
        final String name = typed == null ? "" : typed.trim();
        if (name.isEmpty()) {
            return new String[]{domain, user};
        }
        final String[] split = split(name);
        if (split[0].isEmpty()) {
            return new String[]{domain, split[1]};
        }
        return split;
    }
}
