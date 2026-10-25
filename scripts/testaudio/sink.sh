#!/bin/bash
# A PipeWire null sink to play vento's sound into instead of the speakers,
# and the check that a stream really went there. See usage.md.
#
#   sink.sh load          load the sink (once), print its module id
#   sink.sh unload ID     unload what `load` printed
#   sink.sh check PID     wait (10 s) for PID's stream and succeed only if it
#                         is on the sink; prints the stream's line. Two
#                         programs of one name at once would be confused.
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
        index="$(sink_index)"
        [ -n "$index" ] || { echo "sink $SINK is not loaded" >&2; exit 1; }
        # A stream through ALSA's PipeWire plugin carries no process id,
        # only the program's name: "alsa_playback.<comm>".
        comm="$(cat "/proc/$pid/comm")"
        for _ in $(seq 50); do
            line="$(pactl -f json list sink-inputs | python3 -I -c '
import json, sys
pid, index, comm = sys.argv[1], int(sys.argv[2]), sys.argv[3]
for s in json.load(sys.stdin):
    p = s.get("properties", {})
    if p.get("application.process.id") == pid or p.get("node.name") == "alsa_playback." + comm:
        where = "on" if s["sink"] == index else "NOT on"
        print(where, "sink-input", s["index"], "sink", s["sink"], p.get("application.name"), p.get("media.name"))
        break' "$pid" "$index" "$comm")"
            case "$line" in
                on*) echo "$line"; exit 0 ;;
                NOT*) echo "$line" >&2; exit 1 ;;
            esac
            sleep 0.2
        done
        echo "no stream from pid $pid" >&2
        exit 1
        ;;
    *)
        sed -n '2,13p' "$0"
        exit 2
        ;;
esac
