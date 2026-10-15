// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop.backend;

import android.content.Context;
import android.graphics.Bitmap;
import android.os.Build;
import android.os.Handler;
import android.os.Looper;
import android.util.Log;

import java.util.ArrayList;
import java.util.List;
import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;
import java.util.function.Consumer;

/**
 * A {@link Backend} on any of the backends written in Rust: the protocol
 * crate's {@code session::Backend} adapter, which the Linux client holds too,
 * reached through the one JNI layer in {@code backends/platform/android}.
 *
 * <p>There used to be one of these per protocol, and they were twins: the
 * same handle, the same retirement, the same posting to the main thread,
 * differing in which natives they called and in a handful of things that
 * really are the protocol's. The natives are now one surface
 * ({@link Natives}), and the handful are {@link Hooks}.
 *
 * <p>What a module supplies is its natives class — generated, one per module,
 * because each library registers its functions on a class of its own — and
 * its hooks. Everything else is here:
 *
 * <ul>
 *   <li><b>No session thread.</b> RealVNC's core registers its socket on the
 *       calling thread's {@code ALooper}, so every call into it has to be
 *       marshalled onto one {@code Looper} thread. These own their threads, so
 *       a call from anywhere is just a call.
 *   <li><b>No connect timeout here.</b> It is in each client, on the socket,
 *       and only over the handshake — a deadline that covers the server but not
 *       the person typing a password.
 *   <li><b>No pixel lock against a resize.</b> Each framebuffer is behind a
 *       reader-writer lock inside its client rather than being a scratch buffer
 *       an unlocked allocator can move under a copy in flight.
 * </ul>
 *
 * <p>What survives is {@link #handle}'s retirement: the drawing thread reads
 * pixels through this object, and {@link Natives#destroy} frees the session,
 * so the handle is taken out of circulation under {@link #pixels} before it is
 * freed.
 *
 * <p>Events arrive on the protocol thread, through the {@code on…} methods
 * below, in the order the session said them. Damage arrives once per frame, as
 * one array of rectangles, and is handed to the listener a rectangle at a
 * time from here — on this side of the boundary, so that a busy frame costs
 * one crossing rather than one per rectangle.
 */
public final class NativeBackend implements Backend {

    /**
     * The natives every Rust backend's library registers, on a class of the
     * module's own that {@code vento-codegen natives} writes from the JNI table
     * in {@code backends/platform/android/src/surface.rs}.
     *
     * <h2>Threads</h2>
     * <ul>
     *   <li>{@link #readRegion} and {@link #readThumbnail} are called from
     *       whichever thread is drawing, concurrently with everything else.
     *   <li>Everything else may be called from anywhere; a client encodes input
     *       on the calling thread and writes it on a thread of its own, so a
     *       stalled socket cannot reach the main thread.
     * </ul>
     *
     * <h2>The handle</h2>
     * {@link #destroy} frees the session and joins its thread, so nothing may
     * hold a handle across it; {@link NativeBackend} retires the handle under
     * its own lock first. Every other call takes a zero handle as "already
     * gone" and does nothing.
     */
    public interface Natives {

        /** What the library says it is — the one call that proves the whole path. */
        String version();

        /**
         * Start connecting. Returns a handle at once, or zero where the session
         * could not even start; everything after this arrives on {@code events}.
         *
         * @param tag        what the library logs under
         * @param password   null to be asked; empty is not a password
         * @param options    key, value, key, value — over the catalog's defaults
         * @param screenWidth the window a session gets on this phone, or zero:
         *                   what an RDP {@code DesktopSize=device} asks for
         * @param lastWidth  the size this address's desktop was left at, or zero:
         *                   what an RDP {@code DesktopSize=remote} asks for
         * @param clientName what the far end lists this phone as
         * @param filesDir   where a library may keep what outlives a session
         */
        long create(NativeBackend events, String tag, String address, String userName,
                    String password, String[] options, int screenWidth, int screenHeight,
                    int lastWidth, int lastHeight, String clientName, String filesDir);

