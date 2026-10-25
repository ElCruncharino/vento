// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

// See vento_jni.h for what this is. What is decided here, and why:
//
//   * **Threads attach once and detach when they exit.** A callback from a
//     thread the VM has not seen attaches it and registers a thread-local
//     destructor that detaches it at exit, so the protocol thread pays for
//     registration once rather than once per `frame_end` — which is what the
//     shims used to arrange by hand, each in its own way — and a thread that
//     exits while attached, which aborts the VM, cannot happen. The Java
//     threads calling in are attached already and take the fast path.
//   * **A Java exception out of a callback is a log line, not an abort.**
//     Anything a callback throws is pending on this thread when it returns,
//     and every JNI call made while an exception is pending is undefined —
//     including the detach above, which runs the thread's uncaught-exception
//     handler and takes the process with it. So it is reported and cleared
//     after every call, and a Java fault costs a frame rather than the
//     session. The same argument `jni`'s `LogErrorAndDefault` makes.
//   * **Every string the session hands over goes through `NewString`, not
//     `NewStringUTF`.** The latter takes *modified* UTF-8: a four-byte
//     sequence is not a string it can be given at all, and a build with
//     CheckJNI on aborts rather than producing mojibake. The session layer
//     promises UTF-8, so the conversion here is the plain one — with the
//     Latin-1 fallback the shims used to apply kept behind it, since a
//     `NewString` of something is still better than an abort over a byte a
//     server chose. In the other direction, `GetStringUTFChars` gives the
//     same modified UTF-8 back, in which a character outside the basic plane
//     is a surrogate pair of three-byte sequences — which TigerVNC's clipboard
//     validator rejects outright — so inbound strings go through UTF-16 too.
//   * **The cursor is swapped and hashed here.** The session's pixels are the
//     framebuffer's `R, G, B, A`; Java's `Color` is `0xAARRGGBB`, and the hash
//     is FNV-1a over the packed pixels, which is `CursorCache`'s identity and
//     has to agree bit for bit with `common::cursor::hash` and the Java copy.
//   * **A method the module's `Callbacks` does not declare is skipped.** The
//     three interfaces are one shape less a method or two — libvnc has no
//     `onPointerMode`, FreeRDP no `onUnverified`, only TigerVNC an
//     `onQuestion` (for an RSA-AES server key) — and resolving each optionally is what lets
//     one adapter serve all three. For the questions that is more than
//     tidiness: a question nobody can hear is answered no below, so the
//     protocol thread is not left parked behind it — which is also all this
//     adapter does with the two RealVNC asks (`security_needed`, `message`),
//     since that backend's Android half is Java over the core's own surface
//     and never comes this way.

#include "vento_jni.h"

#include <android/bitmap.h>
#include <android/log.h>
#include <pthread.h>
#include <stdarg.h>
#include <stdlib.h>
#include <string.h>

#include <vento/log.h>

#define TAG "VentoJni"

/* ---- logging ------------------------------------------------------------ */

void vento_log(int level, const char *tag, const char *fmt, ...) {
    va_list ap;
    va_start(ap, fmt);
    __android_log_vprint(level == VENTO_LOG_WARN ? ANDROID_LOG_WARN : ANDROID_LOG_INFO, tag, fmt, ap);
    va_end(ap);
}

/* ---- the session's Android side ----------------------------------------- */

struct vento_jni_session {
    JavaVM *vm;
    jobject target; /* global ref to the Callbacks */
    vento_session *session;
    vento_callbacks callbacks;
};

/* Per module rather than per session: one class per backend implements the
   interface, and resolving `onDamage` by name hundreds of times a frame
   would be the most expensive thing in the path. */
static jclass gCallbacksClass;
static jmethodID mConnected, mDesktopSize, mDamage, mFrameEnd, mCursor, mPointerMode, mBell,
        mClipboard, mCredentialsNeeded, mTrustNeeded, mUnverified, mQuestion, mClosed;

