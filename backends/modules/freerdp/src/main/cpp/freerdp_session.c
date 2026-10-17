// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

// FreeRDP's own client as a `vento_session`, so that above this file it is the
// same shape as the IronRDP backend and a disagreement between them is a
// disagreement between the clients rather than between two ways of binding one.
// That is the whole point of the second RDP client: every RFB question here has
// been answerable by asking the other two clients, and every RDP one has had a
// single answer. Nothing in here names a platform: what the phone needs of it
// is the Rust adapter in `src/main/rust`, linked into the same library, which
// the one JNI layer in `backends/platform/android` stands in front of.
//
// Three things are the library's decisions rather than ours, and each cost a
// session to find out (they were found by driving the library on the phone with
// no backend around it, before this file existed):
//
//   1. **The library installs command-line prompts over the callbacks you
//      set.** Creating the context puts them in unconditionally, and
//      `freerdp_client_start` does it again when `UseCommonStdioCallbacks` is
//      on — so ours go in after the context exists, and the flag is stated
//      off. On a phone there is no terminal, so those prompts refuse, and the
//      refusal is reported as a TLS failure over a handshake that in fact
//      succeeded.
//   2. **`update->DesktopResize` is not optional.** `gdi_ResetGraphics` asserts
//      on it, so no callback is `abort()` rather than a missing feature.
//   3. **The graphics pipeline paints nothing unless it is joined to the GDI**
//      when its channel connects. It negotiates perfectly either way, which is
//      a black desktop with a healthy-looking connection behind it.
//
// (A fourth is Android's and is in this module's CMakeLists.txt: WinPR defines
// a `JNI_OnLoad` of its own, so it is compiled under another name and the one
// the library exports, the Rust crate's, calls it.)
//
// And two are ours, both forced by the seam rather than chosen:
//
//   * One thread owns the connection and every call into the session is a
//     queued command it drains, woken by an event of ours in the library's own
//     handle set — because setting a flag does not wake a thread inside
//     `WaitForMultipleObjects`.
//   * There are two framebuffers. The GDI decodes into its own between
//     `BeginPaint` and `EndPaint`, and the seam promises that pixels may be read
//     from the drawing thread at any moment; so the damage is copied under a
//     writer lock when the batch ends, and reads are served from the copy. The
//     alternative is holding that lock across a decode, which puts the drawing
//     thread behind an H.264 frame.

#include <pthread.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include <freerdp/freerdp.h>
#include <freerdp/client.h>
#include <freerdp/client/channels.h>
#include <freerdp/client/cliprdr.h>
#include <freerdp/client/cmdline.h>
#include <freerdp/crypto/certificate.h>
#include <freerdp/client/disp.h>
#include <freerdp/channels/rdpgfx.h>
#include <freerdp/gdi/gdi.h>
#include <freerdp/gdi/gfx.h>
#include <freerdp/graphics.h>
#include <freerdp/autodetect.h>
#include <freerdp/settings.h>
#include <winpr/string.h>
#include <winpr/synch.h>
#include <openssl/tls1.h>

#include <vento/backend.h>
#include <vento/freerdp.h>
#include <vento/log.h>
#include <vento/text.h>

#include "h264_vento.h"

#define TAG "FreeRdp"
#define LOGI(...) vento_log(VENTO_LOG_INFO, TAG, __VA_ARGS__)
#define LOGW(...) vento_log(VENTO_LOG_WARN, TAG, __VA_ARGS__)

#define CMD_RING 512
#define MAX_HELD_KEYS 64

enum {
    CMD_POINTER = 1,
    CMD_KEY_DOWN,
    CMD_KEY_UP,
    CMD_RELEASE_KEYS,
    CMD_CLIPBOARD,
    CMD_FOCUS,
    CMD_RESIZE,
    CMD_LAYOUT,
    CMD_DISCONNECT,
};

typedef struct {
    int type;
    int a, b;
    INT64 c;
    WCHAR *text; /* owned by the queue */
    size_t textLen;
} Cmd;

typedef struct Session Session;

/* The context FreeRDP allocates for us: its own, and a pointer back to ours.
   `rdpClientContext` is what the client layer keeps in the allocation, so
   `ContextSize` has to name a struct that starts with one. The library
   allocates exactly what it is told; a size short of that is refused by the
   client layer call by call, each one failing with an "application bug" in
   the log. */
typedef struct {
    rdpClientContext client;
    Session *session;
} ShimContext;

struct Session {
    vento_session base; /* first, so that a pointer to this is a pointer to that */
    vento_callbacks cb;

    ShimContext *ctx; /* protocol thread; published under both locks */
    freerdp *instance;

    pthread_t thread;
    HANDLE wake; /* the doorbell in the library's handle set */

    pthread_mutex_t lock;
    pthread_cond_t cond;

    Cmd ring[CMD_RING];
    int head, tail;

    int quit;
    int started;
    int connected;
    int closedReported;
    int sawPointerPosition; /* protocol thread only: log the first, not the stream */
    _Atomic int viewOnly;
    _Atomic int canResize;

    /* the credential prompt, answered through the session */
    int credAnswered;
    char *credUser, *credDomain, *credPass;

    /* and the certificate one, which is the same shape */
    int trustAnswered, trustAccepted;

    /* the copy readRegion is served from */
    pthread_rwlock_t fbLock;
    uint8_t *fb;
    int fbW, fbH;

    /* The channels, kept because they arrive as an event rather than from a
       call: a channel's interface is handed over once, when it connects. */
    CliprdrClientContext *cliprdr;
    DispClientContext *disp;
    _Atomic int gfxOpen; /* the graphics pipeline's channel is up */
    /* What the server confirmed of the capability sets the pipeline offered:
       the version, and whether that version with its flags carries H.264. */
    _Atomic UINT32 gfxVersion;
    _Atomic int gfxH264;

    /* The codec of the last surface the server sent, and the library's own
       handler it is read on the way to. Asking the settings instead answers
       what was *requested*: nothing on a client clears RemoteFxCodec when the
       server declines it, and a server may decline for reasons two rows away —
       xrdp offers the codec only to a client that calls its link a LAN. */
    _Atomic UINT32 lastCodec;
    pSurfaceBits gdiSurfaceBits;
    /* The GDI's handler for the pipeline's reset, chained through: the reset
       is where a server on the pipeline states its monitors. */
    pcRdpgfxResetGraphics gdiResetGraphics;
    /* And its handler for every surface command, chained through so that
       the H.264 contexts the library makes inside it are this session's
       (h264_vento.h). */
    pcRdpgfxSurfaceCommand gdiSurfaceCommand;
    /* The capability set the server confirmed, to advertise again where a
       decoder can no longer follow the stream, and whether that has been
       done since the server last confirmed one. */
    RDPGFX_CAPSET gfxCaps;
    _Atomic int gfxRestarting;
    /* The platform's decoders, which every H.264 surface command goes
       through, and what they say about the last picture. */
    VentoH264 h264;

    /* The desktop's monitors, x, y, width, height each, under `lock`: as the
       server last stated them (`serverCount`, -1 before it has), and as this
       end last asked for them (`askedCount`, at connect and on every layout
       sent). The server's is the answer where it has given one that covers the
       desktop as it now is; what was asked is the answer where the desktop
       came out that size; anything else is a desktop nothing here can divide.
       `wantCount` is the layout op_request_layout left for the protocol thread
       to send, the latest one only. */
    int32_t serverLayout[4 * VENTO_MAX_MONITORS];
    int serverCount;
    int32_t askedLayout[4 * VENTO_MAX_MONITORS];
    int askedCount;
    int32_t wantLayout[4 * VENTO_MAX_MONITORS];
    int wantCount;

    /* What this phone has copied, as the wire wants it — UTF-16 with its
       terminator — held until something over there pastes. RDP's clipboard is a
       delayed rendering: the format list says there is text and the bytes go
       only when they are asked for. */
    WCHAR *clipOut;
    size_t clipOutLen;

    struct {
        INT64 id;
        UINT16 code;  /* RDP scancode, with 0xE000 for an extended one */
        int shift;    /* the layout needed a Shift for it, and we pressed one */
        WCHAR unicode; /* or it had no position at all */
    } keys[MAX_HELD_KEYS];
    int nkeys;
    int shiftHeld; /* by the person, not by us */

    UINT16 buttons; /* the RFB mask as last seen, because RDP wants the edges */

    char *host;
    int port;
    /* Why the address was not one, empty if it was: a session is created and
       started in one call, so the protocol thread is the first place there is
       to say it. */
    char addressError[160];
    char *user, *domain, *password;
    char *nla;
    int legacyTls;
    char *experience;
    char *graphics;
    char *sound;
    char *clientName;
    UINT32 osMajorType;
    char *configPath;
    int width, height, monitors, keyboardLayout;
    int scale; /* percent; what the far end is asked to draw its own interface at */
    int compression;
    int connectTimeoutMs;
};

static const vento_session_ops OPS;

/* ---- the address -------------------------------------------------------- */

/* An address is `host`, `host:port`, `[literal]` or `[literal]:port`, and a bare
   IPv6 literal is refused in words rather than guessed at: unbracketed there is
   no telling it from a host with a port after it, and what the guess this
   replaced produced was the address truncated at its last colon. No display
   number here, which is a VNC habit RDP has never had, so a second colon has
   nothing to mean. The host is left at the front of `address`, which stays the
   caller's buffer to free; `why` is empty unless there is nothing to dial. */
static void splitAddress(char *address, int *port, char *why, size_t whyLen) {
    size_t len = strlen(address);
    while (len > 0 && (address[len - 1] == ' ' || address[len - 1] == '\t')) {
        address[--len] = 0;
    }
    const size_t lead = strspn(address, " \t");
    if (lead > 0) {
        memmove(address, address + lead, len - lead + 1);
        len -= lead;
    }
    if (len == 0 || address[0] == ':') {
        snprintf(why, whyLen, "No host in address");
        return;
    }

    const char *host = address;
    size_t hostLen = len;
    const char *number = NULL;

    if (address[0] == '[') {
        const char *close = strchr(address, ']');
        if (!close) {
            snprintf(why, whyLen, "%s missing closing bracket", address);
            return;
        }
        host = address + 1;
        hostLen = (size_t) (close - host);
        if (close[1] == ':') {
            number = close + 2;
        } else if (close[1]) {
            snprintf(why, whyLen, "%s has junk after closing bracket", address);
            return;
        }
    } else if (strchr(address, ':') != strrchr(address, ':')) {
        snprintf(why, whyLen, "IPv6 addresses must be bracketed, like [::1]:3389, not %s", address);
        return;
    } else {
        const char *colon = strchr(address, ':');
        if (colon) {
            hostLen = (size_t) (colon - address);
            number = colon + 1;
        }
    }

    if (hostLen == 0) {
        snprintf(why, whyLen, "No host in address");
        return;
    }
    if (number) {
        char *end = NULL;
        const long p = strtol(number, &end, 10);
        if (end == number || *end || p <= 0 || p > 65535) {
            snprintf(why, whyLen, "%s does not end in a port number", address);
            return;
        }
        *port = (int) p;
    }
    memmove(address, host, hostLen);
    address[hostLen] = 0;
}

/* ---- the command queue -------------------------------------------------- */

static void post(Session *s, const Cmd *c) {
    pthread_mutex_lock(&s->lock);
    if (s->quit) {
        pthread_mutex_unlock(&s->lock);
        free(c->text);
        return;
    }
    /* Motion coalesces onto the tail: the protocol thread can be inside a
       decode, and by the time it drains, every position but the last is
       somewhere the pointer no longer is. A button change is a different event
       and is never merged away. */
    if (c->type == CMD_POINTER && s->head != s->tail) {
        const int last = (s->head + CMD_RING - 1) % CMD_RING;
        if (s->ring[last].type == CMD_POINTER && s->ring[last].c == c->c) {
            s->ring[last] = *c;
            pthread_mutex_unlock(&s->lock);
            SetEvent(s->wake);
            return;
        }
    }
    const int next = (s->head + 1) % CMD_RING;
    if (next == s->tail) {
        free(s->ring[s->tail].text);
        s->tail = (s->tail + 1) % CMD_RING;
    }
    s->ring[s->head] = *c;
    s->head = next;
    pthread_cond_broadcast(&s->cond);
    pthread_mutex_unlock(&s->lock);
    SetEvent(s->wake);
}

/* ---- callbacks out ------------------------------------------------------ */

static void fireClosed(Session *s, const char *detail) {
    pthread_mutex_lock(&s->lock);
    const int already = s->closedReported;
    s->closedReported = 1;
    pthread_mutex_unlock(&s->lock);
    if (already) {
        return;
    }
    /* The library's own words, which can quote the server's; the seam promises
       UTF-8. */
    char *utf8 = vento_utf8_from_bytes(detail);
    s->cb.closed(s->cb.user, utf8 ? utf8 : "");
    free(utf8);
}

/* The far end's text, which is UTF-16 on the wire and UTF-8 at the seam. */
static void fireClipboard(Session *s, const WCHAR *text, size_t units) {
    char *utf8 = ConvertWCharNToUtf8Alloc(text, units, NULL);
    if (utf8) {
        s->cb.clipboard(s->cb.user, utf8);
        free(utf8);
    }
}