        /**
         * The answer to {@link #onCredentialsNeeded}: what was typed, with a
         * null password for a cancel, which ends the session. An empty user
         * name is the session's own; an RDP one is split into a domain there.
         */
        void answerCredentials(long handle, String userName, String password);

        /** The answer to {@link #onTrustNeeded} or {@link #onUnverified}. */
        void answerTrust(long handle, boolean accept);

        void disconnect(long handle);

        /** Ends the session, joins its thread and frees the handle. */
        void destroy(long handle);

        void pointer(long handle, int x, int y, int buttonMask);

        void pointerRelative(long handle, int dx, int dy, int buttonMask);

        void keyDown(long handle, int keysym, long keyId);

        void keyUp(long handle, long keyId);

        void releaseAllKeys(long handle);

        void focus(long handle, boolean focused);

        void clipboard(long handle, String text);

        /** One option on the running session; what is live is the backend's to say. */
        void setOption(long handle, String key, String value);

        void requestDesktopSize(long handle, int width, int height);

        void requestDisplay(long handle, int index);

        /**
         * {@link Facts} in one crossing: width, height, display, flags (1 can
         * resize, 2 view only, 4 pointer relative), the monitor count, then
         * four ints per monitor and four per display. Null once gone.
         */
        int[] facts(long handle);

        /**
         * The connection panel's rows, four strings each: the
         * {@link ConnectionFact.Field}'s name, the label, the value, and
         * {@code "d"} for a diagnostic. Empty until connected.
         */
        String[] info(long handle);

        /** Received and sent, -1 for a direction nobody counts; null for none. */
        long[] traffic(long handle);

        boolean readRegion(long handle, int x, int y, int width, int height,
                           Bitmap dst, int dstX, int dstY);

        /** The whole desktop at {@code 1/step}, nearest neighbour. */
        boolean readThumbnail(long handle, int step, Bitmap dst);
    }

    /**
     * What a backend does differently on this side of the boundary: the few
     * places where what is right depends on the protocol and the decision is
     * still the app's rather than the session's. Each has the common answer as
     * its default, so a backend says only where it differs.
     */
    public interface Hooks {

        /** The common answer to every question. */
        Hooks NONE = new Hooks() {
        };

        /**
         * {@code {width, height}} the far end was left at last time, for a
         * protocol that has to ask for a size and has no way to ask for
         * "whatever it already is"; null for none.
         */
        default int[] lastDesktopSize() {
            return null;
        }

        /**
         * The far end's desktop is this size now. Protocol thread, as
         * {@link Listener#damaged} is: whatever is done here must be cheap.
         */
        default void desktopSize(int width, int height) {
        }

        /** Words under a credentials prompt, or empty. */
        default String credentialsHint() {
            return "";
        }

        /** The user name a credentials prompt starts with, from the record's. */
        default String credentialsUser(String userName) {
            return userName;
        }

        /**
         * What a pin is kept under: the address, unless an identity means
         * something only together with something else.
         */
        default String pinnedAs(String address, Map<String, String> options) {
            return address;
        }

        /**
         * Whether an identity seen for the first time is pinned without asking:
         * true only where something has already vouched for it, so a prompt
         * would ask a question that has an answer. A change is always asked.
         */
        default boolean pinsFirstSight() {
            return false;
        }
    }

    private final String tag;
    private final Natives natives;
    private final Hooks hooks;
    private final Handler main = new Handler(Looper.getMainLooper());

    /**
     * The remote cursor's shapes. Touched only by {@link #onCursor}, which is
     * the protocol thread's, so it needs no lock of its own.
     */
    private final CursorCache cursors = new CursorCache();
    private final Context context;
    private final String address;
    private final String userName;
    private final String password;
    private final Map<String, String> options;

    private Listener listener;
    private Prompt.Handler prompts;