static void cleared(JNIEnv *env) {
    if ((*env)->ExceptionCheck(env)) {
        (*env)->ExceptionDescribe(env);
        (*env)->ExceptionClear(env);
    }
}

/* A method the interface may lack: NULL rather than a pending
   NoSuchMethodError. */
static jmethodID optional(JNIEnv *env, jclass k, const char *name, const char *sig) {
    jmethodID m = (*env)->GetMethodID(env, k, name, sig);
    if (!m) {
        (*env)->ExceptionClear(env);
    }
    return m;
}

static void resolve(JNIEnv *env, jobject listener) {
    if (gCallbacksClass) {
        return;
    }
    jclass k = (*env)->GetObjectClass(env, listener);
    mConnected = optional(env, k, "onConnected", "(II)V");
    mDesktopSize = optional(env, k, "onDesktopSize", "(II)V");
    mDamage = optional(env, k, "onDamage", "(IIII)V");
    mFrameEnd = optional(env, k, "onFrameEnd", "()V");
    mCursor = optional(env, k, "onCursor", "([IIIIIJ)V");
    mPointerMode = optional(env, k, "onPointerMode", "(Z)V");
    mBell = optional(env, k, "onBell", "()V");
    mClipboard = optional(env, k, "onClipboard", "(Ljava/lang/String;)V");
    mCredentialsNeeded = optional(env, k, "onCredentialsNeeded", "(Z)V");
    mTrustNeeded = optional(env, k, "onTrustNeeded", "(Ljava/lang/String;)V");
    mUnverified = optional(env, k, "onUnverified", "(Ljava/lang/String;)V");
    mQuestion = optional(env, k, "onQuestion", "(Ljava/lang/String;Ljava/lang/String;)V");
    mClosed = optional(env, k, "onClosed", "(Ljava/lang/String;)V");
    /* Last, and that is the point: it is what every other caller tests, so
       publishing it first would let a second one past while the method ids
       above were still zero. */
    gCallbacksClass = (*env)->NewGlobalRef(env, k);
    (*env)->DeleteLocalRef(env, k);
}

/* ---- threads ------------------------------------------------------------ */

static pthread_key_t gAttached;
static pthread_once_t gAttachedOnce = PTHREAD_ONCE_INIT;

static void detachAtExit(void *vm) {
    (*(JavaVM *) vm)->DetachCurrentThread((JavaVM *) vm);
}

static void makeAttachedKey(void) {
    pthread_key_create(&gAttached, detachAtExit);
}

/* This thread's env, attaching it for the rest of its life if need be. NULL
   if the VM would not have it, which is a callback lost rather than a crash. */
static JNIEnv *envOf(JavaVM *vm) {
    JNIEnv *env = NULL;
    if ((*vm)->GetEnv(vm, (void **) &env, JNI_VERSION_1_6) == JNI_OK) {
        return env;
    }
    if ((*vm)->AttachCurrentThread(vm, &env, NULL) != JNI_OK) {
        return NULL;
    }
    pthread_once(&gAttachedOnce, makeAttachedKey);
    pthread_setspecific(gAttached, vm);
    return env;
}

/* ---- strings ------------------------------------------------------------ */

/* UTF-8 to a Java string. Valid UTF-8 as UTF-8 and anything else as Latin-1,
   decoded in one pass with a rewind, since "is this valid UTF-8" is only
   answerable by decoding it. The session layer promises the first case; the
   second is kept because what it costs is nothing. */
