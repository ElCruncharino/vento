// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

// FreeRDP's sound, the rdpsnd channel, played on vento's output (vento/audio.h)
// rather than on a device of the library's own: what the session and the
// module's rdpsnd device (rdpsnd_vento.c) share.
//
// The channel's protocol half is the library's: the formats offered and
// confirmed, the wave PDUs and their confirms, the volume. What it leaves to a
// device is the sound itself, and it picks one by name from the channel's
// arguments ("sys:<name>"), looked up through the addin provider every client
// of the library registers — public API, so the module puts its device there
// without a patch: the session registers a provider that answers "vento"
// itself and asks the library's own for everything else, and names "vento"
// in the arguments of both rdpsnd channels, the static and the dynamic one.
//
// A device is made with nothing but the channel, which knows the library's
// context and nothing of the session; the session says, for a context, which
// sound is its (vento_sound_of), as it does for the H.264 contexts.

#ifndef VENTO_FREERDP_RDPSND_VENTO_H
#define VENTO_FREERDP_RDPSND_VENTO_H

#include <stdbool.h>

#include <freerdp/freerdp.h>

#include <vento/audio.h>

/* What the session names in the channel's arguments. */
#define VENTO_RDPSND_SUBSYSTEM "vento"

/* The library's channel drops a wave of its own accord where it thinks the
   device is too far behind, by the wall clock since the stream began: more
   than two waves plus the latency it was given, which is nothing unless the
   arguments say otherwise. The table's queue already keeps about a tenth of
   a second ahead and drops the oldest past a quarter (vento/audio.h), so the
   channel is given that quarter: its own check then never fires before the
   queue's, and a burst that arrives at once after the network held it is
   the queue's to trim rather than the channel's to lose a wave of. */
#define VENTO_RDPSND_LATENCY_MS 250

/* One session's sound: the platform's output, which every rdpsnd device the
   session's channels make opens its streams on. */
typedef struct VentoSound {
    vento_audio table;
    bool available;
} VentoSound;

/* The library's own symbols, not the backend's: only vento/freerdp.h's are
   exported. */
#pragma GCC visibility push(hidden)

/* `table`, which is the session's from the call on whatever comes back;
   false, with it released, where it is NULL or misses a call this needs. */
bool vento_sound_init(VentoSound *v, const vento_audio *table);

/* After the last device is freed, which is after the channels are. */
void vento_sound_release(VentoSound *v);

/* Puts the module's device where the channel looks for one. The library
   puts its own provider back whenever a client context is made, so this
   comes after freerdp_client_context_new, every time. */
bool vento_sound_register(void);

/* The session's: the sound of the session `context` is. */
VentoSound *vento_sound_of(rdpContext *context);

#pragma GCC visibility pop

#endif
