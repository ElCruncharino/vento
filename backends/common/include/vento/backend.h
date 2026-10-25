// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

// The neutral layer's C face: what a borrowed C or C++ client is written
// against, and what a platform is written to, with neither knowing the other.
//
// It is `backends/common` in Rust — `Events` on the way out and `Session` on
// the way in — said again in C, because three of the clients here are C and
// C++ libraries with a shim of ours around each, and those shims were three
// copies of one JNI binding. What differed between them was the protocol's;
// what was the same was the Android half, and that half is now one adapter in
// `backends/platform/android/jni`, written once against this header.
//
// The shape is `common::Events` and the Java `*Native` classes', method for
// method: `vento_callbacks` is the outward half, fire-and-forget, with every
// question a protocol can ask turned into an event that says one is pending,
// answered later through the session. The inward half is `vento_session`, an
// opaque object reached through a vtable, so that three backends can live in
// one process without a symbol between them clashing and the adapter can be
// written once. Nothing about how a library is driven is decided here —
// the one thread that owns a connection, the command ring it drains, the
// second framebuffer under a writer lock — those are the session files' and
// are forced by the libraries; this header only says what they look like
// from outside.
//
// ## Threads
//
// Every `vento_session_*` call may come from any thread. `read_region` and
// `read_thumbnail` may run concurrently with everything else, including each
// other, and are meant to be called from whichever thread is drawing: they
// take the read side of a lock the protocol thread takes the write side of
// only once per update, so a pixel fetch never queues behind a socket read.
//
// Every callback arrives on a thread of the session's own: the protocol
// thread — the one that owns the connection — or one its library runs beside
// it for a part of the protocol. FreeRDP's dynamic channels have a thread of
// their own, and the graphics pipeline's resizes and paints (`desktop_size`,
// `damaged`, `frame_end`) come on it, so two callbacks of one session can be
// running at once, and each has to be safe for that; the updates of the
// picture are never two at once, since the library makes them all under one
// lock. A callback must not block on anything those threads are waiting for:
// a callback that calls back into the session is fine, a callback that waits
// for a frame is a deadlock. The six questions (`credentials_needed`,
// `trust_needed`, `question`, `unverified`, `security_needed`, `message`)
// are the exception in spirit only: the thread that asked parks after asking,
// and the answer arrives later, through `answer_credentials` or
// `answer_trust`, from whatever thread has it. `closed` is the last callback
// a session makes, and `destroy` returns only after the session's threads
// have ended, so when it returns the callbacks are provably finished and
// whatever `user` pointed at may be freed.
//
// ## Strings
//
// Every `const char *` is UTF-8, NUL-terminated, in both directions. On the
// way out that is a promise the session files keep rather than one the
// libraries make: RFB 3.8 says a desktop name is Latin-1 and every server
// written since sends UTF-8, and a refused connection is reported in the
// library's words, which quote the server's. So a session file passes a
// server's bytes through `vento_utf8_from_bytes` (vento/text.h) — valid
// UTF-8 as itself, anything else as Latin-1, since a name that is not UTF-8
// is still a name — before they reach a callback, and a platform may take
// the string as UTF-8 without looking. Strings handed to a session are the
// caller's and are copied before the call returns. Strings handed to a
// callback are the session's and are gone when the callback returns.
//
// ## Pixels
//
// The framebuffer's word order is `R, G, B, A` in memory — a little-endian
// `uint32_t` of `0xAABBGGRR` — which is what Android's `ARGB_8888` is in
// memory and what a 32-bit RFB format with red at shift 0 decodes to. The
// alpha byte is always `0xFF`: the libraries leave it undefined, and the
// session files fill it in during the copy they make anyway. The cursor
// comes in the same order; a platform that wants `Color`'s `0xAARRGGBB`
// swaps on its side of the call, where it already has to copy, and hashes
// there too — see `common::cursor` for the two things every platform so far
// has wanted.

#ifndef VENTO_BACKEND_H
#define VENTO_BACKEND_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