static jstring toJava(JNIEnv *env, const char *bytes) {
    if (!bytes) {
        bytes = "";
    }
    const size_t n = strlen(bytes);
    jchar *out = malloc((n + 1) * sizeof(jchar));
    if (!out) {
        return NULL;
    }
    size_t w = 0;
    int utf8 = 1;
    for (size_t i = 0; i < n && utf8;) {
        const uint8_t c = (uint8_t) bytes[i];
        int extra;
        uint32_t cp;
        if (c < 0x80) { cp = c; extra = 0; }
        else if ((c & 0xE0) == 0xC0) { cp = c & 0x1Fu; extra = 1; }
        else if ((c & 0xF0) == 0xE0) { cp = c & 0x0Fu; extra = 2; }
        else if ((c & 0xF8) == 0xF0) { cp = c & 0x07u; extra = 3; }
        else { utf8 = 0; break; }
        if (i + (size_t) extra >= n) {
            utf8 = 0;
            break;
        }
        for (int k = 1; k <= extra; k++) {
            const uint8_t cc = (uint8_t) bytes[i + (size_t) k];
            if ((cc & 0xC0) != 0x80) {
                utf8 = 0;
                break;
            }
            cp = (cp << 6) | (cc & 0x3Fu);
        }
        if (!utf8) {
            break;
        }
        i += (size_t) extra + 1;
        if (cp >= 0x10000) {
            cp -= 0x10000;
            out[w++] = (jchar) (0xD800 + (cp >> 10));
            out[w++] = (jchar) (0xDC00 + (cp & 0x3FF));
        } else {
            out[w++] = (jchar) cp;
        }
    }
    if (!utf8) {
        /* One code point a byte, which is one UTF-16 unit each. */
        for (w = 0; w < n; w++) {
            out[w] = (jchar) (uint8_t) bytes[w];
        }
    }
    jstring js = (*env)->NewString(env, out, (jsize) w);
    free(out);
    return js;
}

char *vento_jni_string(JNIEnv *env, jstring js) {
    if (!js) {
        return NULL;
    }
    const jsize n = (*env)->GetStringLength(env, js);
    const jchar *chars = (*env)->GetStringChars(env, js, NULL);
    if (!chars) {
        return NULL;
    }
    /* Four bytes a unit is the most a UTF-16 unit can cost. */
    char *out = malloc((size_t) n * 4 + 1);
    if (!out) {
        (*env)->ReleaseStringChars(env, js, chars);
        return NULL;
    }
    size_t w = 0;
    for (jsize i = 0; i < n; i++) {
        uint32_t cp = chars[i];
        if (cp >= 0xD800 && cp <= 0xDBFF && i + 1 < n
                && chars[i + 1] >= 0xDC00 && chars[i + 1] <= 0xDFFF) {
            cp = 0x10000 + ((cp - 0xD800) << 10) + (chars[++i] - 0xDC00);
        }
        if (cp < 0x80) {
            out[w++] = (char) cp;
        } else if (cp < 0x800) {
            out[w++] = (char) (0xC0 | (cp >> 6));
            out[w++] = (char) (0x80 | (cp & 0x3F));
        } else if (cp < 0x10000) {
            out[w++] = (char) (0xE0 | (cp >> 12));
            out[w++] = (char) (0x80 | ((cp >> 6) & 0x3F));
            out[w++] = (char) (0x80 | (cp & 0x3F));
        } else {
            out[w++] = (char) (0xF0 | (cp >> 18));
            out[w++] = (char) (0x80 | ((cp >> 12) & 0x3F));
            out[w++] = (char) (0x80 | ((cp >> 6) & 0x3F));
            out[w++] = (char) (0x80 | (cp & 0x3F));
        }
    }
    out[w] = '\0';
    (*env)->ReleaseStringChars(env, js, chars);
    return out;
}

/* ---- callbacks into Java (protocol thread) ------------------------------ */

static void callVoid(vento_jni_session *h, jmethodID method, ...) {
    if (!method) {
        return;
    }
    JNIEnv *env = envOf(h->vm);
    if (!env) {
        return;
    }
    va_list ap;
    va_start(ap, method);
    (*env)->CallVoidMethodV(env, h->target, method, ap);
    va_end(ap);
    cleared(env);
}

static void callString(vento_jni_session *h, jmethodID method, const char *utf8) {
    if (!method) {
        return;
    }
    JNIEnv *env = envOf(h->vm);
    if (!env) {
        return;
    }
    jstring js = toJava(env, utf8);
    (*env)->CallVoidMethod(env, h->target, method, js);
    cleared(env);
    if (js) {
        (*env)->DeleteLocalRef(env, js);
    }
}

