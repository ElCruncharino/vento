// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop.backend;

import android.content.Context;

/**
 * The RDP backends' Java-side differences, as {@link NativeBackend.Hooks}:
 * written once for both RDP clients, since what differs is the protocol and
 * not the library.
 *
 * <ul>
 *   <li><b>The desktop size is asked for.</b> There is no desktop at the far end
 *       until this connects, and no way to ask for "whatever it already is", so
 *       {@code DesktopSize=remote} is the size the last session to this address
 *       ended up at, from {@link DesktopSizes}, and every size the far end
 *       reports is remembered for the next.
 *   <li><b>The domain is part of the user name.</b> {@code DOMAIN&#92;user} and
 *       {@code user@domain} are what every RDP client accepts and users
 *       type, so the prompt has no third field, and a hint under the box
 *       says so.
 *       The box starts with the record's name in the first form, and what is
 *       typed back is split, both by {@code api::credentials} on the far
 *       side of the JNI layer.
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
        return "Use DOMAIN\\user for another domain, .\\user for a local account, or"
                + " MicrosoftAccount\\email@address.com for a Microsoft Account.";
    }
}
