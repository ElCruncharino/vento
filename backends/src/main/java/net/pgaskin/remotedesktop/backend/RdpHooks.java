// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop.backend;

import android.content.Context;

/**
 * What an RDP backend does differently on this side of the boundary, as
 * {@link NativeBackend.Hooks}: written once for both RDP clients, since what
 * differs is the protocol and not the library.
 *
 * <ul>
 *   <li><b>The desktop size is asked for.</b> There is no desktop at the far end
 *       until this connects, and no way to ask for "whatever it already is", so
 *       {@code DesktopSize=remote} is the size the last session to this address
 *       ended up at, from {@link DesktopSizes} — and every size the far end
 *       reports is remembered for the next.
 *   <li><b>The domain is part of the user name.</b> {@code DOMAIN&#92;user} and
 *       {@code user@domain} are what every RDP client takes and every user
 *       types, so the prompt has no third field: it says so under the box, and
 *       starts with the record's name in the first form. What is typed back is
 *       split by the session.
 * </ul>
 */
public final class RdpHooks implements NativeBackend.Hooks {

    private final Context context;
    private final String address;

    public RdpHooks(Context context, String address) {
        this.context = context.getApplicationContext();
        this.address = address;
    }

    @Override
    public int[] lastDesktopSize() {
        return DesktopSizes.remembered(context, address);
    }

    @Override
    public void desktopSize(int width, int height) {
        DesktopSizes.remember(context, address, width, height);
    }

    @Override
    public String credentialsHint() {
        return "Use DOMAIN\\user for another domain, .\\user for a local account.";
    }

    @Override
    public String credentialsUser(String userName) {
        final String[] split = RdpIdentity.split(userName);
        return split[0].isEmpty() ? split[1] : split[0] + "\\" + split[1];
    }
}