    /** Guards {@link #handle} against being freed under a read in flight. */
    private final Object pixels = new Object();
    // Volatile as well as guarded: the lock holds off the free while a pixel
    // read is in flight, and every other caller — input, a prompt answered from
    // the protocol thread, the panel — reads it without taking it.
    private volatile long handle;

    private volatile State state = State.IDLE;
    private volatile int desktopWidth;
    private volatile int desktopHeight;
    private volatile boolean dead;

    /**
     * @param tag     what this backend logs under, here and in its library
     * @param catalog the backend's options, whose defaults fill in what the
     *                connection's own options leave out
     */
    public NativeBackend(Context context, String tag, List<BackendOption> catalog,
                         Natives natives, Hooks hooks, String address, String userName,
                         String password, Map<String, String> extraOptions) {
        // The application's, not a screen's: this outlives the activity by
        // design, and what it is for is the pin store and the hooks' notes.
        this.context = context.getApplicationContext();
        this.tag = tag;
        this.natives = natives;
        this.hooks = hooks == null ? Hooks.NONE : hooks;
        this.address = address;
        this.userName = userName == null ? "" : userName;
        // Empty is not a password, for the reason the RealVNC backend found
        // it first: something that looks like a stored secret and is not one
        // fails the connection instead of asking for the real one.
        this.password = password == null || password.isEmpty() ? null : password;
        this.options = new ConcurrentHashMap<>();
        for (var option : catalog) {
            options.put(option.key(), option.defaultValue());
        }
        if (extraOptions != null) {
            // The map is concurrent, so it takes no nulls: an option nobody
            // has answered is an absent key, which is what every reader here
            // expects.
            extraOptions.forEach((k, v) -> {
                if (k != null && v != null) {
                    options.put(k, v);
                }
            });
        }
    }

    // ---- lifecycle ---------------------------------------------------------

    @Override
    public void setListener(Listener listener) {
        this.listener = listener;
    }

    @Override
    public void setPromptHandler(Prompt.Handler handler) {
        this.prompts = handler;
    }

    @Override
    public void connect() {
        if (state != State.IDLE) {
            return;
        }
        state = State.CONNECTING;
        fireState(State.CONNECTING, "Connecting to " + address);
        final String[] flat = new String[options.size() * 2];
        int i = 0;
        for (var e : options.entrySet()) {
            flat[i++] = e.getKey();
            flat[i++] = e.getValue();
        }
        final int[] screen = DesktopSizes.device(context);
        final int[] last = hooks.lastDesktopSize();
        final long h = natives.create(this, tag, address, userName, password, flat,
                screen == null ? 0 : screen[0], screen == null ? 0 : screen[1],
                last == null ? 0 : last[0], last == null ? 0 : last[1],
                // What the far end shows and logs while this session is up.
                // The phone rather than the app: somebody looking at their own
                // desktop wants to know which device is on it.
                Build.MODEL, context.getFilesDir().getPath());
        if (h == 0) {
            closed("Could not start the connection");
            return;
        }
        synchronized (pixels) {
            handle = h;
        }
    }

    @Override
    public void disconnect() {
        final long h = handle;
        if (h != 0) {
            natives.disconnect(h);
        } else {
            closed("Disconnected");
        }
    }

    @Override
    public void destroy() {
        dead = true;
        final long h;
        // Retire it first: a read that has already got past this point is
        // holding the framebuffer's lock, and destroy waits for the protocol
        // thread, not for that.
        synchronized (pixels) {
            h = handle;
            handle = 0;
        }
        if (h != 0) {
            natives.destroy(h);
        }
    }

    @Override
    public State state() {
        return state;
    }

    @Override
    public int desktopWidth() {
        return desktopWidth;
    }

    @Override
    public int desktopHeight() {
        return desktopHeight;
    }