/* ---- the framebuffer ---------------------------------------------------- */

static Session *sessionOf(rdpContext *context) {
    return ((ShimContext *) context)->session;
}

/* One allocation per desktop size, zeroed opaque so that a region read before
   anything has been decoded is black rather than transparent. */
static BOOL allocFramebuffer(Session *s, int w, int h) {
    if (w <= 0 || h <= 0) {
        return FALSE;
    }
    const size_t bytes = (size_t) w * (size_t) h * 4u;
    uint8_t *shadow = malloc(bytes);
    if (!shadow) {
        return FALSE;
    }
    memset(shadow, 0, bytes);
    for (size_t i = 3; i < bytes; i += 4) {
        shadow[i] = 0xFF;
    }
    pthread_rwlock_wrlock(&s->fbLock);
    free(s->fb);
    s->fb = shadow;
    s->fbW = w;
    s->fbH = h;
    pthread_rwlock_unlock(&s->fbLock);
    return TRUE;
}

static BOOL onBeginPaint(rdpContext *context) {
    rdpGdi *gdi = context->gdi;
    gdi->primary->hdc->hwnd->invalid->null = TRUE;
    gdi->primary->hdc->hwnd->ninvalid = 0;
    return TRUE;
}

/* The whole of the second framebuffer's cost, and the only place it is paid:
   once per damaged rectangle per paint, under the writer lock. The GDI's own
   buffer is BGRX and the seam's framebuffer is RGBX, so red and blue trade
   places here and the alpha the codecs leave undefined is filled in — one pass
   over the damage rather than the library's converting copy and then ours. */
static BOOL onEndPaint(rdpContext *context) {
    Session *s = sessionOf(context);
    rdpGdi *gdi = context->gdi;
    HGDI_WND hwnd = gdi->primary->hdc->hwnd;
    if (hwnd->invalid->null || hwnd->ninvalid <= 0) {
        return TRUE;
    }

    const int fw = s->fbW, fh = s->fbH;
    pthread_rwlock_wrlock(&s->fbLock);
    for (INT32 i = 0; i < hwnd->ninvalid; i++) {
        const HGDI_RGN r = &hwnd->cinvalid[i];
        int x = r->x, y = r->y, w = r->w, h = r->h;
        if (x < 0) { w += x; x = 0; }
        if (y < 0) { h += y; y = 0; }
        if (x + w > fw) { w = fw - x; }
        if (y + h > fh) { h = fh - y; }
        if (w <= 0 || h <= 0) {
            r->w = 0;
            continue;
        }
        r->x = x; r->y = y; r->w = w; r->h = h;
        for (int row = 0; row < h; row++) {
            const uint32_t *src = (const uint32_t *) (gdi->primary_buffer
                    + (size_t) (y + row) * gdi->stride) + x;
            uint32_t *dst = (uint32_t *) (s->fb
                    + (size_t) (y + row) * (size_t) fw * 4u) + x;
            for (int col = 0; col < w; col++) {
                const uint32_t p = src[col];
                dst[col] = 0xFF000000u | ((p & 0x00FF0000u) >> 16)
                        | (p & 0x0000FF00u) | ((p & 0x000000FFu) << 16);
            }
        }
    }
    pthread_rwlock_unlock(&s->fbLock);

    for (INT32 i = 0; i < hwnd->ninvalid; i++) {
        const HGDI_RGN r = &hwnd->cinvalid[i];
        if (r->w > 0) {
            s->cb.damaged(s->cb.user, r->x, r->y, r->w, r->h);
        }
    }
    s->cb.frame_end(s->cb.user);

    hwnd->invalid->null = TRUE;
    hwnd->ninvalid = 0;
    return TRUE;
}

/* Only to see what the picture arrived as; the drawing is the library's. */
static BOOL onSurfaceBits(rdpContext *context, const SURFACE_BITS_COMMAND *cmd) {
    Session *s = sessionOf(context);
    if (cmd) {
        s->lastCodec = cmd->bmp.codecID;
    }
    return s->gdiSurfaceBits ? s->gdiSurfaceBits(context, cmd) : TRUE;
}

/* Not optional: gdi_ResetGraphics asserts on this callback, so a client without
   one does not fail to resize, it aborts on the channel thread. */
static BOOL onDesktopResize(rdpContext *context) {
    Session *s = sessionOf(context);
    const UINT32 w = freerdp_settings_get_uint32(context->settings, FreeRDP_DesktopWidth);
    const UINT32 h = freerdp_settings_get_uint32(context->settings, FreeRDP_DesktopHeight);
    if (!allocFramebuffer(s, (int) w, (int) h) || !gdi_resize(context->gdi, w, h)) {
        return FALSE;
    }
    s->cb.desktop_size(s->cb.user, (int) w, (int) h);
    return TRUE;
}

/* ---- the monitors ------------------------------------------------------- */

/* Whether `n` screens, four ints each, are a division of a desktop `w × h`:
   their bounding box is it. A layout that is not is one from before the
   desktop last changed size, and says nothing about the desktop now. */
static bool covers(const int32_t *flat, int n, int w, int h) {
    if (n < 1) {
        return false;
    }
    int right = 0, bottom = 0;
    for (int i = 0; i < n; i++) {
        const int32_t *m = flat + 4 * i;
        if (m[0] < 0 || m[1] < 0) {
            return false;
        }
        right = m[0] + m[2] > right ? m[0] + m[2] : right;
        bottom = m[1] + m[3] > bottom ? m[1] + m[3] : bottom;
    }
    return right == w && bottom == h;
}

/* The monitors as the server states them, in its order, which is the only
   identity RDP gives a monitor. A MONITOR_DEF's corners are inclusive and
   relative to the primary monitor, which others may sit above or to the left
   of; the desktop's pixel 0,0 is the top left of them all, so that is where
   the seam's coordinates start. Two places say it: the Monitor Layout PDU of
   the connection sequence (update->RemoteMonitors), and on the graphics
   pipeline every reset, which is how such a server answers a layout. */
static void heardLayout(Session *s, const MONITOR_DEF *defs, UINT32 count, const char *where) {
    const int n = count > VENTO_MAX_MONITORS ? VENTO_MAX_MONITORS : (int) count;
    int32_t flat[4 * VENTO_MAX_MONITORS];
    INT32 left = 0, top = 0;
    for (int i = 0; i < n; i++) {
        left = i == 0 || defs[i].left < left ? defs[i].left : left;
        top = i == 0 || defs[i].top < top ? defs[i].top : top;
    }
    for (int i = 0; i < n; i++) {
        flat[4 * i + 0] = defs[i].left - left;
        flat[4 * i + 1] = defs[i].top - top;
        flat[4 * i + 2] = defs[i].right - defs[i].left + 1;
        flat[4 * i + 3] = defs[i].bottom - defs[i].top + 1;
        LOGI("%s: monitor %d of %d %dx%d+%d+%d%s", where, i + 1, n, flat[4 * i + 2], flat[4 * i + 3],
             flat[4 * i + 0], flat[4 * i + 1],
             (defs[i].flags & MONITOR_PRIMARY) ? ", primary" : "");
    }
    pthread_mutex_lock(&s->lock);
    memcpy(s->serverLayout, flat, sizeof(int32_t) * 4 * (size_t) n);
    s->serverCount = n;
    pthread_mutex_unlock(&s->lock);
}

static BOOL onRemoteMonitors(rdpContext *context, UINT32 count, const MONITOR_DEF *monitors) {
    heardLayout(sessionOf(context), monitors, count, "monitor layout");
    return TRUE;
}

static UINT onGfxResetGraphics(RdpgfxClientContext *context,
                               const RDPGFX_RESET_GRAPHICS_PDU *reset) {
    rdpGdi *gdi = (rdpGdi *) context->custom;
    Session *s = sessionOf(gdi->context);
    if (reset->monitorCount > 0 && reset->monitorDefArray) {
        heardLayout(s, reset->monitorDefArray, reset->monitorCount, "graphics reset");
    }
    return s->gdiResetGraphics ? s->gdiResetGraphics(context, reset) : CHANNEL_RC_OK;
}

/* `n` screens, x, y, width, height each, as a display-control layout, into
   `out`, and what that layout makes of the desktop into `asked`. The
   protocol's rules rather than ours: the first screen is the primary and sits
   at the origin, so the lot moves to put it there; a monitor is 200 to 8192
   pixels each way and its width is even. A screen made narrower or shorter by
   that moves those right of or below it along by as much, so that screens
   side by side stay side by side. Returns how many went in. */
static int buildLayout(const int32_t *rects, int n, int scale, DISPLAY_CONTROL_MONITOR_LAYOUT *out,
                       int32_t *asked) {
    n = n > VENTO_MAX_MONITORS ? VENTO_MAX_MONITORS : n;
    int32_t w[VENTO_MAX_MONITORS], h[VENTO_MAX_MONITORS];
    for (int i = 0; i < n; i++) {
        const int32_t *r = rects + 4 * i;
        w[i] = r[2] < 200 ? 200 : (r[2] > 8192 ? 8192 : r[2]);
        w[i] &= ~1;
        h[i] = r[3] < 200 ? 200 : (r[3] > 8192 ? 8192 : r[3]);
    }
    int32_t x[VENTO_MAX_MONITORS], y[VENTO_MAX_MONITORS];
    for (int i = 0; i < n; i++) {
        const int32_t *r = rects + 4 * i;
        x[i] = r[0];
        y[i] = r[1];
        for (int j = 0; j < n; j++) {
            const int32_t *o = rects + 4 * j;
            if (o[0] + o[2] <= r[0]) {
                x[i] -= o[2] - w[j];
            }
            if (o[1] + o[3] <= r[1]) {
                y[i] -= o[3] - h[j];
            }
        }
    }
    int32_t left = 0, top = 0;
    for (int i = 0; i < n; i++) {
        left = i == 0 || x[i] < left ? x[i] : left;
        top = i == 0 || y[i] < top ? y[i] : top;
    }
    for (int i = 0; i < n; i++) {
        DISPLAY_CONTROL_MONITOR_LAYOUT *m = &out[i];
        memset(m, 0, sizeof(*m));
        m->Flags = i == 0 ? DISPLAY_CONTROL_MONITOR_PRIMARY : 0;
        m->Left = x[i] - x[0];
        m->Top = y[i] - y[0];
        m->Width = (UINT32) w[i];
        m->Height = (UINT32) h[i];
        m->Orientation = ORIENTATION_LANDSCAPE;
        /* The connection's, not 100: a layout re-states the whole of it, so
           leaving these at the default would undo the interface size the
           session was opened with. */
        m->DesktopScaleFactor = (UINT32) scale;
        m->DeviceScaleFactor = (UINT32) scale;
        asked[4 * i + 0] = x[i] - left;
        asked[4 * i + 1] = y[i] - top;
        asked[4 * i + 2] = w[i];
        asked[4 * i + 3] = h[i];
    }
    return n;
}

/* A layout of `n` screens on the display-control channel, remembered as what
   was asked for. A refusal is silence: the server makes a new desktop, a
   different one, or nothing. */
static void sendLayout(Session *s, const int32_t *rects, int n) {
    if (!s->disp || !s->disp->SendMonitorLayout || n < 1) {
        return;
    }
    DISPLAY_CONTROL_MONITOR_LAYOUT layout[VENTO_MAX_MONITORS];
    int32_t asked[4 * VENTO_MAX_MONITORS];
    n = buildLayout(rects, n, s->scale, layout, asked);
    for (int i = 0; i < n; i++) {
        LOGI("asking for monitor %d of %d: %ux%u%+d%+d", i + 1, n, layout[i].Width, layout[i].Height,
             layout[i].Left, layout[i].Top);
    }
    pthread_mutex_lock(&s->lock);
    memcpy(s->askedLayout, asked, sizeof(int32_t) * 4 * (size_t) n);
    s->askedCount = n;
    pthread_mutex_unlock(&s->lock);
    s->disp->SendMonitorLayout(s->disp, (UINT32) n, layout);
}

/* ---- the remote cursor -------------------------------------------------- */

static void fireCursor(Session *s, const uint32_t *rgba, int w, int h, int hotX, int hotY) {
    s->cb.cursor(s->cb.user, rgba, w, h, hotX, hotY);
}

static BOOL onPointerNew(rdpContext *context, rdpPointer *pointer) {
    (void) context;
    (void) pointer;
    return TRUE;
}

static void onPointerFree(rdpContext *context, rdpPointer *pointer) {
    (void) context;
    (void) pointer;
}

/* Converted when the pointer becomes the current one rather than when it
   arrives: a server sends shapes it then never uses, and an unchanged shape
   costs nothing on the other side anyway — `CursorCache` keys on the hash. */
