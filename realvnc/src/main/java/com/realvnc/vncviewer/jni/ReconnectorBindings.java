// SPDX-License-Identifier: CC0-1.0

package com.realvnc.vncviewer.jni;

/**
 * The "reconnecting…" banner, which is the visible half of the core's automatic
 * reconnect. Unlike the prompt families this one asks nothing: it is told to
 * show, to hide and to close, and there is no result native to call.
 *
 * <p>Worth registering even for a client that draws no banner: it is the only
 * notice that a session still reporting itself as connected is being
 * re-established.
 *
 * <p>Required by
 * {@code Java_com_realvnc_vncviewer_jni_ReconnectorBindings_setDlgFactory}, and
 * by {@code ReconnectorBindings$ReconnectDlg} and {@code $DlgFactory}, which the
 * core resolves by name.
 */
public final class ReconnectorBindings {

    public interface ReconnectDlg {

        void close();

        void hide();

        void show(String message, String detail);
    }

    public interface DlgFactory {
        ReconnectDlg createReconnectDlg(SessionBindings.Session session);
    }

    private ReconnectorBindings() {
    }

    public static native void setDlgFactory(DlgFactory factory);
}