static void cbConnected(void *user, int width, int height) {
    callVoid(user, mConnected, (jint) width, (jint) height);
}

static void cbDesktopSize(void *user, int width, int height) {
    callVoid(user, mDesktopSize, (jint) width, (jint) height);
}

static void cbDamaged(void *user, int x, int y, int width, int height) {
    callVoid(user, mDamage, (jint) x, (jint) y, (jint) width, (jint) height);
}

static void cbFrameEnd(void *user) {
    callVoid(user, mFrameEnd);
}

/* FNV-1a over a cursor's size and its `Color`-packed pixels, which is its
   identity in `CursorCache` — so this, `common::cursor::hash` and the Java one
   all have to agree, bit for bit. What it saves on the other side is a bitmap
   and a texture per cursor change rather than anything on the wire. */
static jlong cursorHash(const jint *argb, int width, int height) {
    uint64_t h = 0xcbf29ce484222325ull;
    h = (h ^ (uint32_t) width) * 0x100000001b3ull;
    h = (h ^ (uint32_t) height) * 0x100000001b3ull;
    for (int i = 0; i < width * height; i++) {
        h = (h ^ (uint32_t) argb[i]) * 0x100000001b3ull;
    }
    return (jlong) h;
}

static void cbCursor(void *user, const uint32_t *pixels, int width, int height, int hotX,
                     int hotY) {
    vento_jni_session *h = user;
    if (!mCursor) {
        return;
    }
    JNIEnv *env = envOf(h->vm);
    if (!env) {
        return;
    }
    jintArray argb = NULL;
    jlong hash = 0;
    if (pixels && width > 0 && height > 0) {
        const int n = width * height;
        argb = (*env)->NewIntArray(env, n);
        if (argb) {
            jint *out = (*env)->GetIntArrayElements(env, argb, NULL);
            if (out) {
                /* `0xAABBGGRR` to `0xAARRGGBB`: red and blue trade places here
                   rather than on a thread that is drawing, and the hash is
                   taken here too, because the pixels are already in cache
                   from the swap. */
                for (int i = 0; i < n; i++) {
                    const uint32_t p = pixels[i];
                    out[i] = (jint) ((p & 0xFF00FF00u) | ((p & 0x00FF0000u) >> 16)
                                     | ((p & 0x000000FFu) << 16));
                }
                hash = cursorHash(out, width, height);
                (*env)->ReleaseIntArrayElements(env, argb, out, 0);
            }
        } else {
            cleared(env);
        }
    }
    (*env)->CallVoidMethod(env, h->target, mCursor, argb, (jint) width, (jint) height,
                           (jint) hotX, (jint) hotY, hash);
    cleared(env);
    if (argb) {
        (*env)->DeleteLocalRef(env, argb);
    }
}

static void cbPointerMode(void *user, bool relative) {
    callVoid(user, mPointerMode, (jboolean) relative);
}

static void cbBell(void *user) {
    callVoid(user, mBell);
}

static void cbClipboard(void *user, const char *utf8) {
    callString(user, mClipboard, utf8);
}

/* A question the module's `Callbacks` cannot hear is answered the way a
   cancelled dialog answers it, so the protocol thread is not left parked
   behind it. None of the three interfaces lacks a question its backend asks,
   so this is the shape of the contract rather than a path anything takes. */
static void refuse(vento_jni_session *h, const char *question) {
    vento_log(VENTO_LOG_WARN, TAG, "no %s in this Callbacks; answering no", question);
    vento_session *s = __atomic_load_n(&h->session, __ATOMIC_ACQUIRE);
    if (s) {
        vento_session_answer_trust(s, false);
    }
}

static void cbCredentialsNeeded(void *user, bool needsUser) {
    vento_jni_session *h = user;
    if (!mCredentialsNeeded) {
        vento_log(VENTO_LOG_WARN, TAG, "no onCredentialsNeeded in this Callbacks; cancelling");
        vento_session *s = __atomic_load_n(&h->session, __ATOMIC_ACQUIRE);
        if (s) {
            vento_session_answer_credentials(s, NULL, NULL, NULL);
        }
        return;
    }
    callVoid(h, mCredentialsNeeded, (jboolean) needsUser);
}

