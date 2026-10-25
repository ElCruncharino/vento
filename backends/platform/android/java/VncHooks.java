// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop.backend;

/**
 * What the two borrowed VNC clients, libvncclient and TigerVNC, do differently
 * on this side of {@link NativeBackend}: one thing, the same in both, so one
 * object.
 *
 * <ul>
 *   <li><b>A TLS with no certificate is still encrypted.</b> Anonymous TLS is
 *       a server with nothing to verify it by, which {@link NativeBackend}
 *       asks about every time — but what going on means is not "in the clear"
 *       as it is for a RustDesk peer with no key: it is hidden from somebody
 *       listening and not from a machine in the middle, and the question says
 *       that, in the same words in both clients.
 * </ul>
 */
public final class VncHooks implements NativeBackend.Hooks {

    public static final VncHooks INSTANCE = new VncHooks();

    private VncHooks() {
    }

    @Override
    public String unverifiedConsequence() {
        return "Connecting anyway hides the session from somebody listening"
                + " and not from a machine in the middle.";
    }
}
