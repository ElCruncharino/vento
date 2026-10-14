// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

// Where the far end's sound goes, which is the platform's to say. TigerVNC's
// QEMU audio extension is the protocol half of it, in `common/`, and the sink
// is their viewer's — a PulseAudio one and a waveOut one, behind an
// `AudioOutput` of their own in `vncviewer/`, which this build does not
// compile. So the shape of theirs is said again here, in the three calls
// `Conn` makes, and the sinks are the platforms': `AndroidAudioOutput` on
// AAudio under `backends/platform/android/tigervnc`, `AlsaAudioOutput` under
// `backends/platform/desktop/tigervnc`. The session asks the factory below
// for one and names neither, the way `vento_log` is declared by the neutral
// layer and defined by whichever platform is linked in.
//
// What CConnection asks every server for — 16-bit signed, stereo, 48 kHz —
// it keeps to itself in CConnection.cxx, and their viewer restates it in its
// own header the same way; each sink here restates it beside the stream it
// opens, where a mismatch would be. The policy is theirs too, numbers
// included: at most a second of audio queued ahead, and 20 ms of silence in
// front of every stream so that a packet arriving a little late does not
// leave the device with nothing to play.

#ifndef REMOTEDESKTOP_AUDIOOUTPUT_H
#define REMOTEDESKTOP_AUDIOOUTPUT_H

#include <stddef.h>
#include <stdint.h>

class AudioOutput {
public:
    virtual ~AudioOutput() = default;

    /** The server has started sending: open if need be, start, pre-roll. */
    virtual void start() = 0;

    /** The server has stopped: let what is queued play out, then stop. */
    virtual void stop() = 0;

    /** A chunk of samples in the agreed format, aligned to nothing at all. */
    virtual void play(const uint8_t *data, size_t length) = 0;
};

/** A sink on this platform's own output, new, for one connection; the caller
    owns it. Every call on it comes from the protocol thread, and none of them
    may wait on the device: a sink that blocked would stall the picture behind
    the sound. Defined by whichever platform file is compiled in. */
AudioOutput *vento_tigervnc_audio_output();

#endif