static void cbTrustNeeded(void *user, const char *fingerprint) {
    vento_jni_session *h = user;
    if (!mTrustNeeded) {
        refuse(h, "onTrustNeeded");
        return;
    }
    callString(h, mTrustNeeded, fingerprint);
}

static void cbUnverified(void *user, const char *why) {
    vento_jni_session *h = user;
    if (!mUnverified) {
        refuse(h, "onUnverified");
        return;
    }
    callString(h, mUnverified, why);
}

static void cbQuestion(void *user, const char *title, const char *text) {
    vento_jni_session *h = user;
    if (!mQuestion) {
        refuse(h, "onQuestion");
        return;
    }
    JNIEnv *env = envOf(h->vm);
    if (!env) {
        return;
    }
    jstring jTitle = toJava(env, title);
    jstring jText = toJava(env, text);
    (*env)->CallVoidMethod(env, h->target, mQuestion, jTitle, jText);
    cleared(env);
    if (jTitle) {
        (*env)->DeleteLocalRef(env, jTitle);
    }
    if (jText) {
        (*env)->DeleteLocalRef(env, jText);
    }
}

/* The two RealVNC asks, which no module's `Callbacks` declares: RealVNC's
   Android half is Java over the core's own JNI surface and never comes
   through this adapter, so these are the contract's shape here, as `refuse`
   says above, and answer no. */
static void cbSecurityNeeded(void *user, int identity, int encryption, const char *signature,
                             const char *catchphrase, const char *matchingAddress) {
    (void) identity;
    (void) encryption;
    (void) signature;
    (void) catchphrase;
    (void) matchingAddress;
    refuse(user, "onSecurityNeeded");
}

static void cbMessage(void *user, const char *text, int severity, bool question,
                      const char *confirmLabel) {
    (void) text;
    (void) severity;
    (void) question;
    (void) confirmLabel;
    refuse(user, "onMessage");
}

static void cbClosed(void *user, const char *detail) {
    callString(user, mClosed, detail);
}

/* ---- create and destroy ------------------------------------------------- */

vento_jni_session *vento_jni_begin(JNIEnv *env, jobject listener) {
    resolve(env, listener);
    vento_jni_session *h = calloc(1, sizeof(*h));
    if (!h) {
        return NULL;
    }
    (*env)->GetJavaVM(env, &h->vm);
    h->target = (*env)->NewGlobalRef(env, listener);
    if (!h->target) {
        free(h);
        return NULL;
    }
    h->callbacks.user = h;
    h->callbacks.connected = cbConnected;
    h->callbacks.desktop_size = cbDesktopSize;
    h->callbacks.damaged = cbDamaged;
    h->callbacks.frame_end = cbFrameEnd;
    h->callbacks.cursor = cbCursor;
    h->callbacks.pointer_mode = cbPointerMode;
    h->callbacks.bell = cbBell;
    h->callbacks.clipboard = cbClipboard;
    h->callbacks.credentials_needed = cbCredentialsNeeded;
    h->callbacks.trust_needed = cbTrustNeeded;
    h->callbacks.unverified = cbUnverified;
    h->callbacks.question = cbQuestion;
    h->callbacks.security_needed = cbSecurityNeeded;
    h->callbacks.message = cbMessage;
    h->callbacks.closed = cbClosed;
    return h;
}

const vento_callbacks *vento_jni_callbacks(const vento_jni_session *h) {
    return &h->callbacks;
}

jlong vento_jni_finish(JNIEnv *env, vento_jni_session *h, vento_session *s) {
    if (!s) {
        (*env)->DeleteGlobalRef(env, h->target);
        free(h);
        return 0;
    }
    /* The protocol thread is already running by now and may read this from a
       callback, so it is published rather than assigned. */
    __atomic_store_n(&h->session, s, __ATOMIC_RELEASE);
    return (jlong) (intptr_t) h;
}