static BOOL onPointerSet(rdpContext *context, rdpPointer *pointer) {
    Session *s = sessionOf(context);
    const UINT32 w = pointer->width, h = pointer->height;
    if (w == 0 || h == 0 || w > 384 || h > 384) {
        return TRUE;
    }
    uint32_t *rgba = calloc((size_t) w * h, sizeof(uint32_t));
    if (!rgba) {
        return TRUE;
    }
    /* RGBA32 — R, G, B, A in memory — which is the seam's own order for a
       cursor as for the framebuffer; whatever the platform packs it into, it
       does on its side of the call. */
    if (freerdp_image_copy_from_pointer_data((BYTE *) rgba, PIXEL_FORMAT_RGBA32, 0, 0, 0, w, h,
                                             pointer->xorMaskData, pointer->lengthXorMask,
                                             pointer->andMaskData, pointer->lengthAndMask,
                                             pointer->xorBpp, &context->gdi->palette)) {
        fireCursor(s, rgba, (int) w, (int) h, (int) pointer->xPos, (int) pointer->yPos);
    }
    free(rgba);
    return TRUE;
}

/* An empty shape is the seam's "the server has hidden the pointer". The default
   arrow is the same answer: this end draws its own cursor, and a server's idea
   of an arrow is not more informative than ours. */
static BOOL onPointerSetNull(rdpContext *context) {
    fireCursor(sessionOf(context), NULL, 0, 0, 0, 0);
    return TRUE;
}

static BOOL onPointerSetDefault(rdpContext *context) {
    fireCursor(sessionOf(context), NULL, 0, 0, 0, 0);
    return TRUE;
}

/* The far end saying where it has put the cursor, which this app's cursor does
   not take instruction from: it is owned here and the viewport follows it.

   On `update->pointer` rather than on the graphics pointer below, which has a
   SetPosition of the same shape that the library never calls: the position
   update is dispatched in core/update.c and the graphics module only ever sees
   shapes. */
static BOOL onPointerPosition(rdpContext *context, const POINTER_POSITION_UPDATE *pos) {
    Session *s = sessionOf(context);
    if (s && !s->sawPointerPosition) {
        s->sawPointerPosition = 1;
        LOGI("server moved its pointer to %u,%u (the first of this session)",
             pos->xPos, pos->yPos);
    }
    return TRUE;
}

static BOOL registerPointer(rdpGraphics *graphics) {
    rdpPointer pointer = {0};
    pointer.size = sizeof(pointer);
    pointer.New = onPointerNew;
    pointer.Free = onPointerFree;
    pointer.Set = onPointerSet;
    pointer.SetNull = onPointerSetNull;
    pointer.SetDefault = onPointerSetDefault;
    graphics_register_pointer(graphics, &pointer);
    return TRUE;
}

/* ---- the clipboard ------------------------------------------------------ */

/* Text and nothing else, in both directions. CF_UNICODETEXT is UTF-16 with a
   terminator, which is what every Windows clipboard holds; the seam's is
   UTF-8, and a file list or a bitmap has nowhere to go on this side. */
static UINT sendFormatList(Session *s, int haveText) {
    CLIPRDR_FORMAT format = {0};
    CLIPRDR_FORMAT_LIST list = {0};
    format.formatId = CF_UNICODETEXT;
    list.common.msgType = CB_FORMAT_LIST;
    list.numFormats = haveText ? 1 : 0;
    list.formats = haveText ? &format : NULL;
    if (!s->cliprdr || !s->cliprdr->ClientFormatList) {
        return ERROR_INTERNAL_ERROR;
    }
    /* The empty list matters as much as the full one: a channel that never
       sends a format list is one neither direction works on. */
    return s->cliprdr->ClientFormatList(s->cliprdr, &list);
}

static UINT onCliprdrMonitorReady(CliprdrClientContext *cliprdr,
                                  const CLIPRDR_MONITOR_READY *ready) {
    (void) ready;
    Session *s = (Session *) cliprdr->custom;
    CLIPRDR_CAPABILITIES caps = {0};
    CLIPRDR_GENERAL_CAPABILITY_SET general = {0};
    general.capabilitySetType = CB_CAPSTYPE_GENERAL;
    general.capabilitySetLength = 12;
    general.version = CB_CAPS_VERSION_2;
    general.generalFlags = CB_USE_LONG_FORMAT_NAMES;
    caps.cCapabilitiesSets = 1;
    caps.capabilitySets = (CLIPRDR_CAPABILITY_SET *) &general;
    const UINT rc = cliprdr->ClientCapabilities(cliprdr, &caps);
    if (rc != CHANNEL_RC_OK) {
        return rc;
    }
    pthread_mutex_lock(&s->lock);
    const int have = s->clipOut != NULL;
    pthread_mutex_unlock(&s->lock);
    return sendFormatList(s, have);
}

static UINT onCliprdrServerCapabilities(CliprdrClientContext *cliprdr,
                                        const CLIPRDR_CAPABILITIES *caps) {
    (void) cliprdr;
    (void) caps;
    return CHANNEL_RC_OK;
}

/* The far end has something. Ask for it only if it is text — the request is
   what makes the bytes arrive, since RDP renders a clipboard on demand. */
static UINT onCliprdrServerFormatList(CliprdrClientContext *cliprdr,
                                      const CLIPRDR_FORMAT_LIST *list) {
    Session *s = (Session *) cliprdr->custom;
    CLIPRDR_FORMAT_LIST_RESPONSE response = {0};
    response.common.msgType = CB_FORMAT_LIST_RESPONSE;
    response.common.msgFlags = CB_RESPONSE_OK;
    const UINT rc = cliprdr->ClientFormatListResponse(cliprdr, &response);
    if (rc != CHANNEL_RC_OK) {
        return rc;
    }
    if (s->viewOnly) {
        return CHANNEL_RC_OK;
    }
    for (UINT32 i = 0; i < list->numFormats; i++) {
        if (list->formats[i].formatId == CF_UNICODETEXT) {
            CLIPRDR_FORMAT_DATA_REQUEST request = {0};
            request.common.msgType = CB_FORMAT_DATA_REQUEST;
            request.requestedFormatId = CF_UNICODETEXT;
            return cliprdr->ClientFormatDataRequest(cliprdr, &request);
        }
    }
    return CHANNEL_RC_OK;
}

static UINT onCliprdrServerFormatListResponse(CliprdrClientContext *cliprdr,
                                              const CLIPRDR_FORMAT_LIST_RESPONSE *response) {
    (void) cliprdr;
    (void) response;
    return CHANNEL_RC_OK;
}

/* Something over there is pasting what this phone copied. */
static UINT onCliprdrServerFormatDataRequest(CliprdrClientContext *cliprdr,
                                             const CLIPRDR_FORMAT_DATA_REQUEST *request) {
    Session *s = (Session *) cliprdr->custom;
    CLIPRDR_FORMAT_DATA_RESPONSE response = {0};
    response.common.msgType = CB_FORMAT_DATA_RESPONSE;

    pthread_mutex_lock(&s->lock);
    BYTE *data = NULL;
    size_t bytes = 0;
    if (request->requestedFormatId == CF_UNICODETEXT && s->clipOut && !s->viewOnly) {
        bytes = (s->clipOutLen + 1) * sizeof(WCHAR);
        data = malloc(bytes);
        if (data) {
            memcpy(data, s->clipOut, bytes);
        } else {
            bytes = 0;
        }
    }
    pthread_mutex_unlock(&s->lock);

    response.common.msgFlags = data ? CB_RESPONSE_OK : CB_RESPONSE_FAIL;
    response.common.dataLen = (UINT32) bytes;
    response.requestedFormatData = data;
    const UINT rc = cliprdr->ClientFormatDataResponse(cliprdr, &response);
    free(data);
    return rc;
}

/* And the answer to our own request: the far end's text. */
static UINT onCliprdrServerFormatDataResponse(CliprdrClientContext *cliprdr,
                                              const CLIPRDR_FORMAT_DATA_RESPONSE *response) {
    Session *s = (Session *) cliprdr->custom;
    if ((response->common.msgFlags & CB_RESPONSE_FAIL) || !response->requestedFormatData) {
        return CHANNEL_RC_OK;
    }
    size_t units = response->common.dataLen / sizeof(WCHAR);
    const WCHAR *text = (const WCHAR *) response->requestedFormatData;
    while (units > 0 && text[units - 1] == 0) {
        units--;
    }
    if (units > 0) {
        fireClipboard(s, text, units);
    }
    return CHANNEL_RC_OK;
}

static void wireCliprdr(Session *s, CliprdrClientContext *cliprdr) {
    s->cliprdr = cliprdr;
    cliprdr->custom = s;
    cliprdr->MonitorReady = onCliprdrMonitorReady;
    cliprdr->ServerCapabilities = onCliprdrServerCapabilities;
    cliprdr->ServerFormatList = onCliprdrServerFormatList;
    cliprdr->ServerFormatListResponse = onCliprdrServerFormatListResponse;
    cliprdr->ServerFormatDataRequest = onCliprdrServerFormatDataRequest;
    cliprdr->ServerFormatDataResponse = onCliprdrServerFormatDataResponse;
}

/* ---- the channels ------------------------------------------------------- */

/* The server's pick from the capability sets the pipeline advertised, which is
   the only place the answer is published: the library keeps it private and
   leaves the settings saying what was asked. 8.1 carries H.264 only with the
   AVC420 flag set, 10 and later unless the AVC-disabled one is, and 8.0 never.
   The GDI does not use this hook, so there is nothing of its to chain. */
static UINT onGfxCapsConfirm(RdpgfxClientContext *gfx, const RDPGFX_CAPS_CONFIRM_PDU *confirm) {
    const rdpGdi *gdi = (const rdpGdi *) gfx->custom;
    if (!gdi || !confirm || !confirm->capsSet) {
        return CHANNEL_RC_OK;
    }
    Session *s = sessionOf(gdi->context);
    const UINT32 version = confirm->capsSet->version;
    const UINT32 flags = confirm->capsSet->flags;
    int h264 = 0;
    if (version == RDPGFX_CAPVERSION_81) {
        h264 = (flags & RDPGFX_CAPS_FLAG_AVC420_ENABLED) != 0;
    } else if (version >= RDPGFX_CAPVERSION_10) {
        h264 = (flags & RDPGFX_CAPS_FLAG_AVC_DISABLED) == 0;
    }
    s->gfxH264 = h264;
    s->gfxVersion = version;
    s->gfxCaps = *confirm->capsSet;
    s->gfxRestarting = 0;
    LOGI("graphics pipeline: server confirmed caps 0x%08x, flags 0x%08x", version, flags);
    return CHANNEL_RC_OK;
}

/* Every surface command, H.264 or not, is decoded inside the GDI's handler,
   and that is where the library makes, resets and feeds a surface's H.264
   context: inside it, the contexts are this session's.

   A decoder that can no longer follow its stream — a hardware one that
   failed mid-stream and was replaced by software, or one the person changed
   the mode under — has none of the pictures the stream refers back to, so
   until the next key frame every command fails and nothing is drawn; and
   Windows sends one only when a stream starts, not for a Refresh Rect, nor
   for output suppressed and allowed again. The capabilities advertised again
   are what it answers with new surfaces and a new stream, whose contexts get
   new decoders. */
static UINT onGfxSurfaceCommand(RdpgfxClientContext *context, const RDPGFX_SURFACE_COMMAND *cmd) {
    rdpGdi *gdi = (rdpGdi *) context->custom;
    Session *s = sessionOf(gdi->context);
    vento_h264_enter(&s->h264);
    const UINT rc = s->gdiSurfaceCommand(context, cmd);
    vento_h264_leave();
    if (atomic_exchange(&s->h264.restart, false) && !s->gfxRestarting && s->gfxVersion) {
        s->gfxRestarting = 1;
        RDPGFX_CAPSET caps = s->gfxCaps;
        const RDPGFX_CAPS_ADVERTISE_PDU pdu = {.capsSetCount = 1, .capsSets = &caps};
        const UINT error = context->CapsAdvertise(context, &pdu);
        if (error != CHANNEL_RC_OK) {
            LOGW("the capabilities could not be sent again: 0x%08x", error);
        }
    }
    return rc;
}

/* A channel's interface arrives on an event rather than from a call, so the
   pointer has to be kept: this is the only place either of them is handed
   over. */
static void onChannelConnected(void *context, const ChannelConnectedEventArgs *e) {
    rdpContext *ctx = (rdpContext *) context;
    Session *s = sessionOf(ctx);
    if (strcmp(e->name, RDPGFX_DVC_CHANNEL_NAME) == 0) {
        /* Without this the pipeline negotiates, the channel opens and nothing
           is ever painted. */
        RdpgfxClientContext *gfx = (RdpgfxClientContext *) e->pInterface;
        gdi_graphics_pipeline_init(ctx->gdi, gfx);
        gfx->CapsConfirm = onGfxCapsConfirm;
        /* Chained, as SurfaceBits is: the GDI's handler is what resizes. */
        s->gdiResetGraphics = gfx->ResetGraphics;
        gfx->ResetGraphics = onGfxResetGraphics;
        s->gdiSurfaceCommand = gfx->SurfaceCommand;
        gfx->SurfaceCommand = onGfxSurfaceCommand;
        s->gfxOpen = 1;
    } else if (strcmp(e->name, CLIPRDR_SVC_CHANNEL_NAME) == 0) {
        wireCliprdr(s, (CliprdrClientContext *) e->pInterface);
    } else if (strcmp(e->name, DISP_DVC_CHANNEL_NAME) == 0) {
        s->disp = (DispClientContext *) e->pInterface;
        /* The one thing that says a desktop can be reshaped at all: the channel
           being open is the capability, and it is a fact about this session
           rather than about the backend. */
        s->canResize = 1;
    }
}

