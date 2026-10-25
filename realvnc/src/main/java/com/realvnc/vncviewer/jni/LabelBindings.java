// SPDX-License-Identifier: CC0-1.0

package com.realvnc.vncviewer.jni;

/**
 * Team labels, a cloud account feature not supported here.
 *
 * <p>The fourth of {@link Bindings#initViewer}'s callbacks, resolved by the core
 * as {@code com/realvnc/vncviewer/jni/LabelBindings$Callback}. The class's two
 * natives look up labels, which is only useful to a client that has them.
 */
final class LabelBindings {

    interface Callback {
        void labelsChanged();
    }

    private LabelBindings() {
    }
}