static vento_jni_session *handleOf(jlong handle) {
    return (vento_jni_session *) (intptr_t) handle;
}

vento_session *vento_jni_session_of(jlong handle) {
    vento_jni_session *h = handleOf(handle);
    return h ? h->session : NULL;
}

void vento_jni_destroy(JNIEnv *env, jlong handle) {
    vento_jni_session *h = handleOf(handle);
    if (!h) {
        return;
    }
    vento_session_destroy(h->session);
    (*env)->DeleteGlobalRef(env, h->target);
    free(h);
}

/* ---- the uniform natives ------------------------------------------------ */

void vento_jni_answer_credentials(JNIEnv *env, jlong handle, jstring userName, jstring domain,
                                  jstring password) {
    vento_session *s = vento_jni_session_of(handle);
    if (!s) {
        return;
    }
    char *u = vento_jni_string(env, userName);
    char *d = vento_jni_string(env, domain);
    char *p = vento_jni_string(env, password);
    vento_session_answer_credentials(s, u, d, p);
    free(u);
    free(d);
    free(p);
}

void vento_jni_answer_trust(jlong handle, jboolean accept) {
    vento_session *s = vento_jni_session_of(handle);
    if (s) {
        vento_session_answer_trust(s, accept != JNI_FALSE);
    }
}

void vento_jni_disconnect(jlong handle) {
    vento_session *s = vento_jni_session_of(handle);
    if (s) {
        vento_session_disconnect(s);
    }
}

void vento_jni_pointer(jlong handle, jint x, jint y, jint buttonMask) {
    vento_session *s = vento_jni_session_of(handle);
    if (s) {
        vento_session_pointer(s, x, y, buttonMask);
    }
}

void vento_jni_key_down(jlong handle, jint keysym, jlong keyId) {
    vento_session *s = vento_jni_session_of(handle);
    if (s) {
        vento_session_key_down(s, (uint32_t) keysym, keyId);
    }
}

void vento_jni_key_up(jlong handle, jlong keyId) {
    vento_session *s = vento_jni_session_of(handle);
    if (s) {
        vento_session_key_up(s, keyId);
    }
}

void vento_jni_release_all_keys(jlong handle) {
    vento_session *s = vento_jni_session_of(handle);
    if (s) {
        vento_session_release_all_keys(s);
    }
}

void vento_jni_focus(jlong handle, jboolean focused) {
    vento_session *s = vento_jni_session_of(handle);
    if (s) {
        vento_session_focus(s, focused != JNI_FALSE);
    }
}

void vento_jni_view_only(jlong handle, jboolean viewOnly) {
    vento_session *s = vento_jni_session_of(handle);
    if (s) {
        vento_session_view_only(s, viewOnly != JNI_FALSE);
    }
}

jboolean vento_jni_can_resize(jlong handle) {
    vento_session *s = vento_jni_session_of(handle);
    return s && vento_session_can_resize(s) ? JNI_TRUE : JNI_FALSE;
}

jintArray vento_jni_monitors(JNIEnv *env, jlong handle) {
    vento_session *s = vento_jni_session_of(handle);
    int32_t flat[4 * VENTO_MAX_MONITORS];
    const int n = s ? vento_session_monitors(s, flat) : 0;
    jintArray out = (*env)->NewIntArray(env, n * 4);
    if (out && n > 0) {
        (*env)->SetIntArrayRegion(env, out, 0, n * 4, flat);
    }
    return out;
}

void vento_jni_request_desktop_size(jlong handle, jint width, jint height) {
    vento_session *s = vento_jni_session_of(handle);
    if (s) {
        vento_session_request_desktop_size(s, width, height);
    }
}

void vento_jni_clipboard(JNIEnv *env, jlong handle, jstring text) {
    vento_session *s = vento_jni_session_of(handle);
    if (!s || !text) {
        return;
    }
    char *utf8 = vento_jni_string(env, text);
    if (utf8) {
        vento_session_clipboard(s, utf8);
        free(utf8);
    }
}