static void onChannelDisconnected(void *context, const ChannelDisconnectedEventArgs *e) {
    rdpContext *ctx = (rdpContext *) context;
    Session *s = sessionOf(ctx);
    if (strcmp(e->name, RDPGFX_DVC_CHANNEL_NAME) == 0) {
        gdi_graphics_pipeline_uninit(ctx->gdi, (RdpgfxClientContext *) e->pInterface);
        s->gfxOpen = 0;
        s->gfxVersion = 0;
        s->gfxH264 = 0;
        s->gfxRestarting = 0;
        atomic_store(&s->h264.hardware, -1);
    } else if (strcmp(e->name, CLIPRDR_SVC_CHANNEL_NAME) == 0) {
        s->cliprdr = NULL;
    } else if (strcmp(e->name, DISP_DVC_CHANNEL_NAME) == 0) {
        s->disp = NULL;
        s->canResize = 0;
    }
}

/* ---- what the connection asks a person ---------------------------------- */

/* Blocks the protocol thread until the session is answered, which is the
   design: on the other side of it is a dialog and a person. */
static BOOL onAuthenticate(freerdp *instance, char **username, char **password, char **domain,
                           rdp_auth_reason reason) {
    Session *s = sessionOf(instance->context);
    /* The library itself now asks only when something is missing: the TLS and
       RDP paths skip the question when a user name and a password are both
       set, and NLA asks only for an empty user name or no password at all. So
       arriving here with both is one of the other reasons, and the answer is
       the library's own — what the record already holds is the answer, and the
       prompt is for when there is nothing to give. Forwarding it to a person
       would turn a saved password into a dialog. */
    if (*username && **username && *password && **password) {
        return TRUE;
    }
    LOGI("credentials needed, reason %d", (int) reason);
    pthread_mutex_lock(&s->lock);
    s->credAnswered = 0;
    free(s->credUser);
    free(s->credDomain);
    free(s->credPass);
    s->credUser = s->credDomain = s->credPass = NULL;
    pthread_mutex_unlock(&s->lock);

    s->cb.credentials_needed(s->cb.user, true);

    pthread_mutex_lock(&s->lock);
    while (!s->credAnswered && !s->quit) {
        pthread_cond_wait(&s->cond, &s->lock);
    }
    const int ok = s->credPass != NULL;
    if (ok) {
        free(*username);
        free(*password);
        free(*domain);
        *username = strdup(s->credUser ? s->credUser : "");
        *password = strdup(s->credPass);
        *domain = strdup(s->credDomain ? s->credDomain : "");
    }
    pthread_mutex_unlock(&s->lock);
    return ok ? TRUE : FALSE;
}

/* The same wait, for the other question a handshake asks. Nothing vouches for
   an RDP server's certificate — a Windows host signs its own — so the identity
   is the fingerprint, and the answer comes from the app: its pin store, or a
   person.

   The library is told the certificate is managed outside it (see
   applySettings), which makes this the only question it asks and the only
   answer it takes. Left to manage it itself, it would first look the server
   up in a store of its own under the config path and connect without asking
   when what is on file matches — a PEM left there by anyone, ever, would
   outvote the app — and a yes it was given would be written into that store
   for next time. Here a yes is good for this connection alone (the library
   keeps it for a redirect or a reconnect of the same session) and nothing is
   written anywhere.

   The fingerprint is the one the library's own prompt would have shown, the
   SHA-256 of the server's certificate, so a pin made against that prompt
   still matches. */
static int onVerifyX509Certificate(freerdp *instance, const BYTE *data, size_t length,
                                   const char *hostname, UINT16 port, DWORD flags) {
    (void) hostname; (void) port; (void) flags;
    Session *s = sessionOf(instance->context);

    /* The chain as PEM, the server's own certificate first, which is the one
       read back here. Copied because the length is what bounds it. */
    char *pem = data ? malloc(length + 1) : NULL;
    if (!pem) {
        return 0;
    }
    memcpy(pem, data, length);
    pem[length] = '\0';
    rdpCertificate *cert = freerdp_certificate_new_from_pem(pem);
    free(pem);
    char *fingerprint = cert ? freerdp_certificate_get_fingerprint(cert) : NULL;
    freerdp_certificate_free(cert);
    if (!fingerprint) {
        return 0;
    }

    pthread_mutex_lock(&s->lock);
    s->trustAnswered = 0;
    s->trustAccepted = 0;
    pthread_mutex_unlock(&s->lock);

    s->cb.trust_needed(s->cb.user, fingerprint);
    free(fingerprint);

    pthread_mutex_lock(&s->lock);
    while (!s->trustAnswered && !s->quit) {
        pthread_cond_wait(&s->cond, &s->lock);
    }
    const int accepted = s->trustAccepted && !s->quit;
    pthread_mutex_unlock(&s->lock);
    /* 0 rather than -1 for a no: both refuse, and -1 has the library log the
       whole certificate as an error as well. */
    return accepted ? 1 : 0;
}

/* ---- keysyms to scancodes ----------------------------------------------- */

/*
 * The one place the protocol does not match the stack. Everything above
 * `CursorController` speaks X11 keysyms, because that is RFB's vocabulary;
 * RDP's keyboard is scancodes — set 1, with an E0 prefix for the keys the
 * original PC keyboard did not have.
 *
 * Three things follow, each a decision rather than a detail:
 *
 *   1. A scancode is a position, not a character: 0x1E is "the key left of S",
 *      which is `a` on a US layout and `q` on a French one. So this table is a
 *      US layout, the connection asks for a US layout, and the server's own
 *      layout is what turns a position back into a character.
 *   2. What the layout cannot place goes as a Unicode keyboard event, which is
 *      how a phone's IME gets `é` and `→` across without a layout in between.
 *      It carries no modifier state, so it is the fallback and not the rule.
 *   3. Shift is part of the answer: `!` is not a key, it is `1` with Shift. The
 *      caller synthesises the modifier and has to know not to synthesise one
 *      that the person is already holding.
 */

#define KEY_EXT 0xE000u
#define KEY_NONE 0xFFFFu
/* Pause, which is not a position: the key never had a scancode of its own, and
   the library has a call of its own for the sequence mstsc sends instead. */
#define KEY_PAUSE 0xFFFEu

static UINT16 namedScancode(UINT32 keysym) {
    switch (keysym) {
        case 0xffe1: return 0x2a;             /* Shift_L */
        case 0xffe2: return 0x36;             /* Shift_R */
        case 0xffe3: return 0x1d;             /* Control_L */
        case 0xffe4: return KEY_EXT | 0x1d;   /* Control_R */
        case 0xffe9: return 0x38;             /* Alt_L */
        case 0xffea: case 0xfe03: return KEY_EXT | 0x38; /* Alt_R, ISO_Level3_Shift: both AltGr */
        case 0xffeb: return KEY_EXT | 0x5b;   /* Super_L */
        case 0xffec: return KEY_EXT | 0x5c;   /* Super_R */
        case 0xff67: return KEY_EXT | 0x5d;   /* Menu */
        case 0xffe5: return 0x3a;             /* Caps_Lock */
        case 0xff7f: return 0x45;             /* Num_Lock */
        case 0xff14: return 0x46;             /* Scroll_Lock */
        case 0xff08: return 0x0e;             /* BackSpace */
        case 0xff09: return 0x0f;             /* Tab */
        case 0xff0d: return 0x1c;             /* Return */
        case 0xff1b: return 0x01;             /* Escape */
        case 0xffff: return KEY_EXT | 0x53;   /* Delete */
        case 0xff63: return KEY_EXT | 0x52;   /* Insert */
        case 0xff50: return KEY_EXT | 0x47;   /* Home */
        case 0xff57: return KEY_EXT | 0x4f;   /* End */
        case 0xff55: return KEY_EXT | 0x49;   /* Page_Up */
        case 0xff56: return KEY_EXT | 0x51;   /* Page_Down */
        case 0xff51: return KEY_EXT | 0x4b;   /* Left */
        case 0xff52: return KEY_EXT | 0x48;   /* Up */
        case 0xff53: return KEY_EXT | 0x4d;   /* Right */
        case 0xff54: return KEY_EXT | 0x50;   /* Down */
        case 0xff61: return KEY_EXT | 0x37;   /* Print */
        case 0xff13: return KEY_PAUSE;
        case 0xffc8: return 0x57;             /* F11, which is not after F10 */
        case 0xffc9: return 0x58;             /* F12 */

        /* The keypad. Each key appears twice, once under each Num Lock state,
           and both spellings are the same position — which is the whole of what
           RDP can say. Which of the two the far end produces is decided by the
           far end's own Num Lock and not by ours, so this is the one place
           where the phone owning the lock does not carry: with the lock off
           here and on there, KP_Home arrives as 7. */
        case 0xffb7: case 0xff95: return 0x47;   /* KP_7, KP_Home */
        case 0xffb8: case 0xff97: return 0x48;   /* KP_8, KP_Up */
        case 0xffb9: case 0xff9a: return 0x49;   /* KP_9, KP_Prior */
        case 0xffb4: case 0xff96: return 0x4b;   /* KP_4, KP_Left */
        case 0xffb5: case 0xff9d: return 0x4c;   /* KP_5, KP_Begin */
        case 0xffb6: case 0xff98: return 0x4d;   /* KP_6, KP_Right */
        case 0xffb1: case 0xff9c: return 0x4f;   /* KP_1, KP_End */
        case 0xffb2: case 0xff99: return 0x50;   /* KP_2, KP_Down */
        case 0xffb3: case 0xff9b: return 0x51;   /* KP_3, KP_Next */
        case 0xffb0: case 0xff9e: return 0x52;   /* KP_0, KP_Insert */
        case 0xffae: case 0xff9f: return 0x53;   /* KP_Decimal, KP_Delete */
        case 0xffaa: return 0x37;                /* KP_Multiply */
        case 0xffad: return 0x4a;                /* KP_Subtract */
        case 0xffab: return 0x4e;                /* KP_Add */
        case 0xffaf: return KEY_EXT | 0x35;      /* KP_Divide */
        case 0xff8d: return KEY_EXT | 0x1c;      /* KP_Enter */
        case 0xffac: return 0x7e;                /* KP_Separator, the Brazilian keypad comma */

        /* The unambiguous three of a Japanese board's conversion keys, which
           are the three the control layer sends. */
        case 0xff27: return 0x70;             /* Hiragana_Katakana */
        case 0xff23: return 0x79;             /* Henkan_Mode */
        case 0xff22: return 0x7b;             /* Muhenkan */
        default:
            /* F1–F10 are consecutive; the keypad took the codes after them. */
            if (keysym >= 0xffbe && keysym <= 0xffc7) {
                return (UINT16) (0x3b + (keysym - 0xffbe));
            }
            /* F13–F24, which no PC keyboard has and every layout still carries
               a position for. F13–F23 run from 0x64; F24 does not follow them,
               and a Windows far end resolves the position that would be F24 by
               counting to VK_OEM_PA3 — KBD4_T6F in winpr's own table, and
               KBD4_T76 is the one that is F24. */
            if (keysym >= 0xffca && keysym <= 0xffd4) {
                return (UINT16) (0x64 + (keysym - 0xffca));
            }
            if (keysym == 0xffd5) {
                return 0x76;   /* F24 */
            }
            return KEY_NONE;
    }
}

/* The printable half of a US layout, written as the rows of the keyboard
   rather than sorted, because that is what it is: a wrong entry is visible as
   a wrong neighbour. */
static const struct {
    UINT16 code;
    UINT16 plain, shifted;
} US_LAYOUT[] = {
        {0x29, '`', '~'}, {0x02, '1', '!'}, {0x03, '2', '@'}, {0x04, '3', '#'},
        {0x05, '4', '$'}, {0x06, '5', '%'}, {0x07, '6', '^'}, {0x08, '7', '&'},
        {0x09, '8', '*'}, {0x0a, '9', '('}, {0x0b, '0', ')'}, {0x0c, '-', '_'},
        {0x0d, '=', '+'},
        {0x10, 'q', 'Q'}, {0x11, 'w', 'W'}, {0x12, 'e', 'E'}, {0x13, 'r', 'R'},
        {0x14, 't', 'T'}, {0x15, 'y', 'Y'}, {0x16, 'u', 'U'}, {0x17, 'i', 'I'},
        {0x18, 'o', 'O'}, {0x19, 'p', 'P'}, {0x1a, '[', '{'}, {0x1b, ']', '}'},
        {0x2b, '\\', '|'},
        {0x1e, 'a', 'A'}, {0x1f, 's', 'S'}, {0x20, 'd', 'D'}, {0x21, 'f', 'F'},
        {0x22, 'g', 'G'}, {0x23, 'h', 'H'}, {0x24, 'j', 'J'}, {0x25, 'k', 'K'},
        {0x26, 'l', 'L'}, {0x27, ';', ':'}, {0x28, '\'', '"'},
        {0x2c, 'z', 'Z'}, {0x2d, 'x', 'X'}, {0x2e, 'c', 'C'}, {0x2f, 'v', 'V'},
        {0x30, 'b', 'B'}, {0x31, 'n', 'N'}, {0x32, 'm', 'M'}, {0x33, ',', '<'},
        {0x34, '.', '>'}, {0x35, '/', '?'}, {0x39, ' ', ' '},
};