#ifdef __cplusplus
extern "C" {
#endif

/* The states RealVNC's security prompt carries, which are `Prompt.Trust`'s
   two enums in Java said again in C, in the same order. `security_needed`
   below is the one caller. */
enum {
    VENTO_IDENTITY_OK = 0,              /* known, and matches */
    VENTO_IDENTITY_NEW = 1,             /* never seen before */
    VENTO_IDENTITY_MATCHES_ANOTHER = 2, /* seen, but under a different address */
    VENTO_IDENTITY_CHANGED = 3,         /* seen under this address, and different now */
    VENTO_IDENTITY_MISSING = 4,         /* the server offered none */
    VENTO_IDENTITY_PRESHARED = 5,       /* configured out of band */
    VENTO_IDENTITY_ARD = 6,             /* Apple Remote Desktop, which has none of this kind */
};

enum {
    VENTO_ENCRYPTED = 0,
    VENTO_UNENCRYPTED_WARN = 1,  /* plain text, and the person should be told */
    VENTO_UNENCRYPTED_QUIET = 2, /* plain text, deliberately: a local or tunnelled link */
};

/* How a `message` should look, which is `Prompt.Message.Severity` in C. */
enum {
    VENTO_SEVERITY_INFORMATION = 0,
    VENTO_SEVERITY_QUESTION = 1,
    VENTO_SEVERITY_WARNING = 2,
    VENTO_SEVERITY_ERROR = 3,
};

/* What a session tells the platform. Every member is required: a platform
   implements the whole interface or none of it, as the Rust `Events` trait and
   the Java `Callbacks` interface both demand, and the session files call
   through these without a NULL check — which is the cheapest thing a callback
   can be, and `damaged` runs hundreds of times a frame. A platform that is not
   interested in one writes a function that returns; the Android adapter does
   exactly that where a module's Java interface lacks a method. */
typedef struct vento_callbacks {
    void *user;

    void (*connected)(void *user, int width, int height);
    void (*desktop_size)(void *user, int width, int height);
    void (*damaged)(void *user, int x, int y, int width, int height);
    void (*frame_end)(void *user);
    /* `pixels` is `width * height` words in the framebuffer's own order, or
       NULL with a size of zero where the server has hidden the pointer or asked
       for the default one, both of which mean "draw your own". Alpha is from
       the cursor's mask. The pointer is the session's and is gone when this
       returns. */
    void (*cursor)(void *user, const uint32_t *pixels, int width, int height, int hot_x,
                   int hot_y);
    void (*pointer_mode)(void *user, bool relative);
    void (*bell)(void *user);
    void (*clipboard)(void *user, const char *utf8);
    /* Credentials are wanted, and the protocol thread is stopped until
       `answer_credentials`. `needs_user` is false for VncAuth, which has no
       notion of one. */
    void (*credentials_needed)(void *user, bool needs_user);
    /* A certificate the library could not verify, which is every certificate
       — nobody signs a VNC or RDP server's own — so the fingerprint is the
       identity and the answer, through `answer_trust`, comes from a pin store
       or from a person. `fingerprint` is SHA-256 as `openssl x509 -fingerprint
       -sha256` prints it, so one host reached through two backends is one
       entry. */
    void (*trust_needed)(void *user, const char *fingerprint);
    /* The far end could not be verified at all — a TLS tunnel with no
       certificate under it — and the session wants to know whether to go on.
       Answered through the same `answer_trust`, since a yes and a no mean the
       same things to the protocol thread and only one of these questions can
       be outstanding. */
    void (*unverified)(void *user, const char *why);
    /* The first of three extensions over `common::Events`, and only TigerVNC
       calls it: for an RSA-AES server key, which their viewer shows and asks
       about every time and keeps nothing of, so what crosses here is that
       question — a title and a text, in its words — rather than a fingerprint
       for the platform to decide about. (Its certificates go to
       `trust_needed` like anybody's.) Answered through `answer_trust` like the
       other two, because it is the same question at the protocol thread's
       end. A platform without a dialog answers no. */
    void (*question)(void *user, const char *title, const char *text);
    /* The second extension, and only RealVNC calls it: its core's security
       prompt, which is `trust_needed` and `unverified` asked as one question
       with both answers spelled out — `identity` is a `VENTO_IDENTITY_*` and
       `encryption` a `VENTO_UNENCRYPTED_*` or `VENTO_ENCRYPTED` — because
       that core keeps its own store of identities and decides for itself
       whether a server is new, known or changed, and all this end can add is
       a yes or a no. `signature` is the identity in the core's own form (a
       short hex string, nothing a certificate pin could be compared with),
       `catchphrase` is its human-readable form, and `matching_address` is
       the address the identity is already known under, for
       `VENTO_IDENTITY_MATCHES_ANOTHER`; any of the three may be empty.
       Answered through `answer_trust` like the other three, and a platform
       without a dialog answers no. */
    void (*security_needed)(void *user, int identity, int encryption, const char *signature,
                            const char *catchphrase, const char *matching_address);
    /* The third, and RealVNC's as well: a message its core wants shown, with
       a `VENTO_SEVERITY_*` and whether it is a yes-or-no `question` or an OK
       — most of what goes wrong short of losing the session arrives this way,
       in the core's words, and a refused connection is one of these followed
       by `closed`. `confirm_label` is the affirmative button's own text where
       the core supplied one, and empty otherwise. Answered through
       `answer_trust`, whose yes is the OK or the yes and whose no is the
       cancel or the no; a platform without a dialog answers no, which for a
       plain OK box is the same answer as any other. */
    void (*message)(void *user, const char *text, int severity, bool question,
                    const char *confirm_label);
    /* Terminal, and the last callback this session makes. `detail` is empty
       for an ending nobody needs telling about — the one that was asked for —
       and otherwise says why, in the words the library chose. */
    void (*closed)(void *user, const char *detail);
} vento_callbacks;

/* The strings `info` returns: a row per fact, in the order that backend's
   Java `nativeInfo` documents. Owned by whoever asked, and freed with
   `vento_strings_free`. Chosen over an emit callback because the JNI adapter,
   building a `String[]`, wants the whole list at once, and over a fixed array
   because the backends disagree about how many facts there are. */
typedef struct vento_strings {
    char **items;
    size_t count;
} vento_strings;

static inline void vento_strings_free(vento_strings *strings) {
    for (size_t i = 0; i < strings->count; i++) {
        free(strings->items[i]);
    }
    free(strings->items);
    strings->items = NULL;
    strings->count = 0;
}

/* For a session filling one in: `count` copies of `values`. False, and
   nothing is allocated, on an allocation failure. */
static inline bool vento_strings_set(vento_strings *out, const char *const *values, size_t count) {
    out->items = (char **) calloc(count, sizeof(char *));
    out->count = 0;
    if (!out->items) {
        return false;
    }
    for (size_t i = 0; i < count; i++) {
        const char *value = values[i] ? values[i] : "";
        const size_t bytes = strlen(value) + 1;
        out->items[i] = (char *) malloc(bytes);
        if (!out->items[i]) {
            out->count = i;
            vento_strings_free(out);
            return false;
        }
        memcpy(out->items[i], value, bytes);
    }
    out->count = count;
    return true;
}

/* As many screens as `monitors` can report. Sixteen heads is already absurd,
   and a fixed buffer is what keeps the call allocation-free on both sides;
   libvncclient's own copy stops at the same number. */
#define VENTO_MAX_MONITORS 16

typedef struct vento_session vento_session;

/* What a session can be asked. Each backend fills one of these in, statically,
   and points every session it creates at it. The inline wrappers below are
   the calls; nothing calls through the table by hand. */
typedef struct vento_session_ops {
    void (*pointer)(vento_session *s, int x, int y, int button_mask);
    void (*key_down)(vento_session *s, uint32_t keysym, int64_t key_id);
    void (*key_up)(vento_session *s, int64_t key_id);
    void (*release_all_keys)(vento_session *s);
    void (*focus)(vento_session *s, bool focused);
    void (*view_only)(vento_session *s, bool view_only);
    bool (*can_resize)(vento_session *s);
    /* The far end's screen layout, four ints a screen — x, y, width, height —
       into a buffer of `4 * VENTO_MAX_MONITORS`; returns how many screens,
       which is zero until the server has said. */
    int (*monitors)(vento_session *s, int32_t *out);
    void (*request_desktop_size)(vento_session *s, int width, int height);
    void (*clipboard)(vento_session *s, const char *utf8);
    /* The facts. False, with nothing allocated, where there are none yet. */
    bool (*info)(vento_session *s, vento_strings *out);
    /* Received and sent, in bytes, counted where the protocol meets its
       transport — inside TLS and outside TCP, which is where every backend
       here counts. False while there is nothing to count yet. */
    bool (*traffic)(vento_session *s, uint64_t out[2]);
    /* Copy the desktop rectangle into `dst` at `(dst_x, dst_y)`, 1:1, where
       `dst` is rows of `R, G, B, A` bytes `dst_stride_bytes` apart. The
       caller guarantees the destination holds the region; the session checks
       only that the source does — for any int, without overflowing — and
       that `dst_x` and `dst_y` are not negative, and returns false where
       either fails or there is no framebuffer yet. */
    bool (*read_region)(vento_session *s, int x, int y, int width, int height, void *dst,
                        size_t dst_stride_bytes, int dst_x, int dst_y);
    /* The whole desktop at `1/step`, nearest neighbour, into the top left of
       a destination `dst_w × dst_h` pixels, which the caller has sized to
       `ceil(w/step) × ceil(h/step)`. False, and nothing written, if it does
       not fit — a desktop resized between the sizing and the read — or there
       is no framebuffer yet. */
    bool (*read_thumbnail)(vento_session *s, int step, void *dst, size_t dst_stride_bytes,
                           int dst_w, int dst_h);
    /* The answer to `credentials_needed`. `user` and `domain` may be NULL for
       empty; a NULL `password` cancels, which ends the session. The domain is
       RDP's and the RFB backends ignore it. */
    void (*answer_credentials)(vento_session *s, const char *user, const char *domain,
                               const char *password);
    /* The answer to `trust_needed`, `unverified`, `question`, `security_needed`
       and `message`. A no ends the session, except to a `message` that was not
       a question, where it is the only answer there is. */
    void (*answer_trust)(vento_session *s, bool accept);
    /* Ends the session without waiting for it. Idempotent. */
    void (*disconnect)(vento_session *s);
    /* Ends the session, waits for its thread, and frees it. Nothing may use
       the session afterwards. */
    void (*destroy)(vento_session *s);
} vento_session_ops;

/* The first member of every backend's own session struct, so that a pointer to
   one is a pointer to this. */
struct vento_session {
    const vento_session_ops *ops;
};

static inline void vento_session_pointer(vento_session *s, int x, int y, int button_mask) {
    s->ops->pointer(s, x, y, button_mask);
}

static inline void vento_session_key_down(vento_session *s, uint32_t keysym, int64_t key_id) {
    s->ops->key_down(s, keysym, key_id);
}

static inline void vento_session_key_up(vento_session *s, int64_t key_id) {
    s->ops->key_up(s, key_id);
}

static inline void vento_session_release_all_keys(vento_session *s) {
    s->ops->release_all_keys(s);
}

static inline void vento_session_focus(vento_session *s, bool focused) {
    s->ops->focus(s, focused);
}

static inline void vento_session_view_only(vento_session *s, bool view_only) {
    s->ops->view_only(s, view_only);
}

static inline bool vento_session_can_resize(vento_session *s) {
    return s->ops->can_resize(s);
}

static inline int vento_session_monitors(vento_session *s, int32_t *out) {
    return s->ops->monitors(s, out);
}

static inline void vento_session_request_desktop_size(vento_session *s, int width, int height) {
    s->ops->request_desktop_size(s, width, height);
}

static inline void vento_session_clipboard(vento_session *s, const char *utf8) {
    s->ops->clipboard(s, utf8);
}

static inline bool vento_session_info(vento_session *s, vento_strings *out) {
    return s->ops->info(s, out);
}

static inline bool vento_session_traffic(vento_session *s, uint64_t out[2]) {
    return s->ops->traffic(s, out);
}

static inline bool vento_session_read_region(vento_session *s, int x, int y, int width,
                                             int height, void *dst, size_t dst_stride_bytes,
                                             int dst_x, int dst_y) {
    return s->ops->read_region(s, x, y, width, height, dst, dst_stride_bytes, dst_x, dst_y);
}

static inline bool vento_session_read_thumbnail(vento_session *s, int step, void *dst,
                                                size_t dst_stride_bytes, int dst_w, int dst_h) {
    return s->ops->read_thumbnail(s, step, dst, dst_stride_bytes, dst_w, dst_h);
}

static inline void vento_session_answer_credentials(vento_session *s, const char *user,
                                                    const char *domain, const char *password) {
    s->ops->answer_credentials(s, user, domain, password);
}

static inline void vento_session_answer_trust(vento_session *s, bool accept) {
    s->ops->answer_trust(s, accept);
}

static inline void vento_session_disconnect(vento_session *s) {
    s->ops->disconnect(s);
}

static inline void vento_session_destroy(vento_session *s) {
    s->ops->destroy(s);
}

#ifdef __cplusplus
}
#endif

#endif
