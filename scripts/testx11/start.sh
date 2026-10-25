#!/bin/sh
# Xvfb on $DISPLAY, Openbox with the witness bindings added to its stock rc.xml,
# and one xterm to take the focus away to. With --network host the server's
# abstract socket (@/tmp/.X11-unix/X42) is in the host's namespace, which is
# how host clients reach it with DISPLAY=:42 and no socket file is left behind.
set -e
: "${DISPLAY:=:42}" "${GEOMETRY:=1600x1000}"
export DISPLAY
mkdir -p /rig/out /root/.config/openbox
: > /rig/out/wm-keys.log
# The stock bindings (A-Tab, C-A-Left/Right, W-F1..F4, …) stay; the witnesses
# go in just before </keyboard>.
awk 'BEGIN { while ((getline l < "/rig/rc-keys.xml") > 0) k = k l "\n" }
     /<\/keyboard>/ { printf "%s", k } { print }' \
    /etc/xdg/openbox/rc.xml > /root/.config/openbox/rc.xml
trap 'kill $(jobs -p) 2>/dev/null; wait; exit 0' TERM INT
Xvfb "$DISPLAY" -screen 0 "${GEOMETRY}x24" -ac -nolisten tcp &
for i in $(seq 50); do xdpyinfo >/dev/null 2>&1 && break; sleep 0.1; done
openbox --sm-disable &
sleep 0.5
xsetroot -solid '#203040'
xterm -geometry 80x20+900+600 -title witness-xterm &
echo "rig up on $DISPLAY ($GEOMETRY)"
wait
