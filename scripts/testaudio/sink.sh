#!/bin/bash
# A PipeWire null sink to play vento's sound into instead of the speakers,
# and the check that a stream really went there. See usage.md.
#
#   sink.sh load          load the sink (once), print its module id
#   sink.sh unload ID     unload what `load` printed
#   sink.sh check PID [N] wait (10 s) for PID's stream (N of them: two
#                         sessions are two) and succeed only if every one is
#                         on the sink; prints a line a stream, with the output
#                         it came through (PipeWire, a stream of vento's own,
#                         or ALSA, PipeWire's ALSA plugin). Two programs of
#                         one name at once would be confused on ALSA.
#   sink.sh off           unload every module named vento_test (cleanup)
#
# Never touches the default sink: `load` compares it before and after, and
# unloads at once (and fails) if loading moved it.
set -e

SINK="${SINK:-vento_test}"

sink_index() {
    pactl -f json list sinks short | python3 -I -c '
import json, sys
for s in json.load(sys.stdin):
    if s["name"] == sys.argv[1]:
        print(s["index"])' "$SINK"
}

case "$1" in
    load)
        before="$(pactl get-default-sink)"
        if [ -n "$(sink_index)" ]; then
            echo "sink $SINK is already loaded" >&2
            exit 1
        fi
        # Lowest priority, so it can never be chosen as the default.
        id="$(pactl load-module module-null-sink sink_name="$SINK" \
            sink_properties="device.description=$SINK priority.session=1 priority.driver=1")"
        after="$(pactl get-default-sink)"
        if [ "$before" != "$after" ]; then
            pactl unload-module "$id"
            echo "loading $SINK moved the default sink ($before -> $after); unloaded" >&2
            exit 1
        fi
        echo "$id"
        ;;
    unload)
        pactl unload-module "${2:?module id}"
        ;;
    off)
        pactl -f json list modules short | python3 -I -c '
import json, sys
for m in json.load(sys.stdin):
    if m["name"] == "module-null-sink" and "sink_name=" + sys.argv[1] in (m.get("argument") or ""):
        print(m["index"])' "$SINK" | while read -r id; do pactl unload-module "$id"; done
        ;;
    check)
        pid="${2:?pid}"
        want="${3:-1}"
        index="$(sink_index)"
        [ -n "$index" ] || { echo "sink $SINK is not loaded" >&2; exit 1; }
        # vento's own PipeWire streams carry its process id (node.name
        # "vento"); a stream through ALSA's PipeWire plugin carries no
        # process id, only the program's name: "alsa_playback.<comm>".
        comm="$(cat "/proc/$pid/comm")"
        for _ in $(seq 50); do
            lines="$(pactl -f json list sink-inputs | python3 -I -c '
import json, sys
pid, index, comm, want = sys.argv[1], int(sys.argv[2]), sys.argv[3], int(sys.argv[4])
mine = []
for s in json.load(sys.stdin):
    p = s.get("properties", {})
    node = p.get("node.name", "")
    if node == "alsa_playback." + comm:
        output = "ALSA"
    elif p.get("application.process.id") == pid:
        output = "PipeWire" if node == "vento" else node
    else:
        continue
    where = "on" if s["sink"] == index else "NOT on"
    mine.append(" ".join(str(x) for x in (where, "sink-input", s["index"], "sink", s["sink"], output,
                                          p.get("application.name"), p.get("media.name"))))
if any(m.startswith("NOT") for m in mine) or len(mine) >= want:
    print("\n".join(mine))' "$pid" "$index" "$comm" "$want")"
            case "$lines" in
                *NOT*) echo "$lines" >&2; exit 1 ;;
                on*) echo "$lines"; exit 0 ;;
            esac
            sleep 0.2
        done
        echo "no stream (or fewer than $want) from pid $pid" >&2
        exit 1
        ;;
    *)
        sed -n '2,16p' "$0"
        exit 2
        ;;
esac
