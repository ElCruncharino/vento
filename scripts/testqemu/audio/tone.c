// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

// The audio guest's one program: it keeps the sound card playing silence and
// plays a burst of 440 Hz at half scale each time a 't' arrives on the serial
// port, so that the host decides when every tone starts and can time it to
// the null sink's recording (scripts/testaudio/bursts.py).
//
// The stream never stops between bursts, which is what a guest playing music
// looks like to QEMU: one AudioBegin, data for as long as it runs, and the
// client's queue under a steady load rather than a new stream per burst.
// The card's buffer is kept short (40 ms) and the serial port is looked at
// every millisecond, and the guest's own share of the delay is answered back
// on the serial port for each burst — how long after the byte arrived the
// first tone frame was written, and how much was already queued ahead of it
// in the card — so that the host can take it out of what it measures. An 's'
// stops the card's stream for five seconds instead.
//
// A third argument makes the silence a square wave of that many LSBs instead
// of zeros (90 dB down at 1): for a far end that sends nothing while its
// desktop is silent (RustDesk's, scripts/testrustdesk), so that a client's
// stream is open, and checked to be on the null sink, before the first burst.

#include <alsa/asoundlib.h>

#include <fcntl.h>
#include <math.h>
#include <poll.h>
#include <stdbool.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <termios.h>
#include <time.h>
#include <unistd.h>

#define RATE 48000
#define CHANNELS 2
#define PERIOD (RATE / 200)       // 5 ms
#define BURST_FRAMES (RATE / 2)   // 500 ms
#define AMPLITUDE 16384.0

static long long now_us(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return ts.tv_sec * 1000000LL + ts.tv_nsec / 1000;
}

int main(int argc, char **argv) {
    const char *device = argc > 1 ? argv[1] : "hw:0,0";
    const char *serial = argc > 2 ? argv[2] : "/dev/ttyS0";
    const short quiet = argc > 3 ? (short) atoi(argv[3]) : 0;

    int tty = open(serial, O_RDWR | O_NOCTTY | O_NONBLOCK);
    if (tty < 0) {
        perror(serial);
        return 1;
    }
    struct termios t;
    if (tcgetattr(tty, &t) == 0) {
        cfmakeraw(&t);
        cfsetspeed(&t, B115200);
        tcsetattr(tty, TCSANOW, &t);
    }

    snd_pcm_t *pcm;
    int err = snd_pcm_open(&pcm, device, SND_PCM_STREAM_PLAYBACK, 0);
    if (err < 0) {
        fprintf(stderr, "tone: %s: %s\n", device, snd_strerror(err));
        return 1;
    }
    // No resampling here: QEMU's audiodev is opened at 48 kHz too, so what
    // the client is sent is these samples.
    err = snd_pcm_set_params(pcm, SND_PCM_FORMAT_S16_LE, SND_PCM_ACCESS_RW_INTERLEAVED, CHANNELS,
                             RATE, 0, 40000);
    if (err < 0) {
        fprintf(stderr, "tone: set_params: %s\n", snd_strerror(err));
        return 1;
    }
    printf("tone: playing silence on %s, a 440 Hz burst per 't' on %s\n", device, serial);
    fflush(stdout);

    short buf[PERIOD * CHANNELS];
    long burst = 0;     // frames of the burst still to play
    long phase = 0;     // frames into the burst
    long long arrived = 0;
    bool answer = false;
    for (;;) {
        char in[64];
        ssize_t n;
        while ((n = read(tty, in, sizeof(in))) > 0) {
            for (ssize_t i = 0; i < n; i++) {
                if (in[i] == 't') {
                    burst = BURST_FRAMES;
                    phase = 0;
                    arrived = now_us();
                    answer = true;
                } else if (in[i] == 's') {
                    // The stream stopped for five seconds and started
                    // again, which QEMU sends on as AudioEnd and
                    // AudioBegin: what a guest between two sounds does.
                    snd_pcm_drop(pcm);
                    sleep(5);
                    snd_pcm_prepare(pcm);
                    burst = 0;
                }
            }
        }
        snd_pcm_sframes_t avail = snd_pcm_avail_update(pcm);
        if (avail < 0) {
            snd_pcm_recover(pcm, (int) avail, 1);
            continue;
        }
        if (avail < PERIOD) {
            struct pollfd p = {tty, POLLIN, 0};
            poll(&p, 1, 1);
            continue;
        }
        for (int f = 0; f < PERIOD; f++) {
            short v = (f & 1) ? quiet : (short) -quiet;
            if (burst > 0) {
                v = (short) lrint(AMPLITUDE * sin(2.0 * M_PI * 440.0 * (double) phase / RATE));
                phase++;
                burst--;
            }
            for (int c = 0; c < CHANNELS; c++) {
                buf[f * CHANNELS + c] = v;
            }
        }
        if (answer) {
            snd_pcm_sframes_t delay = 0;
            snd_pcm_delay(pcm, &delay);
            char line[64];
            int len = snprintf(line, sizeof(line), "%lld %ld\n", now_us() - arrived, (long) delay);
            if (write(tty, line, (size_t) len) < 0) {
                // The host may not be listening; the burst plays anyway.
            }
            answer = false;
        }
        snd_pcm_sframes_t w = snd_pcm_writei(pcm, buf, PERIOD);
        if (w < 0) {
            snd_pcm_recover(pcm, (int) w, 1);
        }
    }
}
