// SPDX-License-Identifier: CC0-1.0

package com.realvnc.vncviewer.jni;

/**
 * "Remember this server's identity." The identity delivered here is what a
 * later connection passes back as the {@code Identity} option, which makes the
 * next security prompt for that server silent.
 *
 * <p>Required by
 * {@code Java_com_realvnc_vncviewer_jni_SaveIdentityBindings_setIdentityStore}
 * and by {@code SaveIdentityBindings$IdentityStore}, which the core resolves by
 * name.
 */
public final class SaveIdentityBindings {

    public interface IdentityStore {
        void saveIdentity(SessionBindings.Session session, String identity);
    }

    private SaveIdentityBindings() {
    }

    public static native void setIdentityStore(IdentityStore store);
}
