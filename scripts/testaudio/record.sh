#!/bin/bash
# Records the null sink's monitor to a WAV file, for a session played into
# it (see usage.md): `record.sh out.wav [seconds]`, 48 kHz stereo S16, until
# the time is up or it is interrupted. analyse.py summary reads it.
set -e
out="${1:?out.wav}"
seconds="${2:-}"
cmd=(parec --device="${SINK:-vento_test}.monitor" --format=s16le --rate=48000 --channels=2 --file-format=wav "$out")
if [ -n "$seconds" ]; then
    exec timeout --signal=INT "$seconds" "${cmd[@]}"
fi
exec "${cmd[@]}"