/* The inverse of `Keysym.fromUnicode`: Latin-1 is itself, everything else is
   0x01000000 | codepoint. */
static UINT32 keysymToUnicode(UINT32 keysym) {
    if ((keysym & 0xff000000u) == 0x01000000u) {
        return keysym & 0x00ffffffu;
    }
    if ((keysym >= 0x20 && keysym <= 0x7e) || (keysym >= 0xa0 && keysym <= 0xff)) {
        return keysym;
    }
    return 0;
}

/* Fills in one of the three answers: a position, a position with Shift, or a
   character with no position at all. */
static void mapKeysym(UINT32 keysym, UINT16 *code, int *shift, WCHAR *unicode) {
    *code = KEY_NONE;
    *shift = 0;
    *unicode = 0;
    const UINT16 named = namedScancode(keysym);
    if (named != KEY_NONE) {
        *code = named;
        return;
    }
    /* The one keypad key a PC has no position for — FreeRDP's own xkb table has
       KP_Equal as unknown — and it names a character, so it goes as one. */
    if (keysym == 0xffbd) {
        *unicode = '=';
        return;
    }
    const UINT32 cp = keysymToUnicode(keysym);
    if (cp == 0 || cp > 0xFFFF) {
        return;
    }
    for (size_t i = 0; i < sizeof(US_LAYOUT) / sizeof(US_LAYOUT[0]); i++) {
        if (US_LAYOUT[i].plain == cp) {
            *code = US_LAYOUT[i].code;
            return;
        }
        if (US_LAYOUT[i].shifted == cp) {
            *code = US_LAYOUT[i].code;
            *shift = 1;
            return;
        }
    }
    *unicode = (WCHAR) cp;
}

/* ---- input, on the protocol thread -------------------------------------- */

static void sendScancode(Session *s, UINT16 code, int down) {
    rdpInput *input = s->instance->context->input;
    if (code == KEY_PAUSE) {
        /* The whole sequence goes on the press, so the release sends nothing.
           What the library puts on the wire is what mstsc does — a Ctrl marked
           E1 around a Num Lock — rather than the E1 1D 45 a keyboard sends. */
        if (down) {
            freerdp_input_send_keyboard_pause_event(input);
        }
        return;
    }
    UINT16 flags = down ? KBD_FLAGS_DOWN : KBD_FLAGS_RELEASE;
    if (code & KEY_EXT) {
        flags |= KBD_FLAGS_EXTENDED;
    }
    freerdp_input_send_keyboard_event(input, flags, (UINT8) (code & 0xFFu));
}

static void doKeyDown(Session *s, UINT32 keysym, INT64 keyId) {
    if (s->viewOnly || !s->connected) {
        return;
    }
    UINT16 code;
    int shift;
    WCHAR unicode;
    mapKeysym(keysym, &code, &shift, &unicode);
    if (code == KEY_NONE && unicode == 0) {
        return;
    }

    /* A second press of a key already down is auto-repeat, which RDP carries as
       another down and nothing else — and re-recording it would fill the table
       and then silently drop every later key, which is the fault stage 23f
       found in two other backends. */
    for (int i = 0; i < s->nkeys; i++) {
        if (s->keys[i].id == keyId) {
            if (s->keys[i].unicode) {
                freerdp_input_send_unicode_keyboard_event(s->instance->context->input,
                                                          0, s->keys[i].unicode);
            } else {
                sendScancode(s, s->keys[i].code, 1);
            }
            return;
        }
    }
    if (s->nkeys >= MAX_HELD_KEYS) {
        return;
    }

    if (unicode) {
        freerdp_input_send_unicode_keyboard_event(s->instance->context->input, 0, unicode);
    } else {
        /* Synthesised only when the person is not already holding one: a
           capital typed while Shift is down would otherwise be released by us
           on the way up, and the far end would see Shift let go mid-word. */
        if (shift && !s->shiftHeld) {
            sendScancode(s, 0x2a, 1);
        }
        sendScancode(s, code, 1);
        if (keysym == 0xffe1 || keysym == 0xffe2) {
            s->shiftHeld = 1;
        }
    }
    s->keys[s->nkeys].id = keyId;
    s->keys[s->nkeys].code = code;
    s->keys[s->nkeys].shift = shift && !s->shiftHeld;
    s->keys[s->nkeys].unicode = unicode;
    s->nkeys++;
}

static void releaseAt(Session *s, int index) {
    if (!s->keys[index].unicode) {
        sendScancode(s, s->keys[index].code, 0);
        if (s->keys[index].shift) {
            sendScancode(s, 0x2a, 0);
        }
        if (s->keys[index].code == 0x2a || s->keys[index].code == 0x36) {
            s->shiftHeld = 0;
        }
    }
    s->keys[index] = s->keys[--s->nkeys];
}

static void doKeyUp(Session *s, INT64 keyId) {
    for (int i = 0; i < s->nkeys; i++) {
        if (s->keys[i].id == keyId) {
            if (!s->viewOnly && s->connected) {
                releaseAt(s, i);
            } else {
                s->keys[i] = s->keys[--s->nkeys];
            }
            return;
        }
    }
}

static void doReleaseAll(Session *s) {
    while (s->nkeys > 0) {
        if (!s->viewOnly && s->connected) {
            releaseAt(s, s->nkeys - 1);
        } else {
            s->nkeys--;
        }
    }
    s->shiftHeld = 0;
}

/* An absolute position and an RFB button mask, which is what the whole control
   stack speaks. Three of those bits are buttons and four are wheel notches, and
   RDP has nothing pseudo about its wheel — so the buttons are held state and
   the wheel is an edge. */
static void doPointer(Session *s, int x, int y, UINT16 mask) {
    if (s->viewOnly || !s->connected) {
        return;
    }
    rdpInput *input = s->instance->context->input;
    const UINT16 previous = s->buttons;
    s->buttons = mask;

    /* The move goes first, so that a click lands where the finger is. */
    freerdp_input_send_mouse_event(input, PTR_FLAGS_MOVE, (UINT16) x, (UINT16) y);

    static const struct { UINT16 bit, flag; } BUTTONS[] = {
            {0x01, PTR_FLAGS_BUTTON1},
            {0x02, PTR_FLAGS_BUTTON3}, /* middle: RDP numbers them differently */
            {0x04, PTR_FLAGS_BUTTON2},
    };
    for (size_t i = 0; i < sizeof(BUTTONS) / sizeof(BUTTONS[0]); i++) {
        const int now = (mask & BUTTONS[i].bit) != 0;
        const int was = (previous & BUTTONS[i].bit) != 0;
        if (now != was) {
            freerdp_input_send_mouse_event(input,
                                           (UINT16) (BUTTONS[i].flag | (now ? PTR_FLAGS_DOWN : 0)),
                                           (UINT16) x, (UINT16) y);
        }
    }
    /* Buttons 8 and 9, which RDP calls X1 and X2 and Windows means browser back
       and forward by. */
    static const struct { UINT16 bit, flag; } EXTENDED[] = {
            {0x80, PTR_XFLAGS_BUTTON1},
            {0x100, PTR_XFLAGS_BUTTON2},
    };
    for (size_t i = 0; i < sizeof(EXTENDED) / sizeof(EXTENDED[0]); i++) {
        const int now = (mask & EXTENDED[i].bit) != 0;
        const int was = (previous & EXTENDED[i].bit) != 0;
        if (now != was) {
            freerdp_input_send_extended_mouse_event(input,
                                                    (UINT16) (EXTENDED[i].flag
                                                              | (now ? PTR_XFLAGS_DOWN : 0)),
                                                    (UINT16) x, (UINT16) y);
        }
    }
    /* RDP counts wheel rotation in 120ths of a click, as Windows does
       everywhere; RFB counts clicks, as button presses. */
    static const struct { UINT16 bit, flags; } WHEEL[] = {
            {0x08, PTR_FLAGS_WHEEL | 120},
            {0x10, PTR_FLAGS_WHEEL | PTR_FLAGS_WHEEL_NEGATIVE | 0x88}, /* -120 in nine bits */
            {0x20, PTR_FLAGS_HWHEEL | PTR_FLAGS_WHEEL_NEGATIVE | 0x88},
            {0x40, PTR_FLAGS_HWHEEL | 120},
    };
    for (size_t i = 0; i < sizeof(WHEEL) / sizeof(WHEEL[0]); i++) {
        if ((mask & WHEEL[i].bit) && !(previous & WHEEL[i].bit)) {
            freerdp_input_send_mouse_event(input, WHEEL[i].flags, (UINT16) x, (UINT16) y);
        }
    }
}

static void doClipboard(Session *s, WCHAR *text, size_t len) {
    pthread_mutex_lock(&s->lock);
    free(s->clipOut);
    s->clipOut = text;
    s->clipOutLen = len;
    pthread_mutex_unlock(&s->lock);
    if (s->cliprdr && !s->viewOnly) {
        sendFormatList(s, text != NULL);
    }
}

/* The reverse of RFB's pause: an RDP server sends what it likes, so this is a
   Suppress Output PDU telling it not to, and the redraw on the way back is the
   server's own doing when output is allowed again. */
static void doFocus(Session *s, int focused) {
    if (!s->connected || !s->instance->context->gdi) {
        return;
    }
    gdi_send_suppress_output(s->instance->context->gdi, focused ? FALSE : TRUE);
}

/* A desktop `width × height`, which is the whole of it and not one monitor:
   the seam's size is the framebuffer's everywhere else. So the monitors the
   session asks for divide the width between them, side by side, the last
   taking what the division leaves over; asking each for the whole width made
   a desktop of two monitors twice as wide as the one asked for. (IronRDP's
   backend divides it the same way.) */
static void doResize(Session *s, int width, int height) {
    const int count = s->monitors < 1 ? 1
                      : (s->monitors > VENTO_MAX_MONITORS ? VENTO_MAX_MONITORS : s->monitors);
    const int each = width / count;
    int32_t rects[4 * VENTO_MAX_MONITORS];
    for (int i = 0; i < count; i++) {
        rects[4 * i + 0] = i * each;
        rects[4 * i + 1] = 0;
        rects[4 * i + 2] = i == count - 1 ? width - each * (count - 1) : each;
        rects[4 * i + 3] = height;
    }
    sendLayout(s, rects, count);
}

/* The layout op_request_layout left, the latest one: requests made faster
   than the thread drains are the same question asked again. */
static void doLayout(Session *s) {
    int32_t rects[4 * VENTO_MAX_MONITORS];
    pthread_mutex_lock(&s->lock);
    const int n = s->wantCount;
    memcpy(rects, s->wantLayout, sizeof(int32_t) * 4 * (size_t) n);
    s->wantCount = 0;
    pthread_mutex_unlock(&s->lock);
    sendLayout(s, rects, n);
}

static void drainCommands(Session *s) {
    for (;;) {
        Cmd c;
        pthread_mutex_lock(&s->lock);
        if (s->head == s->tail) {
            pthread_mutex_unlock(&s->lock);
            return;
        }
        c = s->ring[s->tail];
        s->tail = (s->tail + 1) % CMD_RING;
        pthread_mutex_unlock(&s->lock);

        switch (c.type) {
            case CMD_POINTER: doPointer(s, c.a, c.b, (UINT16) c.c); break;
            case CMD_KEY_DOWN: doKeyDown(s, (UINT32) c.a, c.c); break;
            case CMD_KEY_UP: doKeyUp(s, c.c); break;
            case CMD_RELEASE_KEYS: doReleaseAll(s); break;
            case CMD_CLIPBOARD: doClipboard(s, c.text, c.textLen); c.text = NULL; break;
            case CMD_FOCUS: doFocus(s, c.a); break;
            case CMD_RESIZE: doResize(s, c.a, c.b); break;
            case CMD_LAYOUT: doLayout(s); break;
            case CMD_DISCONNECT:
                doReleaseAll(s);
                freerdp_abort_connect_context(s->instance->context);
                break;
            default: break;
        }
        free(c.text);
    }
}

/* ---- the connection ----------------------------------------------------- */

static BOOL onPreConnect(freerdp *instance) {
    rdpContext *ctx = instance->context;
    PubSub_SubscribeChannelConnected(ctx->pubSub, onChannelConnected);
    PubSub_SubscribeChannelDisconnected(ctx->pubSub, onChannelDisconnected);
    /* Before the connection, since the connection sequence is where the
       Monitor Layout PDU comes. */
    ctx->update->RemoteMonitors = onRemoteMonitors;
    return TRUE;
}

