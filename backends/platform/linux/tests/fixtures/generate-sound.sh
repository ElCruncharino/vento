#!/bin/bash
# SPDX-FileCopyrightText: 2026 Patrick Gaskin
# SPDX-License-Identifier: GPL-3.0-or-later
#
# The sound decoder tests' fixtures (tests/sound.rs), and the only record of
# how they were made; generate.sh's sibling, for the same reason it gives.
# Run from anywhere; it writes beside itself.
#
# Each is 0.3 s of a 440 Hz sine at half scale, the tone the end-to-end rigs
# play (scripts/testaudio), in what a far end sends:
#
#   aac-44100-stereo.aac  AAC LC at 44.1 kHz in stereo, which is what Windows'
#                         RDP server sends (as raw frames: the ADTS headers
#                         here are what the test cuts the frames at and then
#                         drops, as a protocol never sends them)
#   opus-48000-stereo.opus, opus-48000-mono.opus
#                         Opus at 48 kHz in 20 ms packets, as RustDesk and
#                         RDP send it, in Ogg, which the test walks for the
#                         packets alone
#
# Made with ffmpeg 8 (libavcodec 62.28, its native AAC encoder) and libopus.
set -euo pipefail
cd "$(dirname "$0")"

tone() {
    local rate=$1 layout=$2
    local one="0.5*sin(2*PI*440*t)"
    local expr=$one
    [ "$layout" = stereo ] && expr="$one|$one"
    echo "aevalsrc=$expr:s=$rate:d=0.3"
}

ff() { ffmpeg -hide_banner -loglevel error -y -f lavfi -i "$@"; }

ff "$(tone 44100 stereo)" -c:a aac -b:a 128k -f adts aac-44100-stereo.aac
ff "$(tone 48000 stereo)" -c:a libopus -b:a 96k -frame_duration 20 -application audio \
    -map_metadata -1 -fflags +bitexact -flags:a +bitexact -f ogg opus-48000-stereo.opus
ff "$(tone 48000 mono)" -c:a libopus -b:a 48k -frame_duration 20 -application audio \
    -map_metadata -1 -fflags +bitexact -flags:a +bitexact -f ogg opus-48000-mono.opus
