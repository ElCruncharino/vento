#!/bin/bash
# An X server with a window manager on the host's DISPLAY=:42, for running the
# Linux client on X11 without Xwayland in the way.
set -e
here="$PWD"
cd "$(dirname "$0")"
NAME="${NAME:-vento-testx11}"
DISPLAY_NUM="${DISPLAY_NUM:-42}"
GEOMETRY="${GEOMETRY:-1600x1000}"
case "$1" in
    stop)  docker stop -t 3 "$NAME" >/dev/null 2>&1 || true; docker rm -f "$NAME" >/dev/null 2>&1 || true
           echo "stopped"; exit 0 ;;
    logs)  exec docker logs -f "$NAME" ;;
    keys)  exec docker exec "$NAME" cat /rig/out/wm-keys.log ;;   # the WM's witness log
    shot)  # the whole screen; the cursor is never in it (see cursor.py)
           out="${2:-$here/$NAME.png}"
           import -display ":$DISPLAY_NUM" -window root "$out"; echo "$out"; exit 0 ;;
    build) docker build -t "$NAME:latest" . ; exit 0 ;;
esac
if DISPLAY=":$DISPLAY_NUM" xdpyinfo >/dev/null 2>&1; then
    echo "something already answers on :$DISPLAY_NUM" >&2; exit 1
fi
docker image inspect "$NAME:latest" >/dev/null 2>&1 || docker build -t "$NAME:latest" .
docker rm -f "$NAME" >/dev/null 2>&1 || true
# --network host puts Xvfb's abstract socket in the host's namespace, which is
# how DISPLAY=:42 reaches it from here; --ipc=host lets MIT-SHM work across.
docker run -d --name "$NAME" --network host --ipc=host \
    -e DISPLAY=":$DISPLAY_NUM" -e GEOMETRY="$GEOMETRY" "$NAME:latest" >/dev/null
for i in $(seq 50); do DISPLAY=":$DISPLAY_NUM" xdpyinfo >/dev/null 2>&1 && break; sleep 0.1; done
echo "$NAME up: DISPLAY=:$DISPLAY_NUM ($GEOMETRY, Openbox; witnesses W-Return and C-A-t)"