/* Idempotent on purpose: upstream has two call sites for this callback — the
   activation sequence and the end of freerdp_connect — and which of them runs is
   the server's business rather than ours. A second one that got through would
   re-initialise the GDI under a framebuffer the first had already published. */
static BOOL onPostConnect(freerdp *instance) {
    Session *s = sessionOf(instance->context);
    if (s->connected) {
        return TRUE;
    }
    rdpSettings *settings = instance->context->settings;
    const UINT32 w = freerdp_settings_get_uint32(settings, FreeRDP_DesktopWidth);
    const UINT32 h = freerdp_settings_get_uint32(settings, FreeRDP_DesktopHeight);

    /* BGRX because that is what every codec here decodes to, so the library's
       copy into this buffer is a memcpy per row. The seam wants RGBX and the
       difference is two bytes swapped — which onEndPaint does in a pass it
       makes anyway. Asking the GDI for RGBX instead moves that swap into
       FreeRDP's generic converter, which reads and writes a pixel at a time and
       cost a fifth of the decode thread when it was measured. */
    if (!gdi_init(instance, PIXEL_FORMAT_BGRX32)) {
        return FALSE;
    }
    if (!registerPointer(instance->context->graphics) || !allocFramebuffer(s, (int) w, (int) h)) {
        return FALSE;
    }
    rdpUpdate *update = instance->context->update;
    update->BeginPaint = onBeginPaint;
    update->EndPaint = onEndPaint;
    update->DesktopResize = onDesktopResize;
    update->pointer->PointerPosition = onPointerPosition;
    /* Chained rather than replaced: the GDI's own handler is what draws. */
    s->gdiSurfaceBits = update->SurfaceBits;
    update->SurfaceBits = onSurfaceBits;

    /* What the server will take from us. Both of these are the client's wish
       ANDed with the server's offer during capability exchange, so after
       PostConnect they are facts about this session rather than about us. The
       relative one is not acted on yet; whether a server offers it at all is
       the question a relative pointer over RDP turns on. */
    LOGI("server input: relative pointer %s, horizontal wheel %s, unicode %s",
         freerdp_settings_get_bool(settings, FreeRDP_HasRelativeMouseEvent) ? "yes" : "no",
         freerdp_settings_get_bool(settings, FreeRDP_HasHorizontalWheel) ? "yes" : "no",
         freerdp_settings_get_bool(settings, FreeRDP_UnicodeInput) ? "yes" : "no");

    s->connected = 1;
    s->cb.connected(s->cb.user, (int) w, (int) h);
    return TRUE;
}

static void onPostDisconnect(freerdp *instance) {
    gdi_free(instance);
}

static void applySettings(Session *s, rdpSettings *settings) {
    freerdp_settings_set_string(settings, FreeRDP_ServerHostname, s->host);
    freerdp_settings_set_uint32(settings, FreeRDP_ServerPort, (UINT32) s->port);
    freerdp_settings_set_string(settings, FreeRDP_Username, s->user ? s->user : "");
    freerdp_settings_set_string(settings, FreeRDP_Domain, s->domain ? s->domain : "");
    if (s->password) {
        freerdp_settings_set_string(settings, FreeRDP_Password, s->password);
    }
    freerdp_settings_set_string(settings, FreeRDP_ClientHostname, s->clientName);
    /* The major type says what this end runs on, as FreeRDP's X11 and Wayland
       clients and the other RDP backend send it. The minor type stays
       unspecified, which is IronRDP's too: FreeRDP's own clients name their
       display stack there (an X server, Wayland, SDL), and this library draws
       into a buffer for whichever window system the client has, while the
       phone has no minor type at all. */
    freerdp_settings_set_uint32(settings, FreeRDP_OsMajorType, s->osMajorType);
    freerdp_settings_set_uint32(settings, FreeRDP_OsMinorType, OSMINORTYPE_UNSPECIFIED);
    freerdp_settings_set_uint32(settings, FreeRDP_KeyboardLayout, (UINT32) s->keyboardLayout);
    freerdp_settings_set_uint32(settings, FreeRDP_DesktopWidth, (UINT32) s->width);
    freerdp_settings_set_uint32(settings, FreeRDP_DesktopHeight, (UINT32) s->height);
    freerdp_settings_set_uint32(settings, FreeRDP_ColorDepth, 32);
    freerdp_settings_set_bool(settings, FreeRDP_SoftwareGdi, TRUE);
    freerdp_settings_set_uint32(settings, FreeRDP_TcpConnectTimeout,
                                (UINT32) (s->connectTimeoutMs > 0 ? s->connectTimeoutMs : 20000));

    /* Where the library would keep its own certificate store. Nothing is kept
       there — the certificate is managed outside the library, below — but the
       TLS layer still sets a store up at connect, and without a path for one
       the connection fails before it starts. */
    if (s->configPath) {
        freerdp_settings_set_string(settings, FreeRDP_ConfigPath, s->configPath);
    }
    /* Off already, since nothing in the library turns it on, and stated
       because on it makes freerdp_client_start put a command-line prompt back
       over every callback — and there is no terminal on a phone for them to
       prompt at. */
    freerdp_settings_set_bool(settings, FreeRDP_UseCommonStdioCallbacks, FALSE);
    freerdp_settings_set_bool(settings, FreeRDP_IgnoreCertificate, FALSE);
    freerdp_settings_set_bool(settings, FreeRDP_AutoAcceptCertificate, FALSE);
    /* Every certificate goes to onVerifyX509Certificate and to nothing else:
       not the known-hosts store under the config path, not its JSON
       certificate list, not a certificate authority. The app's answer is the
       only one, and the library writes none of its own. */
    freerdp_settings_set_bool(settings, FreeRDP_ExternalCertificateManagement, TRUE);

    /* Network Level Authentication: prefer it, insist on it, or refuse it.
       Nothing here turns off TLS — a server that offers only the RDP security
       layer is one whose password crosses in something a 1990s cipher calls
       encryption, and the connection is worth failing.

       NLA is two switches since the library turned ExtSecurity on by default
       in 3.32.0. That is HYBRID_EX: the same CredSSP, plus the Early User
       Authorization Result PDU, which turns "this account may not log in here"
       into an error rather than a login screen that says it. It is negotiated
       on its own switch and ahead of plain NLA, so "Never" has to refuse both
       or CredSSP happens anyway; "If supported" gets HYBRID_EX first, as every
       client of this library now does. */
    const int nlaOff = s->nla && strcmp(s->nla, "off") == 0;
    const int nlaRequired = s->nla && strcmp(s->nla, "require") == 0;
    freerdp_settings_set_bool(settings, FreeRDP_NlaSecurity, !nlaOff);
    freerdp_settings_set_bool(settings, FreeRDP_ExtSecurity, !nlaOff);
    freerdp_settings_set_bool(settings, FreeRDP_TlsSecurity, !nlaRequired);
    freerdp_settings_set_bool(settings, FreeRDP_RdpSecurity, FALSE);
    freerdp_settings_set_bool(settings, FreeRDP_UseRdpSecurityLayer, FALSE);

    /* The floor under TLS. 3.27.0 raised it to TLS 1.2 at OpenSSL's security
       level 2, from the TLS 1.0 at level 1 that 3.19.1 set — but level 1
       already refuses TLS 1.0 and 1.1 in OpenSSL 3, and the SHA-1 signatures
       that come with them, so what the raise took from this app is keys under
       2048 bits, and what this row gives back is more than it took. Level 0,
       because it is the only one at which the lower version is a version a
       handshake can use. Unset, both are the library's. */
    if (s->legacyTls) {
        freerdp_settings_set_uint16(settings, FreeRDP_TLSMinVersion, TLS1_VERSION);
        freerdp_settings_set_uint32(settings, FreeRDP_TlsSecLevel, FREERDP_TLS_SECLEVEL_0);
    }

    freerdp_settings_set_bool(settings, FreeRDP_CompressionEnabled, s->compression != 0);
    freerdp_settings_set_bool(settings, FreeRDP_RedirectClipboard, TRUE);
    freerdp_settings_set_bool(settings, FreeRDP_SupportDisplayControl, TRUE);

    /* The picture. The graphics pipeline is what a machine since Windows 8
       would rather send a desktop over, and the only path that carries H.264 —
       which this build decodes with the platform's decoders (h264_vento.h),
       on the phone's MediaCodec or the Linux client's FFmpeg. RemoteFX is the
       older codec, and bitmaps are what is left when a server has neither. */
    const char *g = s->graphics ? s->graphics : "gfx";
    const int gfx = strcmp(g, "gfx") == 0 || strcmp(g, "gfx-novideo") == 0;
    /* No decoders, which is the probe, and nothing is offered that would need
       one. */
    const int h264 = strcmp(g, "gfx") == 0 && s->h264.available;
    const int rfx = gfx || strcmp(g, "rfx") == 0;
    freerdp_settings_set_bool(settings, FreeRDP_SupportGraphicsPipeline, gfx);
    freerdp_settings_set_bool(settings, FreeRDP_RemoteFxCodec, rfx);
    /* One switch and not two: with H264 on and AVC444 off, the client advertises
       capability version 8.1 and stops — every version from 10 up is inside a
       branch these two select together — so half of "H.264 but only AVC420" is
       also "no progressive v2, no scaling, no error-reporting caps". The choice
       worth offering is video or no video, and it is this pair. */
    freerdp_settings_set_bool(settings, FreeRDP_GfxH264, h264);
    freerdp_settings_set_bool(settings, FreeRDP_GfxAVC444, h264);
    freerdp_settings_set_bool(settings, FreeRDP_GfxProgressive, gfx);
    freerdp_settings_set_bool(settings, FreeRDP_GfxPlanar, gfx);
    freerdp_settings_set_bool(settings, FreeRDP_GfxSmallCache, FALSE);
    freerdp_settings_set_bool(settings, FreeRDP_GfxThinClient, FALSE);
    /* Off, which is the library's own default and is stated here because the
       obvious change is to turn it on: the legacy path is the only one that
       uses cache orders, and on the damage workload the cache misses every
       time and costs 0.97 MiB against 0.77. */
    freerdp_settings_set_bool(settings, FreeRDP_BitmapCacheEnabled, FALSE);

    /* Sound: the two settings decide what the client info tells the server to
       do — play it here, play it over there, or play nothing — and the library
       loads the channel that carries it from the first of them, as a static
       and a dynamic channel both, which is its own design. On Android the
       channel tries OpenSL ES and then a device that accepts every format and
       discards every sample.

       That silent device is also what "off" and "over there" get: autodetect,
       which is on below, makes the library register device redirection, and
       device redirection without rdpsnd asked for adds rdpsnd against the
       silent device anyway — a channel the client info has already told the
       server not to play anything on. */
    const int soundHere = s->sound && strcmp(s->sound, "local") == 0;
    const int soundThere = s->sound && strcmp(s->sound, "remote") == 0;
    freerdp_settings_set_bool(settings, FreeRDP_AudioPlayback, soundHere);
    freerdp_settings_set_bool(settings, FreeRDP_RemoteConsoleAudio, soundThere);

    /* How big the far end draws its own interface. The protocol allows 100–500
       for the desktop factor and exactly 100, 140 or 180 for the device one,
       and a value outside either range makes a server ignore both — which is
       why one row sets them to the same three values rather than two rows.
       These two are the copy in the client core data, which Windows reads and
       does nothing with; the copy that works is in the monitor block below. */
    freerdp_settings_set_uint32(settings, FreeRDP_DesktopScaleFactor, (UINT32) s->scale);
    freerdp_settings_set_uint32(settings, FreeRDP_DeviceScaleFactor, (UINT32) s->scale);

    /* What the remote machine may spend the link on drawing. The flags are a
       hint it is free to ignore, and it decides some of the rest for itself
       from the connection type. */
    UINT32 performance = PERF_FLAG_NONE;
    UINT32 connectionType = CONNECTION_TYPE_AUTODETECT;
    if (s->experience && strcmp(s->experience, "plain") == 0) {
        performance = PERF_DISABLE_WALLPAPER | PERF_DISABLE_FULLWINDOWDRAG
                | PERF_DISABLE_MENUANIMATIONS | PERF_DISABLE_THEMING
                | PERF_DISABLE_CURSOR_SHADOW | PERF_DISABLE_CURSORSETTINGS;
        connectionType = CONNECTION_TYPE_MODEM;
    } else if (!s->experience || strcmp(s->experience, "balanced") == 0) {
        performance = PERF_DISABLE_MENUANIMATIONS | PERF_DISABLE_FULLWINDOWDRAG
                | PERF_DISABLE_CURSOR_SHADOW;
        /* LAN, on a row that only asks about effects, because a server reads
           this as what it may spend: xrdp offers RemoteFX to a client that says
           LAN and plain bitmaps to one that says broadband, so anything less
           here silently costs the codec. The other RDP client says the same
           thing on the same row, which is what makes the two comparable. */
        connectionType = CONNECTION_TYPE_LAN;
    } else {
        performance = PERF_ENABLE_FONT_SMOOTHING | PERF_ENABLE_DESKTOP_COMPOSITION;
        connectionType = CONNECTION_TYPE_LAN;
    }
    freerdp_settings_set_uint32(settings, FreeRDP_PerformanceFlags, performance);
    freerdp_settings_set_uint32(settings, FreeRDP_ConnectionType, connectionType);
    /* On whatever the connection type says, because a Windows host sends RTT
       measure requests of its own accord and a client that has not enabled
       autodetect treats one as a protocol error and drops the connection —
       which is a session that ends half a second after it starts. What it buys
       as well is the only line speed this protocol ever states. */
    freerdp_settings_set_bool(settings, FreeRDP_NetworkAutoDetect, TRUE);

    /* The monitor layout, which a scale other than 100 needs as much as a second
       screen does. Two library conditions gate the block that carries each
       monitor's scale factors: the layout is only written under UseMultimon,
       and the attributes inside it only under HasMonitorAttributes, which
       nothing on the client path ever sets — so a client of this library sends
       its scale in the core data alone, and Windows acts on neither number
       until the monitor block arrives with them. Both are therefore set for one
       monitor as well, which costs a layout block naming a single screen. */
    if (s->monitors > 1 || s->scale != 100) {
        freerdp_settings_set_bool(settings, FreeRDP_UseMultimon, TRUE);
        freerdp_settings_set_bool(settings, FreeRDP_HasMonitorAttributes, TRUE);
        freerdp_settings_set_uint32(settings, FreeRDP_MonitorCount, (UINT32) s->monitors);
        if (s->monitors > 1) {
            freerdp_settings_set_uint32(settings, FreeRDP_DesktopWidth,
                                        (UINT32) (s->width * s->monitors));
        }
        if (freerdp_settings_set_pointer_len(settings, FreeRDP_MonitorDefArray, NULL,
                                             (size_t) s->monitors)) {
            for (int i = 0; i < s->monitors; i++) {
                rdpMonitor *m = freerdp_settings_get_pointer_array_writable(
                        settings, FreeRDP_MonitorDefArray, (size_t) i);
                if (!m) {
                    continue;
                }
                m->x = i * s->width;
                m->y = 0;
                m->width = s->width;
                m->height = s->height;
                m->is_primary = i == 0;
                m->orig_screen = i;
                m->attributes.physicalWidth = 0;
                m->attributes.physicalHeight = 0;
                m->attributes.orientation = 0;
                m->attributes.desktopScaleFactor = (UINT32) s->scale;
                m->attributes.deviceScaleFactor = (UINT32) s->scale;
            }
        }
        /* What the connection asks for is the first layout asked for, which
           is the answer until the server says otherwise. */
        pthread_mutex_lock(&s->lock);
        s->askedCount = s->monitors > VENTO_MAX_MONITORS ? VENTO_MAX_MONITORS : s->monitors;
        for (int i = 0; i < s->askedCount; i++) {
            s->askedLayout[4 * i + 0] = i * s->width;
            s->askedLayout[4 * i + 1] = 0;
            s->askedLayout[4 * i + 2] = s->width;
            s->askedLayout[4 * i + 3] = s->height;
        }
        pthread_mutex_unlock(&s->lock);
    }
}

