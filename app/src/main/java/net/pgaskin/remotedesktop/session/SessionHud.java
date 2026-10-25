// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop.session;

import android.graphics.Canvas;
import android.os.SystemClock;

import net.pgaskin.remotedesktop.backend.Backend;
import net.pgaskin.remotedesktop.control.CursorController;
import net.pgaskin.remotedesktop.control.Viewport;
import net.pgaskin.remotedesktop.control.input.Button;
import net.pgaskin.remotedesktop.control.input.Config;
import net.pgaskin.remotedesktop.control.input.ExtensionKeyboard;
import net.pgaskin.remotedesktop.control.input.GestureRecognizer;
import net.pgaskin.remotedesktop.control.input.MouseOverlay;
import net.pgaskin.remotedesktop.control.input.PhysicalKeyboard;
import net.pgaskin.remotedesktop.control.input.PhysicalMouse;
import net.pgaskin.remotedesktop.control.ui.Hud;

import java.util.Arrays;
import java.util.Locale;

/**
 * The playground's HUD on a real connection: the same lines in the same order
 * so the two screens can be compared, plus the pixel path statistics, which
 * the playground lacks and which show whether the mirror is keeping up.
 *
 * <p>{@link Hud} draws the box (and documents why it is shared); this class
 * builds the session screen's lines. Separate from the view because it reads
 * every component at once, for no purpose of the session's own.
 */
final class SessionHud {

    private final Hud hud;
    private final Config cfg;
    private final Backend backend;
    private final SessionClipboard clipboard;
    private final CursorController cursor;
    private final Viewport viewport;
    private final GestureRecognizer gestures;
    private final MouseOverlay overlay;
    private final ExtensionKeyboard keyboard;
    private final PhysicalMouse mouse;
    private final PhysicalKeyboard keys;

    private final Hud.Rate frameRate = new Hud.Rate();
    private final Hud.Rate eventRate = new Hud.Rate();
    private final Hud.Rate damageRate = new Hud.Rate();
    private long frames;

    SessionHud(Config cfg, Backend backend, SessionClipboard clipboard, CursorController cursor,
               Viewport viewport, GestureRecognizer gestures, MouseOverlay overlay,
               ExtensionKeyboard keyboard, PhysicalMouse mouse, PhysicalKeyboard keys) {
        this.hud = new Hud(cfg);
        this.cfg = cfg;
        this.backend = backend;
        this.clipboard = clipboard;
        this.cursor = cursor;
        this.viewport = viewport;
        this.gestures = gestures;
        this.overlay = overlay;
        this.keyboard = keyboard;
        this.mouse = mouse;
        this.keys = keys;
    }

    /**
     * @param mirror    the mirror, or null before a desktop size arrived
     * @param imeHeight the system IME's height, which is separate from the
     *                  overlay and the extension row and can outlive both
     */
    void draw(Canvas c, int viewW, int viewH, Mirror mirror, Backend.State state,
              String lastRegion, int imeHeight, boolean pointerCaptured) {
        final long now = System.nanoTime();
        final long rects = mirror == null ? 0 : mirror.damageRects();
        frames++;
        String[] lines = {
                "state " + state
                        + "  desktop " + backend.desktopWidth() + "x" + backend.desktopHeight()
                        + "  fps " + frameRate.sample(frames, now)
                        + "  dmg " + rects + " (" + damageRate.sample(rects, now) + "/s)"
                        // The pixel path, in the order it happens.
                        + "  tile " + (mirror == null ? "-"
                        : mirror.tileWidth() + "x" + mirror.tileHeight()
                        + " " + mirror.visibleTiles() + "/" + mirror.allocatedTiles()
                        + "/" + mirror.tileCount()
                        + " rd " + mirror.lastRead() + " dty " + mirror.dirtyCount())
                        + "  clip " + clipboard.summary(),
                "cursor " + (cursor.isRelative() ? "theirs"
                        : (int) cursor.x() + "," + (int) cursor.y())
                        + "  btn " + cursor.buttonsName()
                        + "  scale " + String.format(Locale.ROOT, "%.3f", viewport.getScale())
                        + " [" + (viewport.zoomIndex() + 1) + "/" + viewport.zoomLadder().length + "]"
                        + "  origin " + (int) viewport.originX() + "," + (int) viewport.originY()
                        + "  content " + viewport.contentWidth() + "x" + viewport.contentHeight(),
                "down " + gestures.downCount() + "  max " + gestures.maxDownCount()
                        + "  mode " + gestures.mode()
                        + "  moving " + (gestures.moving() ? "Y" : "N")
                        + "  held " + (gestures.heldButton() == null ? "-" : gestures.heldButton()),
                "accel x" + String.format(Locale.ROOT, "%.2f", gestures.accelFactor())
                        // dp/ms, so it can be read against the Config thresholds
                        + "  spd " + String.format(Locale.ROOT, "%.2f", gestures.accelSpeed() / cfg.density)
                        + "  lock " + gestures.axisLock()
                        + "  hover x" + String.format(Locale.ROOT, "%.2f", gestures.hoverGain())
                        + " lag " + String.format(Locale.ROOT, "%.0f", gestures.hoverLagMs())
                        + (gestures.hoverLockedOut(SystemClock.uptimeMillis()) ? " LOCKED" : "")
                        + "  events " + cursor.eventCount()
                        + " (" + eventRate.sample(cursor.eventCount(), now) + "/s)",
                "ovl " + (overlay.visible()
                        ? Button.maskName(overlay.heldMask())
                        + " rate " + String.format(Locale.ROOT, "%.1f", overlay.scrollRate())
                        : "off")
                        + "   kbd " + (keyboard.visible()
                        ? "on ime " + imeHeight + " mod " + keyboard.heldModifierCount()
                        : "off")
                        + "   region " + lastRegion
                        + (backend.viewOnly() ? "   VIEW ONLY" : ""),
                // Physical mouse and keyboard. Capture state is shown because
                // an uncaptured mouse looks the same until it reaches the edge
                // of the screen and stops.
                "mouse " + (pointerCaptured ? "captured"
                        : mouse.seen() ? "hover" : "-")
                        + " btn " + Button.maskName(mouse.heldMask())
                        + "   keys " + keys.heldCount() + " held",
        };
        // Audio, if the session has a sound switch, in the same format as the
        // desktop client: what is playing, the queue before the device and the
        // device's own buffer (together, the playback latency), the queue's
        // corrections for the far end's timing, and whether the far end is
        // asked not to send audio or it is muted here. Only read here; the
        // panel sets the switch.
        final String sound = backend.soundLine();
        if (sound != null) {
            lines = Arrays.copyOf(lines, lines.length + 1);
            lines[lines.length - 1] = sound;
        }
        // The keyboard's heightPx(), not its insetBottomPx(): the desktop runs
        // under the info bar but the HUD does not.
        hud.draw(c, lines, viewW, viewH,
                Math.max(Math.max(overlay.insetBottomPx(), keyboard.heightPx()), imeHeight));
    }
}
