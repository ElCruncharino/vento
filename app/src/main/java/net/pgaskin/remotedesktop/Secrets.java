// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop;

import android.security.keystore.KeyGenParameterSpec;
import android.security.keystore.KeyProperties;
import android.util.Base64;
import android.util.Log;

import java.nio.charset.StandardCharsets;
import java.security.KeyStore;
import java.security.SecureRandom;

import javax.crypto.Cipher;
import javax.crypto.KeyGenerator;
import javax.crypto.SecretKey;
import javax.crypto.spec.GCMParameterSpec;

/**
 * Encryption for what this app stores under a key: a saved connection's
 * password, and the desktop preview on its home card.
 *
 * <p>The preview is the more revealing of the two: it is an image of the remote
 * screen when the session ended, written to disk without the user asking. It
 * is encrypted the same way, under a separate key.
 *
 * <p>AES-256-GCM under a key generated inside the Android keystore, so the key
 * material never exists in this process. Whether it is also hardware-backed is
 * up to the platform: nothing here requests StrongBox or checks
 * {@code KeyInfo.isInsideSecureHardware}, because a phone without it should
 * still be able to save a password. The rest of a connection (name, address,
 * user name) stays in plain text, since it is not secret.
 *
 * <p>Android's auto-backup includes the preference file, but the ciphertext in
 * it cannot be decrypted anywhere else: not on another device, whose keystore
 * lacks the key, and not on the same one after a reinstall or clearing the app
 * data, which deletes the key. This is intended, and it only requires that a
 * decryption failure be handled as "no saved password": the connection still
 * opens, the server asks for the password, and the prompt offers to save it.
 *
 * <p>Fails closed: if the keystore cannot produce a key, {@link #protect}
 * returns null and nothing is saved. A password is never stored in plain text
 * as a fallback.
 */
final class Secrets {

    private static final String TAG = "Secrets";
    private static final String KEYSTORE = "AndroidKeyStore";
    private static final String ALIAS = "connection-password";
    /** A separate key for previews. */
    private static final String ALIAS_PREVIEW = "connection-preview";
    private static final String TRANSFORM = "AES/GCM/NoPadding";
    /** Marks a value as ciphertext, and leaves room to change the scheme. */
    private static final String PREFIX = "v1:";
    private static final int IV_BYTES = 12;
    private static final int TAG_BITS = 128;

    private Secrets() {
    }

    /**
     * @return the stored form of {@code plaintext}, or null if it cannot be
     * encrypted, in which case the caller stores nothing
     */
    static String protect(String plaintext) {
        if (plaintext == null || plaintext.isEmpty()) {
            return "";
        }
        try {
            return PREFIX + Base64.encodeToString(
                    seal(plaintext.getBytes(StandardCharsets.UTF_8), ALIAS), Base64.NO_WRAP);
        } catch (Exception e) {
            Log.w(TAG, "cannot encrypt; the password will not be saved", e);
            return null;
        }
    }

    /**
     * The inverse of {@link #protect}. Returns "" for anything this device
     * cannot decrypt, the same as "nothing was saved" (see the class comment).
     */
    static String reveal(String stored) {
        if (stored == null || stored.isEmpty()) {
            return "";
        }
        if (!stored.startsWith(PREFIX)) {
            // Written before passwords were encrypted. Used as is, and stored
            // encrypted on the next save of that connection.
            return stored;
        }
        try {
            final byte[] blob = Base64.decode(stored.substring(PREFIX.length()), Base64.NO_WRAP);
            if (blob.length <= IV_BYTES) {
                return "";
            }
            return new String(open(blob, ALIAS), StandardCharsets.UTF_8);
        } catch (Exception e) {
            // A restored backup, cleared app data or a reinstall: the key is
            // gone and this can never be decrypted. Return no password and let
            // the server ask.
            Log.i(TAG, "no usable key for the saved password (" + e.getClass().getSimpleName()
                    + "); it will be asked for");
            return "";
        }
    }

    /** A twelve-byte IV and then the ciphertext, under {@code alias}'s key. */
    private static byte[] seal(byte[] plaintext, String alias) throws Exception {
        final Cipher cipher = Cipher.getInstance(TRANSFORM);
        cipher.init(Cipher.ENCRYPT_MODE, key(alias));
        final byte[] iv = cipher.getIV();
        final byte[] ct = cipher.doFinal(plaintext);
        final byte[] out = new byte[iv.length + ct.length];
        System.arraycopy(iv, 0, out, 0, iv.length);
        System.arraycopy(ct, 0, out, iv.length, ct.length);
        return out;
    }

    /** The inverse of {@link #seal}. The caller has checked that there is an IV to read. */
    private static byte[] open(byte[] sealed, String alias) throws Exception {
        final Cipher cipher = Cipher.getInstance(TRANSFORM);
        cipher.init(Cipher.DECRYPT_MODE, key(alias),
                new GCMParameterSpec(TAG_BITS, sealed, 0, IV_BYTES));
        return cipher.doFinal(sealed, IV_BYTES, sealed.length - IV_BYTES);
    }

    /**
     * The key for {@code alias}, generated once and kept in the keystore.
     *
     * <p>{@code setUnlockedDeviceRequired} because a password is only needed
     * while the device is in use, and {@code randomizedEncryptionRequired}
     * (the default, stated explicitly) because GCM with a repeated IV is
     * insecure.
     */
    private static SecretKey key(String alias) throws Exception {
        final KeyStore ks = KeyStore.getInstance(KEYSTORE);
        ks.load(null);
        final KeyStore.Entry entry = ks.getEntry(alias, null);
        if (entry instanceof KeyStore.SecretKeyEntry e) {
            return e.getSecretKey();
        }
        final KeyGenerator gen = KeyGenerator.getInstance(
                KeyProperties.KEY_ALGORITHM_AES, KEYSTORE);
        gen.init(new KeyGenParameterSpec.Builder(alias,
                KeyProperties.PURPOSE_ENCRYPT | KeyProperties.PURPOSE_DECRYPT)
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                .setKeySize(256)
                .setRandomizedEncryptionRequired(true)
                .setUnlockedDeviceRequired(true)
                .build(), new SecureRandom());
        return gen.generateKey();
    }

    // ---- byte arrays, for the desktop preview ------------------------------

    /**
     * Encrypt a preview for writing to disk. No Base64 and no prefix, because
     * it goes into its own file rather than a preference string.
     *
     * @return null if it cannot be encrypted, in which case nothing is written
     */
    static byte[] sealBytes(byte[] plaintext) {
        if (plaintext == null || plaintext.length == 0) {
            return null;
        }
        try {
            return seal(plaintext, ALIAS_PREVIEW);
        } catch (Exception e) {
            Log.w(TAG, "cannot encrypt; nothing will be written", e);
            return null;
        }
    }

    /** Null for anything this device cannot decrypt, which is not an error. */
    static byte[] openBytes(byte[] sealed) {
        if (sealed == null || sealed.length <= IV_BYTES) {
            return null;
        }
        try {
            return open(sealed, ALIAS_PREVIEW);
        } catch (Exception e) {
            Log.i(TAG, "no usable key for a stored preview ("
                    + e.getClass().getSimpleName() + "); it will be drawn again");
            return null;
        }
    }
}