static void *protocolThread(void *arg) {
    Session *s = (Session *) arg;
    ShimContext *shim = s->ctx;
    rdpContext *ctx = &shim->client.context;

    if (s->addressError[0]) {
        fireClosed(s, s->addressError);
        return NULL;
    }

    ctx->instance->PreConnect = onPreConnect;
    ctx->instance->PostConnect = onPostConnect;
    ctx->instance->PostDisconnect = onPostDisconnect;
    ctx->instance->AuthenticateEx = onAuthenticate;
    /* The one certificate question, asked with the library's own store left
       out of it (see onVerifyX509Certificate). */
    ctx->instance->VerifyX509Certificate = onVerifyX509Certificate;

    applySettings(s, ctx->settings);

    if (freerdp_client_start(ctx) != CHANNEL_RC_OK) {
        fireClosed(s, "Could not start the connection");
        return NULL;
    }
    s->started = 1;

    if (!freerdp_connect(ctx->instance)) {
        const UINT32 err = freerdp_get_last_error(ctx);
        const char *detail = freerdp_get_last_error_string(err);
        LOGW("connect failed: 0x%08x (%s)", err, detail ? detail : "?");
        pthread_mutex_lock(&s->lock);
        const int asked = s->quit;
        pthread_mutex_unlock(&s->lock);
        fireClosed(s, asked ? "" : (detail ? detail : "The connection failed"));
        return NULL;
    }
    LOGI("handshake done: %s", freerdp_settings_get_string(ctx->settings, FreeRDP_ServerHostname));

    /* The library's own handles plus one of ours. Setting a flag does not wake
       a thread inside WaitForMultipleObjects, so the doorbell has to be in the
       set the wait is over. */
    while (!freerdp_shall_disconnect_context(ctx)) {
        pthread_mutex_lock(&s->lock);
        const int quit = s->quit;
        pthread_mutex_unlock(&s->lock);
        if (quit) {
            break;
        }
        HANDLE handles[64];
        handles[0] = s->wake;
        const DWORD count = freerdp_get_event_handles(ctx, &handles[1], 63);
        if (count == 0) {
            break;
        }
        const DWORD status = WaitForMultipleObjects(count + 1, handles, FALSE, 1000);
        if (status == WAIT_FAILED) {
            break;
        }
        /* Before the drain, not after: a command posted in between only costs a
           wakeup that finds nothing, where the other order loses it until the
           timeout. The reset is ours to do — a WinPR event handle has no
           cleanup hook, so no wait resets one whatever it was created as, and
           an auto-reset event left signalled turns this loop into a spin. */
        ResetEvent(s->wake);
        drainCommands(s);
        if (!freerdp_check_event_handles(ctx)) {
            break;
        }
    }

    pthread_mutex_lock(&s->lock);
    const int asked = s->quit;
    pthread_mutex_unlock(&s->lock);
    s->connected = 0;

    freerdp_disconnect(ctx->instance);
    const UINT32 err = freerdp_get_last_error(ctx);
    const char *detail = err != FREERDP_ERROR_SUCCESS ? freerdp_get_last_error_string(err) : NULL;
    fireClosed(s, asked ? "" : (detail ? detail : "The connection was lost"));
    return NULL;
}

/* ---- the session's surface ---------------------------------------------- */

static Session *sessionOfBase(vento_session *base) {
    return (Session *) base;
}

static char *dupOrNull(const char *s) {
    return s ? strdup(s) : NULL;
}

static void freeSession(Session *s) {
    if (s->wake) {
        CloseHandle(s->wake);
    }
    for (int i = s->tail; i != s->head; i = (i + 1) % CMD_RING) {
        free(s->ring[i].text);
    }
    free(s->fb);
    free(s->clipOut);
    free(s->host);
    free(s->user);
    free(s->domain);
    free(s->password);
    free(s->nla);
    free(s->graphics);
    free(s->experience);
    free(s->sound);
    free(s->clientName);
    free(s->configPath);
    free(s->credUser);
    free(s->credDomain);
    free(s->credPass);
    pthread_mutex_destroy(&s->lock);
    pthread_cond_destroy(&s->cond);
    pthread_rwlock_destroy(&s->fbLock);
    vento_h264_release(&s->h264);
    free(s);
}

const char *vento_freerdp_version(void) {
    static char buf[64];
    snprintf(buf, sizeof(buf), "FreeRDP %s", freerdp_get_version_string());
    return buf;
}

vento_session *vento_freerdp_create(const vento_freerdp_config *cfg, const vento_callbacks *cb) {
    Session *s = calloc(1, sizeof(Session));
    if (!s) {
        if (cfg->video && cfg->video->release) {
            cfg->video->release(cfg->video->user);
        }
        return NULL;
    }
    /* The decoders are the session's from here, released by freeSession on
       every way out. */
    vento_h264_init(&s->h264, cfg->video);
    s->base.ops = &OPS;
    s->cb = *cb;
    pthread_mutex_init(&s->lock, NULL);
    pthread_cond_init(&s->cond, NULL);
    pthread_rwlock_init(&s->fbLock, NULL);
    s->wake = CreateEvent(NULL, FALSE, FALSE, NULL);
    if (!s->wake) {
        LOGW("no wake event");
        freeSession(s);
        return NULL;
    }

    char *addr = dupOrNull(cfg->address);
    s->port = 3389;
    if (addr) {
        s->host = addr;
        splitAddress(addr, &s->port, s->addressError, sizeof s->addressError);
    }
    s->user = dupOrNull(cfg->user_name);
    s->domain = dupOrNull(cfg->domain);
    s->password = dupOrNull(cfg->password);
    s->nla = dupOrNull(cfg->nla);
    s->legacyTls = cfg->legacy_tls;
    s->graphics = dupOrNull(cfg->graphics);
    s->experience = dupOrNull(cfg->experience);
    s->sound = dupOrNull(cfg->sound);
    s->clientName = dupOrNull(cfg->client_name);
    s->osMajorType = OSMAJORTYPE_UNSPECIFIED;
    if (cfg->platform && strcmp(cfg->platform, "android") == 0) {
        s->osMajorType = OSMAJORTYPE_ANDROID;
    } else if (cfg->platform && strcmp(cfg->platform, "linux") == 0) {
        s->osMajorType = OSMAJORTYPE_UNIX;
    }
    s->configPath = dupOrNull(cfg->config_path);
    s->compression = cfg->compression;
    /* Out of the protocol's range is a scale both ends ignore, so a bad value
       is 100 rather than something a server has to argue with. */
    s->scale = cfg->scale == 140 || cfg->scale == 180 ? cfg->scale : 100;
    s->width = cfg->width;
    s->height = cfg->height;
    s->monitors = cfg->monitors;
    s->serverCount = -1;
    s->keyboardLayout = cfg->keyboard_layout;
    s->connectTimeoutMs = cfg->connect_timeout_ms;

    /* WinPR reads $HOME through GetKnownPath, and an app's process has no HOME
       at all — which is not a missing feature but a session that cannot start:
       freerdp_settings_new fails outright when the home path comes back null,
       so the context below is never allocated. Process-global, as every session
       here wants the same answer, and set before anything of FreeRDP's runs. */
    if (s->configPath) {
        setenv("HOME", s->configPath, 1);
    }

    /* ContextSize is what the library allocates and the client layer keeps
       rdpClientContext in; short of that, the client layer refuses the context
       at every call that needs its half. */
    RDP_CLIENT_ENTRY_POINTS entry = {0};
    entry.Size = sizeof(RDP_CLIENT_ENTRY_POINTS_V1);
    entry.Version = RDP_CLIENT_INTERFACE_VERSION;
    entry.ContextSize = sizeof(ShimContext);
    rdpContext *ctx = freerdp_client_context_new(&entry);
    if (!ctx) {
        LOGW("freerdp_client_context_new failed");
        freeSession(s);
        return NULL;
    }
    s->ctx = (ShimContext *) ctx;
    s->ctx->session = s;
    s->instance = ctx->instance;

    if (pthread_create(&s->thread, NULL, protocolThread, s) != 0) {
        freerdp_client_context_free(ctx);
        s->ctx = NULL;
        freeSession(s);
        return NULL;
    }
    return &s->base;
}

static void op_answer_credentials(vento_session *base, const char *user, const char *domain,
                                  const char *password) {
    Session *s = sessionOfBase(base);
    char *u = dupOrNull(user);
    char *d = dupOrNull(domain);
    char *p = dupOrNull(password);
    pthread_mutex_lock(&s->lock);
    free(s->credUser);
    free(s->credDomain);
    free(s->credPass);
    s->credUser = u;
    s->credDomain = d;
    s->credPass = p;
    s->credAnswered = 1;
    if (!p) {
        s->quit = 1;
    }
    pthread_cond_broadcast(&s->cond);
    pthread_mutex_unlock(&s->lock);
    SetEvent(s->wake);
}

static void op_answer_trust(vento_session *base, bool accept) {
    Session *s = sessionOfBase(base);
    pthread_mutex_lock(&s->lock);
    s->trustAccepted = accept;
    s->trustAnswered = 1;
    if (!s->trustAccepted) {
        s->quit = 1;
    }
    pthread_cond_broadcast(&s->cond);
    pthread_mutex_unlock(&s->lock);
    SetEvent(s->wake);
}

/* Both of the ways a session ends set the flag, wake the wait *and* abort the
   connect: during a handshake the thread is inside somebody else's socket read,
   which no event of ours is in the set of. */
static void stop(Session *s) {
    pthread_mutex_lock(&s->lock);
    s->quit = 1;
    pthread_cond_broadcast(&s->cond);
    pthread_mutex_unlock(&s->lock);
    if (s->ctx) {
        freerdp_abort_connect_context(&s->ctx->client.context);
    }
    SetEvent(s->wake);
}

static void op_disconnect(vento_session *base) {
    stop(sessionOfBase(base));
}

