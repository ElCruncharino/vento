#!/bin/bash
# A far end's rhythm through the desktop's output: the `audio_pattern`
# example writes a protocol's pattern of writes with a tone burst every two
# seconds into a null sink, and pattern.py times each burst from its write to
# its capture and counts what went missing. See usage.md.
#
#   pattern.sh OUT_DIR PATTERN SECONDS [audio_pattern's options...]
#
# The environment passes through (VENTO_AUDIO_OUTPUT=alsa for ALSA). Nothing
# is audible: the example writes silence until this script has seen its
# stream on the null sink, and only then says `go`.
set -e
here="$(cd "$(dirname "$0")" && pwd)"
repo="$(cd "$here/../.." && pwd)"
out="$(realpath -m "$1")"
pattern="$2"
seconds="$3"
shift 3
mkdir -p "$out"
rm -f "$out/ctl" "$out/rec.raw"

# As rig.sh: NASM on PATH the first time is the caller's business.
# AUDIO_PATTERN runs another build of the example (one kept from before a
# change, to compare), as it is.
bin="$AUDIO_PATTERN"
if [ -z "$bin" ]; then
    (cd "$repo" && cargo build --offline -q -p remotedesktop-platform-desktop --features probe --example audio_pattern)
    bin="$repo/target/debug/examples/audio_pattern"
fi

module="$("$here/sink.sh" load)"
pid=""
cleanup() {
    exec 3>&- 2>/dev/null || true
    if [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null; then kill "$pid"; fi
    pactl unload-module "$module"
    rm -f "$out/ctl"
}
trap cleanup EXIT

mkfifo "$out/ctl"
PIPEWIRE_NODE=vento_test "$bin" "$pattern" "$seconds" "$@" < "$out/ctl" > "$out/client.out" 2> "$out/client.log" &
pid=$!
exec 3>"$out/ctl"
for _ in $(seq 100); do grep -q READY "$out/client.out" && break; sleep 0.1; done
grep -q READY "$out/client.out" || { cat "$out/client.log"; echo "the example did not start" >&2; exit 1; }

# The route, before anything but silence is written.
"$here/sink.sh" check "$pid"
python3 -I "$here/pattern.py" "$out" "$seconds" 3>&-
wait "$pid" || true
pid=""
grep -E '^DONE' "$out/client.out" || true
grep -E 'WARN|playing' "$out/client.log" | head -3