jobjectArray vento_jni_info(JNIEnv *env, jlong handle) {
    vento_session *s = vento_jni_session_of(handle);
    vento_strings facts;
    if (!s || !vento_session_info(s, &facts)) {
        return NULL;
    }
    jclass string = (*env)->FindClass(env, "java/lang/String");
    jobjectArray out = (*env)->NewObjectArray(env, (jsize) facts.count, string, NULL);
    if (out) {
        for (size_t i = 0; i < facts.count; i++) {
            jstring js = toJava(env, facts.items[i]);
            (*env)->SetObjectArrayElement(env, out, (jsize) i, js);
            if (js) {
                (*env)->DeleteLocalRef(env, js);
            }
        }
    }
    (*env)->DeleteLocalRef(env, string);
    vento_strings_free(&facts);
    return out;
}

jlongArray vento_jni_traffic(JNIEnv *env, jlong handle) {
    vento_session *s = vento_jni_session_of(handle);
    uint64_t counts[2];
    if (!s || !vento_session_traffic(s, counts)) {
        return NULL;
    }
    const jlong both[2] = {(jlong) counts[0], (jlong) counts[1]};
    jlongArray out = (*env)->NewLongArray(env, 2);
    if (out) {
        (*env)->SetLongArrayRegion(env, out, 0, 2, both);
    }
    return out;
}

/* ---- pixels, from whichever thread is drawing ---------------------------- */

/* The three NDK calls that let pixels be written straight into a `Bitmap`,
   which is what `Backend.readRegion` promises: a row copy out of the
   session's framebuffer into the bitmap about to be drawn, with no buffer
   between. The unlock has to happen on every path out, including the one
   where the region turns out not to fit — a bitmap left locked is one Java
   can never draw again. */
static int lockBitmap(JNIEnv *env, jobject bitmap, AndroidBitmapInfo *info, void **pixels) {
    if (AndroidBitmap_getInfo(env, bitmap, info) != ANDROID_BITMAP_RESULT_SUCCESS
            || info->format != ANDROID_BITMAP_FORMAT_RGBA_8888) {
        return 0;
    }
    *pixels = NULL;
    if (AndroidBitmap_lockPixels(env, bitmap, pixels) != ANDROID_BITMAP_RESULT_SUCCESS
            || !*pixels) {
        return 0;
    }
    return 1;
}

jboolean vento_jni_read_region(JNIEnv *env, jlong handle, jint x, jint y, jint w, jint h,
                               jobject bitmap, jint dstX, jint dstY) {
    vento_session *s = vento_jni_session_of(handle);
    if (!s || w <= 0 || h <= 0 || dstX < 0 || dstY < 0) {
        return JNI_FALSE;
    }
    AndroidBitmapInfo info;
    if (AndroidBitmap_getInfo(env, bitmap, &info) != ANDROID_BITMAP_RESULT_SUCCESS
            || (uint64_t) dstX + (uint64_t) w > info.width
            || (uint64_t) dstY + (uint64_t) h > info.height) {
        return JNI_FALSE;
    }
    void *dst;
    if (!lockBitmap(env, bitmap, &info, &dst)) {
        return JNI_FALSE;
    }
    const bool ok = vento_session_read_region(s, x, y, w, h, dst, info.stride, dstX, dstY);
    AndroidBitmap_unlockPixels(env, bitmap);
    return ok ? JNI_TRUE : JNI_FALSE;
}

jboolean vento_jni_read_thumbnail(JNIEnv *env, jlong handle, jint step, jobject bitmap) {
    vento_session *s = vento_jni_session_of(handle);
    if (!s || step < 1) {
        return JNI_FALSE;
    }
    AndroidBitmapInfo info;
    void *dst;
    if (!lockBitmap(env, bitmap, &info, &dst)) {
        return JNI_FALSE;
    }
    const bool ok = vento_session_read_thumbnail(s, step, dst, info.stride, (int) info.width,
                                                 (int) info.height);
    AndroidBitmap_unlockPixels(env, bitmap);
    return ok ? JNI_TRUE : JNI_FALSE;
}