    @Override
    public Facts facts() {
        final long h = handle;
        final int[] f = h != 0 && !dead ? natives.facts(h) : null;
        if (f == null || f.length < 5) {
            return new Facts(desktopWidth, desktopHeight, List.of(), List.of(), -1, false,
                    viewOnly(), false);
        }
        final int monitors = Math.max(0, Math.min(f[4], (f.length - 5) / 4));
        final int split = 5 + monitors * 4;
        return new Facts(f[0], f[1],
                Monitor.fromFlat(java.util.Arrays.copyOfRange(f, 5, split)),
                Monitor.fromFlat(java.util.Arrays.copyOfRange(f, split, f.length)),
                f[2], (f[3] & 1) != 0, (f[3] & 2) != 0, (f[3] & 4) != 0);
    }

    @Override
    public void requestDesktopSize(int width, int height) {
        final long h = handle;
        if (h != 0 && !dead) {
            natives.requestDesktopSize(h, width, height);
        }
    }

    @Override
    public void requestDisplay(int index) {
        final long h = handle;
        if (h != 0 && !dead) {
            natives.requestDisplay(h, index);
        }
    }

    /**
     * The facts, gathered without touching the protocol thread: every client
     * keeps them in a mutex beside the connection, so this is a lock and a
     * copy. Asynchronous all the same, because the interface is.
     *
     * <p>The rows and their words are the backend's; the one row whose words
     * are ours, {@link ConnectionFact.Field#DATA}, is made here from the raw
     * counts, in the phone's own number format.
     */
    @Override
    public void connectionInfo(Consumer<List<ConnectionFact>> callback) {
        final long h = handle;
        if (h == 0 || dead || state != State.CONNECTED) {
            main.post(() -> callback.accept(List.of()));
            return;
        }
        final String[] info = natives.info(h);
        final List<ConnectionFact> facts = new ArrayList<>();
        for (int i = 0; info != null && i + 3 < info.length; i += 4) {
            final ConnectionFact.Field field;
            try {
                field = ConnectionFact.Field.valueOf(info[i]);
            } catch (IllegalArgumentException e) {
                continue;
            }
            if (field == ConnectionFact.Field.DATA) {
                final long[] traffic = natives.traffic(h);
                if (traffic != null && traffic.length == 2) {
                    facts.add(ConnectionFact.data(traffic[0], traffic[1]));
                }
            } else {
                facts.add(new ConnectionFact(field, info[i + 1], info[i + 2],
                        "d".equals(info[i + 3])));
            }
        }
        main.post(() -> callback.accept(facts));
    }

    @Override
    public boolean viewOnly() {
        return Boolean.parseBoolean(options.get("ViewOnly"));
    }

    @Override
    public void setOption(String key, String value) {
        options.put(key, value == null ? "" : value);
        final long h = handle;
        if (h != 0 && !dead) {
            natives.setOption(h, key, value == null ? "" : value);
        }
    }

    // ---- input -------------------------------------------------------------

    @Override
    public void pointer(int x, int y, int buttonMask) {
        final long h = handle;
        if (h != 0) {
            natives.pointer(h, x, y, buttonMask);
        }
    }

    @Override
    public void pointerRelative(int dx, int dy, int buttonMask) {
        final long h = handle;
        if (h != 0) {
            natives.pointerRelative(h, dx, dy, buttonMask);
        }
    }

    @Override
    public void keyDown(int keysym, int keyId) {
        final long h = handle;
        if (h != 0) {
            natives.keyDown(h, keysym, keyId);
        }
    }

    @Override
    public void keyUp(int keyId) {
        final long h = handle;
        if (h != 0) {
            natives.keyUp(h, keyId);
        }
    }

    @Override
    public void releaseAllKeys() {
        final long h = handle;
        if (h != 0) {
            natives.releaseAllKeys(h);
        }
    }

    @Override
    public void focus(boolean focused) {
        final long h = handle;
        if (h != 0) {
            natives.focus(h, focused);
        }
    }

    @Override
    public void clipboardToRemote(String text) {
        final long h = handle;
        if (h != 0 && text != null && !text.isEmpty()) {
            natives.clipboard(h, text);
        }
    }

    // ---- pixels ------------------------------------------------------------

