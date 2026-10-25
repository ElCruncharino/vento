// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop;

import org.json.JSONException;
import org.json.JSONObject;

import java.util.Collections;
import java.util.LinkedHashMap;
import java.util.Iterator;
import java.util.Map;
import java.util.UUID;

/**
 * One saved connection: what to connect to, with what, and how.
 *
 * <p>Deliberately not RealVNC's {@code ServerRec}, which brings in native lists,
 * a finalizer idiom and an account sync that are not used here, and would only
 * have been needed for a thumbnail. This is a record and a JSON object.
 *
 * <p>{@link #options} holds only the {@link net.pgaskin.remotedesktop.backend.BackendOption.Scope#CONNECTION}
 * settings (including {@link AppOptions}), and only the ones changed from the
 * default: an absent key means "the backend's default", so a default that
 * changes in a later version reaches connections saved before it.
 *
 * @param pinned   kept at the top of the home screen. About the list rather
 *                 than about connecting, but stored here like the name, since
 *                 it is a per-connection choice of the user's.
 * @param sealedPassword the password as stored: encrypted under a keystore
 *                 key, or empty for none. See {@link Secrets}, which also
 *                 covers what happens on restoring a backup. Kept sealed
 *                 because this record is read whenever the list is, and only a
 *                 few callers need the plain text; a record holding it would
 *                 decrypt every password to draw the home screen and re-encrypt
 *                 every password to save any one of them, which with a keystore
 *                 that is briefly unavailable would overwrite the whole file's
 *                 passwords with nothing. The other fields are not secrets and
 *                 are stored as is.
 */
public record Connection(String id, String name, String backendId, String address,
                         String userName, String sealedPassword, Map<String, String> options,
                         boolean pinned) {

    public Connection {
        options = Collections.unmodifiableMap(new LinkedHashMap<>(options));
        sealedPassword = sealedPassword == null ? "" : sealedPassword;
    }

    public static Connection blank(String backendId) {
        return new Connection(UUID.randomUUID().toString(), "", backendId, "", "", "",
                Map.of(), false);
    }

    /**
     * The password in the clear, decrypted on each call. Empty if none is
     * stored or it cannot be decrypted; see {@link Secrets}.
     */
    public String password() {
        return Secrets.reveal(sealedPassword);
    }

    /**
     * The same connection with a new password, encrypted. A password that cannot
     * be encrypted is not stored at all rather than stored in the clear, so this
     * can return a record with no password.
     */
    public Connection withPassword(String plaintext) {
        final String sealed = Secrets.protect(plaintext);
        return new Connection(id, name, backendId, address, userName,
                sealed == null ? "" : sealed, options, pinned);
    }

    /** The title shown in lists: the name if there is one, else the address. */
    public String title() {
        return name == null || name.isEmpty() ? address : name;
    }

    /** The subtitle: the address, unless that is already the title. */
    public String subtitle() {
        return name == null || name.isEmpty() ? "" : address;
    }

    /** The same connection under a different user name. */
    public Connection withUserName(String newUserName) {
        return new Connection(id, name, backendId, address, newUserName, sealedPassword,
                options, pinned);
    }

    /**
     * Sets both credentials at once: from a prompt's answer, or cleared by the
     * editor. Together because they belong together: a user name kept without
     * its password makes the next prompt offer the wrong name.
     */
    public Connection withCredentials(String newUserName, String plaintext) {
        return withUserName(newUserName).withPassword(plaintext);
    }

    public Connection withOptions(Map<String, String> newOptions) {
        return new Connection(id, name, backendId, address, userName, sealedPassword,
                newOptions, pinned);
    }

    public Connection withPinned(boolean nowPinned) {
        return new Connection(id, name, backendId, address, userName, sealedPassword, options,
                nowPinned);
    }

    JSONObject toJson() throws JSONException {
        final JSONObject o = new JSONObject();
        o.put("id", id);
        o.put("name", name);
        o.put("backend", backendId);
        o.put("address", address);
        o.put("user", userName);
        // Already encrypted, and written as is: encrypting here would mean
        // every save re-encrypting every record, and one keystore failure
        // would lose every password in the file.
        o.put("password", sealedPassword);
        final JSONObject opts = new JSONObject();
        for (Map.Entry<String, String> e : options.entrySet()) {
            opts.put(e.getKey(), e.getValue());
        }
        o.put("options", opts);
        if (pinned) {
            o.put("pinned", true);
        }
        return o;
    }

    static Connection fromJson(JSONObject o) {
        final Map<String, String> opts = new LinkedHashMap<>();
        final JSONObject j = o.optJSONObject("options");
        if (j != null) {
            for (Iterator<String> it = j.keys(); it.hasNext(); ) {
                final String k = it.next();
                opts.put(k, j.optString(k));
            }
        }
        return new Connection(
                o.optString("id", UUID.randomUUID().toString()),
                o.optString("name", ""),
                o.optString("backend", ""),
                o.optString("address", ""),
                o.optString("user", ""),
                o.optString("password", ""),
                opts,
                o.optBoolean("pinned", false));
    }
}
