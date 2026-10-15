// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop.backend.rustdesk;

import android.content.Context;

import net.pgaskin.remotedesktop.backend.Backend;
import net.pgaskin.remotedesktop.backend.BackendOption;
import net.pgaskin.remotedesktop.backend.BackendProvider;
import net.pgaskin.remotedesktop.backend.NativeBackend;

import java.util.List;
import java.util.Map;

/**
 * This module's entry in a build's list of backends. What the RustDesk backend
 * is called and what it can be told are the catalog's, which the Linux client
 * reads too: {@link RustDeskCatalog}, generated from {@code backends/session}
 * at build time, holds the words and the option keys.
 *
 * <p>A session is a {@link NativeBackend} on {@link RustDeskNatives}, generated
 * too: the client in {@code src/main/rust} as a {@code session::Backend},
 * decoding with the phone's MediaCodec. What is left on this side is
 * {@link #HOOKS}, which is about the pin.
 */
public final class RustDeskProvider implements BackendProvider {

    /** {@code ConnectBy}'s two values, which decide what the address means. */
    public static final String BY_ID = "id";
    public static final String BY_ADDRESS = "address";

    /**
     * The public rendezvous network, which is what an empty
     * {@code RendezvousServer} means. Named here so that a pin keyed on it is
     * keyed on something rather than on the absence of an answer.
     */
    public static final String PUBLIC_NETWORK = "rs-ny.rustdesk.com";

    /**
     * The peer's long-term key, which only the id path has, is pinned under the
     * id <em>and the network it means something on</em>, and the first sight of
     * it is pinned <em>silently</em> — the one place this app does not ask.
     *
     * <p>An id is nine digits handed out by a rendezvous server and re-issued
     * when a machine's configuration is wiped, so the same digits on two
     * networks are two machines, and one entry under the bare id would have a
     * self-hosted server's peer and the public network's overwriting each
     * other's identity.
     *
     * <p>Silent because everywhere else a fingerprint arrives with nothing
     * behind it — a server's certificate says only that whoever made it made it
     * — so somebody has to be asked once. Here the rendezvous server has signed
     * that this key belongs to the id that was dialled, and the peer has proved
     * it holds the key, so a prompt would be asking a question that has an
     * answer and offering no way to check it. What is worth interrupting for is
     * the key changing under an id that has not, which arrives as the loud
     * prompt.
     */
    static final NativeBackend.Hooks HOOKS = new NativeBackend.Hooks() {
        @Override
        public String pinnedAs(String address, Map<String, String> options) {
            final String server = options.get(RustDeskCatalog.RENDEZVOUS_SERVER);
            return address + "@" + (server == null || server.isBlank()
                    ? PUBLIC_NETWORK : server.trim());
        }

        @Override
        public boolean pinsFirstSight() {
            return true;
        }
    };

    @Override
    public String id() {
        return RustDeskCatalog.ID;
    }

    @Override
    public String name() {
        return RustDeskCatalog.NAME;
    }

    @Override
    public String shortName() {
        return RustDeskCatalog.SHORT_NAME;
    }

    @Override
    public String description() {
        return RustDeskCatalog.DESCRIPTION;
    }

    @Override
    public int order() {
        return RustDeskCatalog.ORDER;
    }

    @Override
    public List<BackendOption> options() {
        return RustDeskCatalog.OPTIONS;
    }

    /**
     * What the address field holds by id: nine digits somebody reads off the
     * other machine. Dialled by address it is an address, which is what the app
     * calls it anyway.
     */
    @Override
    public String addressLabel(Map<String, String> options) {
        return BY_ID.equals(options.getOrDefault(RustDeskCatalog.CONNECT_BY, BY_ID)) ? "ID" : null;
    }

    @Override
    public Backend create(Context context, String address, String userName, String password,
                          Map<String, String> options) {
        return new NativeBackend(context, "RustDesk", RustDeskCatalog.OPTIONS,
                new RustDeskNatives(), HOOKS, address, userName, password, options);
    }
}