    @Override
    public boolean readRegion(int x, int y, int width, int height,
                              Bitmap dst, int dstX, int dstY) {
        if (dst == null || dst.getConfig() != Bitmap.Config.ARGB_8888) {
            return false;
        }
        synchronized (pixels) {
            return handle != 0
                    && natives.readRegion(handle, x, y, width, height, dst, dstX, dstY);
        }
    }

    @Override
    public Bitmap thumbnail(int maxWidth, int maxHeight) {
        final int w = desktopWidth, h = desktopHeight;
        if (w <= 0 || h <= 0 || maxWidth <= 0 || maxHeight <= 0) {
            return null;
        }
        // Any integer step. RealVNC's had to be a power of two because their
        // scaler computes its stride as (int)(1/scale) and a float reciprocal
        // does not round-trip; these take the step itself and never see a
        // float.
        final int step = Math.max(1, Math.max(
                (w + maxWidth - 1) / maxWidth, (h + maxHeight - 1) / maxHeight));
        final int tw = Math.max(1, (w + step - 1) / step);
        final int th = Math.max(1, (h + step - 1) / step);
        final Bitmap out = Bitmap.createBitmap(tw, th, Bitmap.Config.ARGB_8888);
        final boolean ok;
        synchronized (pixels) {
            ok = handle != 0 && natives.readThumbnail(handle, step, out);
        }
        if (!ok) {
            out.recycle();
            return null;
        }
        return out;
    }

    // ---- events (protocol thread; called from the library) -----------------

    void onConnected(int width, int height) {
        Log.i(tag, "connected: " + width + "x" + height);
        desktopWidth = width;
        desktopHeight = height;
        hooks.desktopSize(width, height);
        main.post(() -> {
            if (dead) {
                return;
            }
            state = State.CONNECTED;
            if (listener != null) {
                listener.desktopSize(width, height);
            }
            fireState(State.CONNECTED, null);
            // Offer what is on the phone's clipboard now, so a paste works
            // without leaving the app and coming back. RealVNC's core pulls
            // when it wants one; these clients have to be told.
            final Listener l = listener;
            if (l != null) {
                clipboardToRemote(l.clipboardForRemote());
            }
        });
    }

    void onDesktopSize(int width, int height) {
        Log.i(tag, "desktop size: " + width + "x" + height);
        desktopWidth = width;
        desktopHeight = height;
        hooks.desktopSize(width, height);
        main.post(() -> {
            if (listener != null) {
                listener.desktopSize(width, height);
            }
        });
    }

    /** One frame: every rectangle that changed in it, four ints each. */
    void onFrame(int[] rects) {
        final Listener l = listener;
        if (l == null) {
            return;
        }
        for (int i = 0; i + 3 < rects.length; i += 4) {
            l.damaged(rects[i], rects[i + 1], rects[i + 2], rects[i + 3]);
        }
        l.frameEnd();
    }

    /**
     * @param argb {@code width * height} pixels, {@code Color}-packed; empty
     *             when the server has hidden the pointer
     * @param hash of the pixels, computed where they were filled in: the
     *             identity {@code CursorCache} keeps the built bitmap under,
     *             so an unchanged shape costs no bitmap at all
     */
    void onCursor(int[] argb, int width, int height, int hotX, int hotY, long hash) {
        final Bitmap shape = cursors.shape(hash, argb, width, height);
        main.post(() -> {
            if (listener != null) {
                listener.cursor(shape, hotX, hotY);
            }
        });
    }

    void onPointerMode(boolean relative) {
        main.post(() -> {
            if (listener != null) {
                listener.pointerMode(relative);
            }
        });
    }

    /** Already through {@code AcceptBell}, which the session applies. */
    void onBell() {
        main.post(() -> {
            if (listener != null) {
                listener.bell();
            }
        });
    }

    void onClipboard(String text) {
        if (text == null) {
            return;
        }
        main.post(() -> {
            if (listener != null) {
                listener.clipboardFromRemote(text);
            }
        });
    }