static void op_destroy(vento_session *base) {
    Session *s = sessionOfBase(base);
    stop(s);
    pthread_join(s->thread, NULL);

    if (s->ctx) {
        if (s->started) {
            freerdp_client_stop(&s->ctx->client.context);
        }
        freerdp_client_context_free(&s->ctx->client.context);
        s->ctx = NULL;
    }
    freeSession(s);
}

static void op_pointer(vento_session *base, int x, int y, int buttonMask) {
    Cmd c = {CMD_POINTER, x, y, buttonMask, NULL, 0};
    post(sessionOfBase(base), &c);
}

static void op_key_down(vento_session *base, uint32_t keysym, int64_t keyId) {
    Cmd c = {CMD_KEY_DOWN, (int) keysym, 0, keyId, NULL, 0};
    post(sessionOfBase(base), &c);
}

static void op_key_up(vento_session *base, int64_t keyId) {
    Cmd c = {CMD_KEY_UP, 0, 0, keyId, NULL, 0};
    post(sessionOfBase(base), &c);
}

static void op_release_all_keys(vento_session *base) {
    Cmd c = {CMD_RELEASE_KEYS, 0, 0, 0, NULL, 0};
    post(sessionOfBase(base), &c);
}

static void op_focus(vento_session *base, bool focused) {
    Cmd c = {CMD_FOCUS, focused, 0, 0, NULL, 0};
    post(sessionOfBase(base), &c);
}

static void op_view_only(vento_session *base, bool viewOnly) {
    Session *s = sessionOfBase(base);
    s->viewOnly = viewOnly;
    if (s->viewOnly) {
        /* Whatever is held at the far end is held by this session, so it is
           this session's to let go of when it stops driving. */
        Cmd c = {CMD_RELEASE_KEYS, 0, 0, 0, NULL, 0};
        post(s, &c);
    }
}

static bool op_can_resize(vento_session *base) {
    return sessionOfBase(base)->canResize != 0;
}

static void op_request_desktop_size(vento_session *base, int width, int height) {
    Cmd c = {CMD_RESIZE, width, height, 0, NULL, 0};
    post(sessionOfBase(base), &c);
}

void vento_freerdp_set_monitor_count(vento_session *base, int count) {
    /* Remembered rather than acted on: the count is what the *next* layout
       asks for, and a desktop somebody is working on does not get rearranged
       as a side effect of a settings row moving. */
    sessionOfBase(base)->monitors = count < 1 ? 1 : (count > 16 ? 16 : count);
}

/* The monitors the desktop is made of, in the server's order: as it last
   stated them, or as they were last asked for, whichever covers the desktop as
   it is now (see `covers`). None for a desktop of one monitor, which is not a
   layout, and none for a desktop neither covers. */
static int op_monitors(vento_session *base, int32_t *out) {
    Session *s = sessionOfBase(base);
    if (!s->connected) {
        return 0;
    }
    pthread_rwlock_rdlock(&s->fbLock);
    const int w = s->fbW, h = s->fbH;
    pthread_rwlock_unlock(&s->fbLock);
    pthread_mutex_lock(&s->lock);
    const int32_t *from = NULL;
    int n = 0;
    if (s->serverCount > 0 && covers(s->serverLayout, s->serverCount, w, h)) {
        from = s->serverLayout;
        n = s->serverCount;
    } else if (covers(s->askedLayout, s->askedCount, w, h)) {
        from = s->askedLayout;
        n = s->askedCount;
    }
    if (n < 2) {
        n = 0;
    } else {
        memcpy(out, from, sizeof(int32_t) * 4 * (size_t) n);
    }
    pthread_mutex_unlock(&s->lock);
    return n;
}

/* RDP has no name for a monitor but its place in the list, so the ids here
   are indices (no `screens` member) and a layout is sent in the order given:
   the first is the primary, and a screen past the far end's last is a new
   one. The desktop's size is the layout's bounding box, which is what
   `width` and `height` are anyway. */
static void op_request_layout(vento_session *base, int width, int height, const int32_t *screens,
                              int n) {
    (void) width;
    (void) height;
    Session *s = sessionOfBase(base);
    if (n < 1) {
        return;
    }
    n = n > VENTO_MAX_MONITORS ? VENTO_MAX_MONITORS : n;
    pthread_mutex_lock(&s->lock);
    for (int i = 0; i < n; i++) {
        memcpy(s->wantLayout + 4 * i, screens + 5 * i + 1, sizeof(int32_t) * 4);
    }
    s->wantCount = n;
    pthread_mutex_unlock(&s->lock);
    Cmd c = {CMD_LAYOUT, 0, 0, 0, NULL, 0};
    post(s, &c);
}

/* Held as the wire wants it, UTF-16 with its terminator, from the moment it
   arrives: the conversion is done once here rather than on every paste. */
static void op_clipboard(vento_session *base, const char *utf8) {
    if (!utf8) {
        return;
    }
    size_t units = 0;
    WCHAR *copy = ConvertUtf8ToWCharAlloc(utf8, &units);
    if (!copy) {
        return;
    }
    Cmd c = {CMD_CLIPBOARD, 0, 0, 0, copy, units};
    post(sessionOfBase(base), &c);
}

/* The facts: protocol, connection, security, encoding, line speed, server and
   viewer pixels — the order the Rust adapter's `info` labels them in. What the
   protocol has no concept of is omitted rather than printed empty, which is
   why there is no desktop name here: RDP has none. */
static bool op_info(vento_session *base, vento_strings *out) {
    Session *s = sessionOfBase(base);
    if (!s->connected || !s->ctx) {
        return false;
    }
    rdpSettings *settings = s->ctx->client.context.settings;
    char protocol[64], connection[128], security[64], lineSpeed[64], server[64], viewer[64];

    snprintf(protocol, sizeof(protocol), "RDP (FreeRDP %s)", freerdp_get_version_string());
    /* Written back the way it would have to be typed, brackets and all. */
    const char *host = s->host ? s->host : "";
    if (strchr(host, ':')) {
        snprintf(connection, sizeof(connection), "[%s]:%d", host, s->port);
    } else {
        snprintf(connection, sizeof(connection), "%s:%d", host, s->port);
    }
    const char *layer;
    switch (freerdp_settings_get_uint32(settings, FreeRDP_SelectedProtocol)) {
        case 0: layer = "RDP"; break;
        case 1: layer = "TLS"; break;
        case 2: layer = "NLA (CredSSP)"; break;
        case 4: layer = "RDSTLS"; break;
        case 8: layer = "NLA (CredSSP, extended)"; break;
        default: layer = "?"; break;
    }
    snprintf(security, sizeof(security), "%s", layer);

    /* The autodetect exchange's own number, in kilobits, and empty until the
       server has run one — which it does when it feels like it rather than at
       connect, so this row appears part way into a session. */
    const rdpAutoDetect *detect = autodetect_get(&s->ctx->client.context);
    const UINT32 kbps = detect ? detect->netCharBandwidth : 0;
    if (kbps >= 1000) {
        snprintf(lineSpeed, sizeof(lineSpeed), "%u.%u Mbit/s", kbps / 1000, (kbps % 1000) / 100);
    } else if (kbps > 0) {
        snprintf(lineSpeed, sizeof(lineSpeed), "%u kbit/s", kbps);
    } else {
        lineSpeed[0] = 0;
    }

    const UINT32 w = freerdp_settings_get_uint32(settings, FreeRDP_DesktopWidth);
    const UINT32 h = freerdp_settings_get_uint32(settings, FreeRDP_DesktopHeight);
    snprintf(server, sizeof(server), "%u×%u, %u bpp", w, h,
             freerdp_settings_get_uint32(settings, FreeRDP_ColorDepth));
    snprintf(viewer, sizeof(viewer), "%u×%u, 32 bpp", w, h);

    /* What the picture is arriving as rather than what was asked for: the
       pipeline's channel opening is the server agreeing to it, and the version
       and H.264 are what the server confirmed, since the settings that asked
       for them are left standing whatever it says. The codec below it is read
       off the surfaces themselves, for the same reason: a declined RemoteFX
       leaves the setting that asked for it standing too. */
    char encoding[64];
    if (s->gfxOpen) {
        const UINT32 version = s->gfxVersion;
        if (version) {
            const int hardware = atomic_load(&s->h264.hardware);
            snprintf(encoding, sizeof(encoding), "EGFX %u.%u%s%s", (version >> 16) & 0xFFFFu,
                     (version >> 8) & 0xFFu, s->gfxH264 ? ", H.264" : "",
                     !s->gfxH264 || hardware < 0 ? "" : hardware ? " (hardware)" : " (software)");
        } else {
            snprintf(encoding, sizeof(encoding), "EGFX");
        }
    } else if (s->lastCodec == RDP_CODEC_ID_REMOTEFX) {
        snprintf(encoding, sizeof(encoding), "RemoteFX");
    } else if (s->lastCodec == RDP_CODEC_ID_NSCODEC) {
        snprintf(encoding, sizeof(encoding), "NSCodec");
    } else {
        snprintf(encoding, sizeof(encoding), "Bitmaps");
    }

    const char *values[] = {protocol, connection, security, encoding, lineSpeed,
                            server, viewer};
    return vento_strings_set(out, values, sizeof(values) / sizeof(values[0]));
}

/* The overlay's line for the H.264: the last decoder's own, as RustDesk's and
   TigerVNC's are ("H.264 vaapi map 4.1 ms", "H.264 sw 3.2 ms (vaapi: no
   device)"). */
static bool op_decoder_status(vento_session *base, char *out, size_t len) {
    return vento_h264_status(&sessionOfBase(base)->h264, out, len);
}

/* Both directions out of the library's own statistics, which are counted where
   the transport reads and writes its front BIO — so they are the protocol,
   inside TLS and outside TCP, which is where every other backend here counts.
   Nothing is patched for this and no private header is reached into: the pair
   is published. */
static bool op_traffic(vento_session *base, uint64_t out[2]) {
    Session *s = sessionOfBase(base);
    if (!s->connected || !s->ctx) {
        return false;
    }
    UINT64 in = 0, sent = 0;
    if (!freerdp_get_stats(s->ctx->client.context.rdp, &in, &sent, NULL, NULL)) {
        return false;
    }
    out[0] = in;
    out[1] = sent;
    return true;
}

static bool op_read_region(vento_session *base, int x, int y, int w, int h, void *dst,
                           size_t stride, int dstX, int dstY) {
    Session *s = sessionOfBase(base);
    if (w <= 0 || h <= 0 || dstX < 0 || dstY < 0) {
        return false;
    }
    bool ok = false;
    pthread_rwlock_rdlock(&s->fbLock);
    /* Subtracted rather than added: `x + w` near INT_MAX wraps negative and
       would pass, and the rows would be read from before the framebuffer. */
    if (s->fb && x >= 0 && y >= 0 && x <= s->fbW && y <= s->fbH && w <= s->fbW - x
            && h <= s->fbH - y) {
        for (int row = 0; row < h; row++) {
            const uint8_t *src = s->fb + ((size_t) (y + row) * (size_t) s->fbW + x) * 4u;
            uint8_t *out = (uint8_t *) dst + (size_t) (dstY + row) * stride + (size_t) dstX * 4u;
            memcpy(out, src, (size_t) w * 4u);
        }
        ok = true;
    }
    pthread_rwlock_unlock(&s->fbLock);
    return ok;
}

static bool op_read_thumbnail(vento_session *base, int step, void *dst, size_t stride, int dstW,
                              int dstH) {
    Session *s = sessionOfBase(base);
    if (step < 1) {
        return false;
    }
    bool ok = false;
    pthread_rwlock_rdlock(&s->fbLock);
    if (s->fb && s->fbW > 0 && s->fbH > 0) {
        const int tw = (s->fbW + step - 1) / step;
        const int th = (s->fbH + step - 1) / step;
        if (tw <= dstW && th <= dstH) {
            for (int row = 0; row < th; row++) {
                const uint32_t *src = (const uint32_t *) (s->fb
                        + (size_t) (row * step) * (size_t) s->fbW * 4u);
                uint32_t *out = (uint32_t *) ((uint8_t *) dst + (size_t) row * stride);
                for (int col = 0; col < tw; col++) {
                    out[col] = src[col * step];
                }
            }
            ok = true;
        }
    }
    pthread_rwlock_unlock(&s->fbLock);
    return ok;
}

static const vento_session_ops OPS = {
        .size = sizeof(vento_session_ops),
        .pointer = op_pointer,
        .key_down = op_key_down,
        .key_up = op_key_up,
        .release_all_keys = op_release_all_keys,
        .focus = op_focus,
        .view_only = op_view_only,
        .can_resize = op_can_resize,
        .monitors = op_monitors,
        .request_desktop_size = op_request_desktop_size,
        .clipboard = op_clipboard,
        .info = op_info,
        .traffic = op_traffic,
        .read_region = op_read_region,
        .read_thumbnail = op_read_thumbnail,
        .answer_credentials = op_answer_credentials,
        .answer_trust = op_answer_trust,
        .disconnect = op_disconnect,
        .destroy = op_destroy,
        .request_layout = op_request_layout,
        .decoder_status = op_decoder_status,
};
