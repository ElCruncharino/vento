#!/bin/bash
# The desktop's sound output, end to end: the `audio_rig` example plays its
# script into a null sink, records the sink's monitor, and analyse.py reads
# the recording back. See usage.md.
#
#   rig.sh [OUT_DIR]      default build/scratch/audio/rig
#
# Nothing is audible: the rig writes silence until this script has seen its
# stream on the null sink, and only then says `go`.
set -e
here="$(cd "$(dirname "$0")" && pwd)"
repo="$(cd "$here/../.." && pwd)"
out="$(realpath -m "${1:-$repo/build/scratch/audio/rig}")"
mkdir -p "$out"
rm -f "$out/ctl" "$out/rec.raw" "$out/timeline.json"

# The platform crate's build script builds FFmpeg, whose assembly wants NASM;
# a PATH with one in it is the caller's business (see the repo's notes).
(cd "$repo" && cargo build --offline -q -p remotedesktop-platform-linux --features probe --example audio_rig)
rig="$repo/target/debug/examples/audio_rig"

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
PIPEWIRE_NODE=vento_test "$rig" --out "$out" < "$out/ctl" > "$out/rig.log" 2>&1 &
pid=$!
exec 3>"$out/ctl"
for _ in $(seq 100); do grep -q READY "$out/rig.log" && break; sleep 0.1; done
grep -q READY "$out/rig.log" || { cat "$out/rig.log"; echo "the rig did not start" >&2; exit 1; }

# The route, before anything but silence is written.
"$here/sink.sh" check "$pid"
echo go >&3
wait "$pid"
pid=""
cat "$out/rig.log"
python3 -I "$here/analyse.py" rig "$out"
