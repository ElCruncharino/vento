#!/bin/bash
# Build (if needed) and run the relative-pointer test server, then print the
# address to put in the app. testvnc/run.sh's pattern, with the one
# difference this container is for: the pointer at the far end is relative.
set -e
cd "$(dirname "$0")"

NAME="${NAME:-protoqemu}"
PORT="${PORT:-5902}"
SPICE_PORT="${SPICE_PORT:-5930}"
SPICE_TLS_PORT="${SPICE_TLS_PORT:-5931}"
MEMORY="${MEMORY:-512}"
VNC_PASSWORD="${VNC_PASSWORD:-vncpass}"
SPICE_PASSWORD="${SPICE_PASSWORD:-spicepass}"
POINTER="${POINTER:-relative}"
# The audio guest (usage.md): its own image, made on top of protoqemu's, and
# one more port, the guest's serial line, which is how a burst is asked for.
# AUDIO=spice is the same guest heard over SPICE's playback channel.
AUDIO="${AUDIO:-}"
TONE_PORT="${TONE_PORT:-5955}"
# The image is protoqemu's whatever the container is called, so a private
# instance (NAME=..., other ports) serves the same certificate.
IMAGE="${IMAGE:-protoqemu}"
[ -n "$AUDIO" ] && IMAGE="$IMAGE-audio"

build() {
    if [ -n "$AUDIO" ]; then
        docker image inspect protoqemu:latest >/dev/null 2>&1 || docker build -t protoqemu:latest .
        docker build -t "$IMAGE:latest" -f Dockerfile.audio .
    else
        docker build -t "$IMAGE:latest" .
    fi
}

case "$1" in
    stop)    docker rm -f "$NAME" >/dev/null 2>&1 || true; echo "stopped"; exit 0 ;;
    logs)    exec docker logs -f "$NAME" ;;
    build)   build; exit 0 ;;
esac

docker image inspect "$IMAGE:latest" >/dev/null 2>&1 || build
docker rm -f "$NAME" >/dev/null 2>&1 || true

# KVM if this machine has it and this user can open it; KolibriOS boots under
# plain TCG too, just slower.
kvm=()
if [ -r /dev/kvm ] && [ -w /dev/kvm ]; then
    kvm=(--device /dev/kvm)
fi

audio=()
if [ -n "$AUDIO" ]; then
    audio=(-e AUDIO="$AUDIO" -p "$TONE_PORT:4555")
fi

docker run -d --name "$NAME" "${kvm[@]}" \
    -p "$PORT:5900" \
    -p "$SPICE_PORT:5930" \
    -p "$SPICE_TLS_PORT:5931" \
    -e MEMORY="$MEMORY" \
    -e VNC_PASSWORD="$VNC_PASSWORD" \
    -e SPICE_PASSWORD="$SPICE_PASSWORD" \
    -e POINTER="$POINTER" \
    -e SPICE_WAN="${SPICE_WAN:-}" \
    "${audio[@]}" \
    "$IMAGE:latest" >/dev/null

ip="$(ip -4 -o addr show scope global | awk '{print $4}' | cut -d/ -f1 | head -1)"
echo "$NAME up: $ip:$PORT  (pointer $POINTER, password '$VNC_PASSWORD')"
echo "     spice: $ip:$SPICE_PORT  (password '$SPICE_PASSWORD'), TLS on $ip:$SPICE_TLS_PORT"
if [ -n "$AUDIO" ]; then
    where="QEMU audio on the VNC port"
    [ "$AUDIO" = spice ] && where="the SPICE playback channel (and QEMU audio on the VNC port)"
    echo "     audio: $where; a 440 Hz burst per 't' sent to $ip:$TONE_PORT"
fi
echo "the guest takes a few seconds to reach its desktop"
