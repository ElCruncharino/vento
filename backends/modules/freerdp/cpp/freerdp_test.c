// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

// Tests of the session file's pure parts, built with the probes
// (`-DVENTO_PROBES=ON`) and run by `ctest` in that build tree or as
// `freerdp/vento_freerdp_test`: the address, the keysym tables, the monitor
// layouts in both directions, the full command ring, the keys sent for a
// press and its release, the effect of the display-control channel's
// capabilities on a layout, the H.264 subsystem's handling of a decoder that
// won't open, and the sound switch as rdpsnd devices come and go.
//
// The three sources are included rather than linked, to reach their static
// functions; nothing in them connects anywhere. The two input calls used for
// keys are the library's, so the build wraps them (`--wrap`) and the
// wrappers here record what would have been sent.

#include "freerdp_session.c"
#include "h264_vento.c"
#include "rdpsnd_vento.c"

static int failures;

#define CHECK(cond)                                                                   \
    do {                                                                              \
        if (!(cond)) {                                                                \
            fprintf(stderr, "%s:%d: %s: CHECK failed: %s\n", __FILE__, __LINE__,      \
                    __func__, #cond);                                                 \
            failures++;                                                               \
        }                                                                             \
    } while (0)

/* ---- what the keys sent ------------------------------------------------- */

typedef struct {
    int unicode; /* a Unicode event rather than a scancode */
    UINT16 flags, code;
} Sent;

static Sent sent[64];
static int nsent;

BOOL __wrap_freerdp_input_send_keyboard_event(rdpInput *input, UINT16 flags, UINT8 code);
BOOL __wrap_freerdp_input_send_unicode_keyboard_event(rdpInput *input, UINT16 flags, UINT16 code);

BOOL __wrap_freerdp_input_send_keyboard_event(rdpInput *input, UINT16 flags, UINT8 code) {
    (void) input;
    if (nsent < 64) {
        sent[nsent++] = (Sent){0, flags, code};
    }
    return TRUE;
}

BOOL __wrap_freerdp_input_send_unicode_keyboard_event(rdpInput *input, UINT16 flags,
                                                      UINT16 code) {
    (void) input;
    if (nsent < 64) {
        sent[nsent++] = (Sent){1, flags, code};
    }
    return TRUE;
}

/* A session with only its locks set up, and a placeholder instance and
   context. */
static freerdp fakeInstance;
static rdpContext fakeContext;

static Session *testSession(void) {
    Session *s = calloc(1, sizeof(Session));
    pthread_mutex_init(&s->lock, NULL);
    pthread_cond_init(&s->cond, NULL);
    pthread_rwlock_init(&s->fbLock, NULL);
    fakeContext.input = (rdpInput *) &fakeContext;
    fakeInstance.context = &fakeContext;
    s->instance = &fakeInstance;
    s->connected = 1;
    s->serverCount = -1;
    s->scale = 100;
    s->wake = CreateEventA(NULL, TRUE, FALSE, NULL);
    return s;
}

static void freeTestSession(Session *s) {
    for (int i = s->tail; i != s->head; i = (i + 1) % CMD_RING) {
        free(s->ring[i].text);
    }
    CloseHandle(s->wake);
    pthread_mutex_destroy(&s->lock);
    pthread_cond_destroy(&s->cond);
    pthread_rwlock_destroy(&s->fbLock);
    free(s);
}

/* ---- the address -------------------------------------------------------- */

static void split(const char *in, const char *host, int port, const char *why) {
    char buf[128], said[128] = "";
    int p = 3389;
    snprintf(buf, sizeof(buf), "%s", in);
    splitAddress(buf, &p, said, sizeof(said));
    if (why) {
        if (strstr(said, why) == NULL) {
            fprintf(stderr, "splitAddress(\"%s\"): said \"%s\", wanted \"%s\"\n", in, said, why);
            failures++;
        }
        return;
    }
    if (said[0] || strcmp(buf, host) != 0 || p != port) {
        fprintf(stderr, "splitAddress(\"%s\"): \"%s\" %d \"%s\", wanted \"%s\" %d\n", in, buf, p,
                said, host, port);
        failures++;
    }
}

static void testAddress(void) {
    split("host", "host", 3389, NULL);
    split("  host\t", "host", 3389, NULL);
    split("host:3390", "host", 3390, NULL);
    split("10.0.0.1:1", "10.0.0.1", 1, NULL);
    split("[::1]", "::1", 3389, NULL);
    split("[fe80::1%eth0]:3391", "fe80::1%eth0", 3391, NULL);
    split("", NULL, 0, "No host");
    split(":3389", NULL, 0, "No host");
    split("::1", NULL, 0, "must be bracketed");
    split("[::1", NULL, 0, "missing closing bracket");
    split("[::1]x", NULL, 0, "junk after");
    split("host:", NULL, 0, "port number");
    split("host:0", NULL, 0, "port number");
    split("host:65536", NULL, 0, "port number");
    split("host:33a", NULL, 0, "port number");
}

/* ---- the keysyms -------------------------------------------------------- */

static void key(UINT32 keysym, UINT16 code, int shift, WCHAR unicode) {
    UINT16 c;
    int s;
    WCHAR u;
    mapKeysym(keysym, &c, &s, &u);
    if (c != code || s != shift || u != unicode) {
        fprintf(stderr, "mapKeysym(0x%x): 0x%x %d U+%04x, wanted 0x%x %d U+%04x\n",
                (unsigned) keysym, c, s, u, code, shift, unicode);
        failures++;
    }
}

static void testKeysyms(void) {
    key('a', 0x1e, 0, 0);
    key('A', 0x1e, 1, 0);
    key('!', 0x02, 1, 0);
    key(' ', 0x39, 0, 0);
    key('`', 0x29, 0, 0);
    key('|', 0x2b, 1, 0);
    key(0xff0d, 0x1c, 0, 0);          /* Return */
    key(0xff8d, KEY_EXT | 0x1c, 0, 0); /* KP_Enter */
    key(0xffff, KEY_EXT | 0x53, 0, 0); /* Delete */
    key(0xffe4, KEY_EXT | 0x1d, 0, 0); /* Control_R */
    key(0xfe03, KEY_EXT | 0x38, 0, 0); /* ISO_Level3_Shift */
    key(0xffbe, 0x3b, 0, 0);          /* F1 */
    key(0xffc7, 0x44, 0, 0);          /* F10 */
    key(0xffc8, 0x57, 0, 0);          /* F11 */
    key(0xffca, 0x64, 0, 0);          /* F13 */
    key(0xffd4, 0x6e, 0, 0);          /* F23 */
    key(0xffd5, 0x76, 0, 0);          /* F24 */
    key(0xff13, KEY_PAUSE, 0, 0);
    key(0xff95, 0x47, 0, 0); /* KP_Home */
    key(0xffb7, 0x47, 0, 0); /* KP_7, the same scancode */
    key(0xffbd, KEY_NONE, 0, '=');      /* KP_Equal: no scancode */
    key(0xe9, KEY_NONE, 0, 0xe9);       /* eacute, Latin-1 as itself */
    key(0x10020ac, KEY_NONE, 0, 0x20ac); /* the euro sign */
    key(0x101f600, KEY_NONE, 0, 0);     /* past the BMP: nothing to send */
    key(0xfe50, KEY_NONE, 0, 0);        /* dead_grave: no scancode, no character */
    CHECK(namedScancode('a') == KEY_NONE);
}

/* ---- the keys sent ------------------------------------------------------ */

static void testKeysSent(void) {
    Session *s = testSession();

    /* A character with no scancode is sent as Unicode, down and then up. */
    nsent = 0;
    doKeyDown(s, 0xe9, 1);
    doKeyUp(s, 1);
    CHECK(nsent == 2);
    CHECK(sent[0].unicode && sent[0].flags == 0 && sent[0].code == 0xe9);
    CHECK(sent[1].unicode && sent[1].flags == KBD_FLAGS_RELEASE && sent[1].code == 0xe9);

    /* Auto-repeat is another down, and one up at the end. */
    nsent = 0;
    doKeyDown(s, 0x10020ac, 2);
    doKeyDown(s, 0x10020ac, 2);
    doKeyUp(s, 2);
    CHECK(nsent == 3);
    CHECK(sent[1].unicode && sent[1].flags == 0 && sent[1].code == 0x20ac);
    CHECK(sent[2].unicode && sent[2].flags == KBD_FLAGS_RELEASE);
    CHECK(s->nkeys == 0);

    /* A capital typed without Shift held gets a synthesised Shift, released
       after it. */
    nsent = 0;
    doKeyDown(s, 'A', 3);
    doKeyUp(s, 3);
    CHECK(nsent == 4);
    CHECK(!sent[0].unicode && sent[0].code == 0x2a && sent[0].flags == KBD_FLAGS_DOWN);
    CHECK(sent[1].code == 0x1e && sent[1].flags == KBD_FLAGS_DOWN);
    CHECK(sent[2].code == 0x1e && sent[2].flags == KBD_FLAGS_RELEASE);
    CHECK(sent[3].code == 0x2a && sent[3].flags == KBD_FLAGS_RELEASE);

    /* With Shift held by the user, it doesn't. */
    nsent = 0;
    doKeyDown(s, 0xffe1, 4);
    doKeyDown(s, 'A', 5);
    doKeyUp(s, 5);
    doKeyUp(s, 4);
    CHECK(nsent == 4);
    CHECK(sent[0].code == 0x2a && sent[1].code == 0x1e && sent[2].code == 0x1e
          && sent[3].code == 0x2a && sent[3].flags == KBD_FLAGS_RELEASE);

    /* Releasing everything sends every release, Unicode ones included. */
    nsent = 0;
    doKeyDown(s, 0xe9, 6);
    doKeyDown(s, 0xffff, 7);
    doReleaseAll(s);
    CHECK(nsent == 4);
    CHECK(sent[2].flags & KBD_FLAGS_RELEASE);
    CHECK(sent[3].flags & KBD_FLAGS_RELEASE);
    CHECK(s->nkeys == 0);

    freeTestSession(s);
}

/* ---- the command ring --------------------------------------------------- */

static int count(const Session *s, int type) {
    int n = 0;
    for (int i = s->tail; i != s->head; i = (i + 1) % CMD_RING) {
        n += s->ring[i].type == type;
    }
    return n;
}

static int queued(const Session *s) {
    return (s->head - s->tail + CMD_RING) % CMD_RING;
}

static void testRing(void) {
    /* More presses and releases than the ring holds: every release stays, in
       order, and presses are dropped oldest first. */
    Session *s = testSession();
    for (int i = 0; i < 400; i++) {
        Cmd down = {CMD_KEY_DOWN, 'a', 0, i, NULL, 0};
        Cmd up = {CMD_KEY_UP, 0, 0, i, NULL, 0};
        post(s, &down);
        post(s, &up);
    }
    CHECK(queued(s) == CMD_RING - 1);
    CHECK(count(s, CMD_KEY_UP) == 400);
    INT64 last = -1;
    int ordered = 1;
    for (int i = s->tail; i != s->head; i = (i + 1) % CMD_RING) {
        if (s->ring[i].type == CMD_KEY_UP) {
            ordered = ordered && s->ring[i].c > last;
            last = s->ring[i].c;
        }
    }
    CHECK(ordered);
    CHECK(s->dropped == 289);
    freeTestSession(s);

    /* A ring of pointer changes between two clipboards, then a release: the
       oldest command restated by a later one (the first clipboard, with its
       text) is dropped, and the release is last. */
    s = testSession();
    WCHAR *text = calloc(4, sizeof(WCHAR));
    Cmd clip = {CMD_CLIPBOARD, 0, 0, 0, text, 3};
    post(s, &clip);
    for (int i = 0; i < CMD_RING - 3; i++) {
        Cmd move = {CMD_POINTER, i, i, i & 1, NULL, 0};
        post(s, &move);
    }
    Cmd clip2 = {CMD_CLIPBOARD, 0, 0, 0, calloc(4, sizeof(WCHAR)), 3};
    post(s, &clip2);
    CHECK(queued(s) == CMD_RING - 1);
    Cmd up = {CMD_KEY_UP, 0, 0, 99, NULL, 0};
    post(s, &up);
    CHECK(queued(s) == CMD_RING - 1);
    CHECK(s->ring[(s->head + CMD_RING - 1) % CMD_RING].type == CMD_KEY_UP);
    CHECK(count(s, CMD_CLIPBOARD) == 1);
    CHECK(s->ring[s->tail].type == CMD_POINTER);
    freeTestSession(s);

    /* A ring of only releases: a press is dropped, and a release replaces
       the oldest. */
    s = testSession();
    for (int i = 0; i < CMD_RING - 1; i++) {
        Cmd r = {CMD_KEY_UP, 0, 0, i, NULL, 0};
        post(s, &r);
    }
    Cmd down = {CMD_KEY_DOWN, 'a', 0, 1000, NULL, 0};
    post(s, &down);
    CHECK(count(s, CMD_KEY_DOWN) == 0);
    Cmd r = {CMD_RELEASE_KEYS, 0, 0, 0, NULL, 0};
    post(s, &r);
    CHECK(s->ring[s->tail].c == 1);
    CHECK(s->ring[(s->head + CMD_RING - 1) % CMD_RING].type == CMD_RELEASE_KEYS);
    freeTestSession(s);
}

/* ---- the monitors ------------------------------------------------------- */

static void testCovers(void) {
    const int32_t two[] = {0, 0, 1920, 1080, 1920, 0, 1920, 1080};
    CHECK(covers(two, 2, 3840, 1080));
    CHECK(!covers(two, 2, 3840, 1200));
    CHECK(!covers(two, 0, 0, 0));
    const int32_t negative[] = {-1, 0, 100, 100};
    CHECK(!covers(negative, 1, 99, 100));
    /* An edge past 32 bits does not cover a desktop of 100 (it did when the
       sum wrapped). */
    const int32_t wraps[] = {0, 0, 100, 100, INT32_MAX - 10, 0, 20, 100};
    CHECK(!covers(wraps, 2, 100, 100));
}

static void testFlatten(void) {
    int32_t flat[4 * VENTO_MAX_MONITORS];
    /* The primary at 0,0 with one to its left: the left one's corner is the
       desktop's. Corners are inclusive. */
    const MONITOR_DEF two[] = {{0, 0, 1919, 1079, MONITOR_PRIMARY}, {-1280, 0, -1, 1023, 0}};
    CHECK(flattenLayout(two, 2, flat));
    CHECK(flat[0] == 1280 && flat[1] == 0 && flat[2] == 1920 && flat[3] == 1080);
    CHECK(flat[4] == 0 && flat[5] == 0 && flat[6] == 1280 && flat[7] == 1024);

    const MONITOR_DEF backwards[] = {{100, 0, 99, 99, 0}};
    CHECK(!flattenLayout(backwards, 1, flat));
    const MONITOR_DEF huge[] = {{INT32_MIN, 0, INT32_MAX, 99, 0}};
    CHECK(!flattenLayout(huge, 1, flat));
    const MONITOR_DEF apart[] = {{0, 0, 1919, 1079, 0}, {40000, 0, 41919, 1079, 0}};
    CHECK(!flattenLayout(apart, 2, flat));
    const MONITOR_DEF widest[] = {{0, 0, MAX_DESKTOP_SIDE - 1, 99, 0}};
    CHECK(flattenLayout(widest, 1, flat) && flat[2] == MAX_DESKTOP_SIDE);

    /* heardLayout ignores an invalid layout and keeps a valid one. */
    Session *s = testSession();
    heardLayout(s, two, 2, "test");
    CHECK(s->serverCount == 2);
    heardLayout(s, huge, 1, "test");
    CHECK(s->serverCount == 0);
    freeTestSession(s);
}

static void testBuildLayout(void) {
    DISPLAY_CONTROL_MONITOR_LAYOUT out[VENTO_MAX_MONITORS];
    int32_t asked[4 * VENTO_MAX_MONITORS];
    /* Too narrow and odd, too wide, too short: clamped, and the screen to the
       right of the widened one moves by the same amount. */
    const int32_t rects[] = {0, 0, 151, 300, 151, 0, 9000, 100};
    CHECK(buildLayout(rects, 2, 150, out, asked) == 2);
    CHECK(out[0].Width == 200 && out[0].Height == 300 && out[0].Left == 0 && out[0].Top == 0);
    CHECK(out[0].Flags == DISPLAY_CONTROL_MONITOR_PRIMARY && out[1].Flags == 0);
    CHECK(out[1].Width == 8192 && out[1].Height == 200 && out[1].Left == 200);
    CHECK(out[0].DesktopScaleFactor == 150 && out[1].DeviceScaleFactor == 150);
    CHECK(asked[4] == 200 && asked[6] == 8192);

    /* The primary is the origin, even with a screen to its left. */
    const int32_t left[] = {1920, 0, 1920, 1080, 0, 0, 1920, 1080};
    CHECK(buildLayout(left, 2, 100, out, asked) == 2);
    CHECK(out[0].Left == 0 && out[1].Left == -1920);
    CHECK(asked[0] == 1920 && asked[4] == 0);
}

/* ---- the display-control channel ---------------------------------------- */

static UINT32 sentMonitors;
static DISPLAY_CONTROL_MONITOR_LAYOUT sentLayout[VENTO_MAX_MONITORS];
static int layouts;

static UINT fakeSendLayout(DispClientContext *disp, UINT32 n, DISPLAY_CONTROL_MONITOR_LAYOUT *m) {
    (void) disp;
    sentMonitors = n;
    memcpy(sentLayout, m, sizeof(*m) * n);
    layouts++;
    return CHANNEL_RC_OK;
}

static void testDisplayControl(void) {
    Session *s = testSession();
    DispClientContext disp = {0};
    disp.SendMonitorLayout = fakeSendLayout;
    s->disp = &disp;
    disp.custom = s;
    const int32_t two[] = {0, 0, 1920, 1080, 1920, 0, 1920, 1080};

    /* Before the capabilities, nothing is sent: the desktop cannot be
       resized yet, and the latest requested size waits for them. */
    layouts = 0;
    sendLayout(s, two, 2);
    CHECK(layouts == 0);
    CHECK(!op_can_resize(&s->base));
    doResize(s, 1024, 768);
    doResize(s, 1600, 1000);
    CHECK(layouts == 0);
    CHECK(onDisplayControlCaps(&disp, 16, 8192, 8192) == CHANNEL_RC_OK);
    CHECK(queued(s) == 1 && s->ring[s->tail].type == CMD_RESIZE);
    drainCommands(s);
    CHECK(layouts == 1 && sentMonitors == 1 && sentLayout[0].Width == 1600);

    /* The same for a layout. */
    s->canResize = 0;
    memcpy(s->wantLayout, two, sizeof(two));
    s->wantCount = 2;
    doLayout(s);
    CHECK(layouts == 1 && s->wantCount == 2);
    CHECK(onDisplayControlCaps(&disp, 16, 8192, 8192) == CHANNEL_RC_OK);
    drainCommands(s);
    CHECK(layouts == 2 && sentMonitors == 2 && s->wantCount == 0);
    layouts = 0;

    /* A single-monitor server: a layout of two is cut to one, and a resize
       of a session set to two monitors requests one of the whole size. */
    CHECK(onDisplayControlCaps(&disp, 1, 8192, 8192) == CHANNEL_RC_OK);
    CHECK(op_can_resize(&s->base));
    sendLayout(s, two, 2);
    CHECK(layouts == 1 && sentMonitors == 1 && s->askedCount == 1);
    s->monitors = 2;
    doResize(s, 2560, 1440);
    CHECK(layouts == 2 && sentMonitors == 1 && sentLayout[0].Width == 2560);

    /* Room for two, each up to 1920 x 1080: two fit, a bigger pair does not. */
    CHECK(onDisplayControlCaps(&disp, 2, 1920, 1080) == CHANNEL_RC_OK);
    sendLayout(s, two, 2);
    CHECK(layouts == 3 && sentMonitors == 2);
    const int32_t big[] = {0, 0, 2560, 1440, 2560, 0, 2560, 1440};
    sendLayout(s, big, 2);
    CHECK(layouts == 3);
    CHECK(queued(s) == 0);

    /* A server allowing no monitors cannot be resized. */
    CHECK(onDisplayControlCaps(&disp, 0, 8192, 8192) == CHANNEL_RC_OK);
    CHECK(!op_can_resize(&s->base));
    freeTestSession(s);
}

/* ---- the H.264 subsystem ------------------------------------------------ */

static int opens, hardwareOpens;
static bool hardwareOpensFail, everyOpenFails;
static int fakeDecoder;

static vento_video_decoder *fakeOpen(void *user, int codec, int w, int h, bool hardware) {
    (void) user, (void) codec, (void) w, (void) h;
    opens++;
    hardwareOpens += hardware;
    if (everyOpenFails || (hardware && hardwareOpensFail)) {
        return NULL;
    }
    return (vento_video_decoder *) &fakeDecoder;
}

static int fakeMode(void *user) {
    (void) user;
    return 1;
}

static bool fakeDecode(vento_video_decoder *d, const uint8_t *data, size_t len,
                       vento_video_planes *out) {
    (void) d, (void) data, (void) len, (void) out;
    return false;
}

static void fakeClose(vento_video_decoder *d) {
    (void) d;
}

static int decompress(H264_CONTEXT *h264, UINT32 w, UINT32 h) {
    static const BYTE frame[4] = {0, 0, 0, 1};
    h264->width = w;
    h264->height = h;
    return vento_h264_subsystem_decompress(h264, frame, sizeof(frame));
}

static void testH264(void) {
    const vento_video_decoders table = {
            .open = fakeOpen, .mode = fakeMode, .decode_planes = fakeDecode, .close = fakeClose};
    VentoH264 v;
    CHECK(vento_h264_init(&v, &table));
    vento_h264_enter(&v);
    H264_CONTEXT h264 = {0};
    CHECK(vento_h264_subsystem_init(&h264));
    vento_h264_leave();
    Context *c = h264.pSystemData;

    /* No hardware decoder: software immediately, on the same frame. */
    hardwareOpensFail = true;
    opens = hardwareOpens = 0;
    decompress(&h264, 1920, 1080);
    CHECK(opens == 2 && hardwareOpens == 1);
    CHECK(c->decoder != NULL && !c->hardware);
    CHECK(atomic_load(&v.hardwareHeld) == 0);
    decompress(&h264, 1920, 1080);
    CHECK(opens == 2);

    /* No decoder at all: logged once, not retried at the same size, retried
       at another. */
    everyOpenFails = true;
    opens = 0;
    CHECK(decompress(&h264, 4096, 2160) < 0);
    CHECK(opens == 2);
    CHECK(decompress(&h264, 4096, 2160) < 0);
    CHECK(decompress(&h264, 4096, 2160) < 0);
    CHECK(opens == 2);
    everyOpenFails = hardwareOpensFail = false;
    decompress(&h264, 1280, 720);
    CHECK(opens == 3 && c->decoder != NULL && c->hardware);
    CHECK(atomic_load(&v.hardwareHeld) == 1);

    vento_h264_subsystem_uninit(&h264);
    CHECK(atomic_load(&v.hardwareHeld) == 0);
    vento_h264_release(&v);
}

/* ---- the sound switch --------------------------------------------------- */

static int offers, withdrawals;

static void fakeOffer(void *user, int sw, bool enabled, void (*enable)(void *ctx, bool on),
                      void *ctx) {
    (void) user, (void) sw, (void) enabled, (void) enable, (void) ctx;
    offers++;
}

static void fakeWithdraw(void *user) {
    (void) user;
    withdrawals++;
}

static vento_audio_stream *fakeStreamOpen(void *user, int format, int rate, int channels) {
    (void) user, (void) format, (void) rate, (void) channels;
    return NULL;
}

static void fakeWrite(vento_audio_stream *st, const void *data, size_t len) {
    (void) st, (void) data, (void) len;
}

static void fakeStreamClose(vento_audio_stream *st) {
    (void) st;
}

static Device *testDevice(VentoSound *sound) {
    Device *d = calloc(1, sizeof(Device));
    d->sound = sound;
    pthread_mutex_init(&d->lock, NULL);
    pthread_cond_init(&d->cond, NULL);
    return d;
}

static void testSoundSwitch(void) {
    const vento_audio table = {.open = fakeStreamOpen,
                               .write = fakeWrite,
                               .close = fakeStreamClose,
                               .offer = fakeOffer,
                               .withdraw = fakeWithdraw};
    VentoSound sound;
    CHECK(vento_sound_init(&sound, &table));
    const AUDIO_FORMAT pcm = {.wFormatTag = WAVE_FORMAT_PCM,
                              .nChannels = 2,
                              .nSamplesPerSec = 44100,
                              .wBitsPerSample = 16};
    const AUDIO_FORMAT odd = {.wFormatTag = WAVE_FORMAT_PCM,
                              .nChannels = 2,
                              .nSamplesPerSec = 4000,
                              .wBitsPerSample = 16};

    /* The static and the dynamic channel, both offering: the switch stays
       until both are gone. */
    Device *a = testDevice(&sound), *b = testDevice(&sound);
    CHECK(serverFormatAnnounce(&a->device, &pcm, 1) == CHANNEL_RC_OK);
    CHECK(serverFormatAnnounce(&b->device, &pcm, 1) == CHANNEL_RC_OK);
    CHECK(serverFormatAnnounce(&b->device, &pcm, 1) == CHANNEL_RC_OK);
    CHECK(offers == 3 && atomic_load(&sound.offering) == 2);
    free_(&a->device);
    CHECK(withdrawals == 0);
    free_(&b->device);
    CHECK(withdrawals == 1);

    /* A server that then lists nothing playable withdraws it; a device that
       never offered withdraws nothing when freed. */
    Device *c = testDevice(&sound), *d = testDevice(&sound);
    CHECK(serverFormatAnnounce(&c->device, &pcm, 1) == CHANNEL_RC_OK);
    CHECK(serverFormatAnnounce(&c->device, &odd, 1) == CHANNEL_RC_OK);
    CHECK(withdrawals == 2);
    free_(&c->device);
    CHECK(serverFormatAnnounce(&d->device, &odd, 1) == CHANNEL_RC_OK);
    free_(&d->device);
    CHECK(withdrawals == 2);
    vento_sound_release(&sound);
}

int main(void) {
    testAddress();
    testKeysyms();
    testKeysSent();
    testRing();
    testCovers();
    testFlatten();
    testBuildLayout();
    testDisplayControl();
    testH264();
    testSoundSwitch();
    if (failures) {
        fprintf(stderr, "%d checks failed\n", failures);
        return 1;
    }
    printf("vento_freerdp_test: all passed\n");
    return 0;
}