    /**
     * Credentials are wanted. Answered through {@link Natives#answerCredentials}
     * with what was typed: what the protocol makes of a blank or a domain is
     * the session's to say.
     */
    void onCredentialsNeeded(boolean needsUserName) {
        Log.i(tag, "credentials wanted (user name: " + needsUserName + ")");
        main.post(() -> {
            final Prompt.Handler h = prompts;
            final Prompt.Credentials prompt = new Prompt.Credentials(address,
                    hooks.credentialsHint(), needsUserName,
                    needsUserName ? hooks.credentialsUser(userName) : "", true, "", "") {
                @Override
                protected void deliver(boolean ok, String user, String secret) {
                    final long session = handle;
                    if (session != 0) {
                        natives.answerCredentials(session, user == null ? "" : user,
                                ok ? secret : null);
                    }
                }
            };
            if (h == null) {
                prompt.cancel();
            } else {
                h.credentials(prompt);
            }
        });
    }

    /**
     * The far end's identity, against what it was accepted with before: a
     * match goes on without asking, from this thread, and anything else is a
     * person's question — except a first sight a backend's {@link Hooks} says
     * is already vouched for, which is pinned without one.
     */
    void onTrustNeeded(String fingerprint) {
        final String key = hooks.pinnedAs(address, options);
        if (KnownHosts.matches(context, key, fingerprint)) {
            Log.i(tag, "identity matches the pin for " + key);
            answerTrust(true);
            return;
        }
        if (hooks.pinsFirstSight() && KnownHosts.pinned(context, key) == null) {
            KnownHosts.pin(context, key, fingerprint);
            Log.i(tag, "pinned " + key);
            answerTrust(true);
            return;
        }
        main.post(() -> KnownHosts.ask(context, key, fingerprint, prompts, this::answerTrust));
    }

    /**
     * The machine could not be verified at all, which is a different question
     * from whether an identity is the right one: there is nothing to pin,
     * nothing to compare it against, and going on means going on in the clear.
     *
     * <p>So it is a question rather than a refusal, and it is
     * {@link Prompt.Message} rather than {@link Prompt.Trust}: a trust prompt
     * offers to remember an identity, and the whole of this case is that there
     * is not one. Nothing is remembered and it is asked every time. Only
     * RustDesk asks it today; the others have an identity or nothing to ask.
     */
    void onUnverified(String why) {
        final String key = hooks.pinnedAs(address, options);
        Log.w(tag, "cannot verify " + key + ": " + why);
        main.post(() -> {
            final Prompt.Handler h = prompts;
            final Prompt.Message prompt = new Prompt.Message(key,
                    why + " Connecting anyway leaves the session unencrypted.",
                    Prompt.Message.Severity.WARNING, true, "Connect anyway") {
                @Override
                protected void deliver(boolean ok) {
                    answerTrust(ok);
                }
            };
            if (h == null) {
                prompt.cancel();
            } else {
                h.message(prompt);
            }
        });
    }

    /** Terminal. Empty {@code detail} for an ordinary disconnect. */
    void onClosed(String detail) {
        Log.i(tag, "closed: " + (detail == null || detail.isEmpty() ? "disconnected" : detail));
        closed(detail == null || detail.isEmpty() ? null : detail);
    }

    // ---- plumbing ----------------------------------------------------------

    private void answerTrust(boolean accept) {
        final long h = handle;
        if (h != 0) {
            natives.answerTrust(h, accept);
        }
    }

    private void closed(String detail) {
        runOnMain(() -> {
            if (state == State.CLOSED) {
                return;
            }
            state = State.CLOSED;
            fireState(State.CLOSED, detail);
        });
    }

    private void fireState(State s, String detail) {
        runOnMain(() -> {
            if (listener != null) {
                listener.state(s, detail);
            }
        });
    }

    private void runOnMain(Runnable r) {
        if (Looper.myLooper() == Looper.getMainLooper()) {
            r.run();
        } else {
            main.post(r);
        }
    }
}
