#!/bin/bash
# QEMU with its own VNC server on 5900, its SPICE server on 5930 and the same
# SPICE server's TLS port on 5931, and a KolibriOS guest on the screen of all
# three.
set -e

MEMORY="${MEMORY:-256}"
VNC_PASSWORD="${VNC_PASSWORD:-vncpass}"
SPICE_PASSWORD="${SPICE_PASSWORD:-spicepass}"
# Absolute is what a `usb-tablet` gives, and it is here only so that the two
# modes can be compared against the same guest — the point of the container is
# the default.
POINTER="${POINTER:-relative}"

accel="tcg"
if [ -e /dev/kvm ] && [ -w /dev/kvm ]; then
    accel="kvm"
fi

tablet=()
if [ "$POINTER" = "absolute" ]; then
    tablet=(-device usb-ehci -device usb-tablet)
fi

# A VNC password rather than none, so this rig exercises the same VncAuth path
# the others do. QEMU takes it as a secret object; eight characters is the
# protocol's limit, not ours.
secret=(-object "secret,id=vncpw,data=${VNC_PASSWORD}")
auth="password-secret=vncpw"
if [ -z "$VNC_PASSWORD" ]; then
    secret=()
    auth=""
fi

# SPICE's ticket, which is a password encrypted to a key the server sends and
# not a challenge over one. Ticketing is on by default and refuses every
# connection until a ticket is set, so an empty password has to say so.
# The two encodings a client cannot ask for: JPEG and zlib-over-GLZ are what
# the server reaches for when it thinks the link is a WAN, and `always` is the
# only way to see either of them from a container on this network.
wan=""
if [ -n "${SPICE_WAN:-}" ]; then
    wan=",jpeg-wan-compression=always,zlib-glz-wan-compression=always"
fi

spicesecret=(-object "secret,id=spicepw,data=${SPICE_PASSWORD}")
spiceauth="password-secret=spicepw"
if [ -z "$SPICE_PASSWORD" ]; then
    spicesecret=()
    spiceauth="disable-ticketing=on"
fi

# The certificate is the image's, made when it was built, so the fingerprint a
# client pins is the same one after a restart.
echo "accel=$accel pointer=$POINTER memory=${MEMORY}M password=${VNC_PASSWORD:-none} spice=${SPICE_PASSWORD:-none}"
openssl x509 -in /certs/server-cert.pem -noout -fingerprint -sha256

# The audio guest (AUDIO=1, the protoqemu-audio image): Debian's kernel and
# a busybox initramfs that keeps an HDA card playing silence and plays 440 Hz
# for half a second whenever a 't' arrives on the serial port, which is a TCP
# server on 4555. The card's output goes to QEMU's `none` audiodev, which
# plays nothing anywhere but still runs the mixer that the VNC server's audio
# capture taps — so the only place the guest is heard is a VNC client that
# asks. 48 kHz stereo S16 at both ends, which is what TigerVNC asks for, so
# QEMU resamples nothing. The codec's mixer is off so that the guest's volume
# controls (nobody sets them) cannot change the level.
guest=(-drive file=/kolibri.img,format=raw,if=floppy,readonly=on -boot a)
serial=(-serial none)
vncaudio=""
if [ -n "${AUDIO:-}" ]; then
    [ -e /audio/vmlinuz ] || { echo "AUDIO is set but this image has no audio guest (build Dockerfile.audio)" >&2; exit 1; }
    guest=(-kernel /audio/vmlinuz -initrd /audio/initrd.gz -append "console=tty0 quiet"
        -audiodev "none,id=snd0,out.frequency=48000,out.channels=2,out.format=s16"
        -device intel-hda -device "hda-output,audiodev=snd0,mixer=off"
        -chardev "socket,id=tone,host=0.0.0.0,port=4555,server=on,wait=off,nodelay=on")
    serial=(-serial chardev:tone)
    vncaudio=",audiodev=snd0"
    echo "audio guest: tone bursts on the serial port, tcp 4555"
fi

# Read only, so the container keeps no state and every run is the same desktop.
exec qemu-system-x86_64 \
    -machine "type=pc,accel=$accel" \
    -m "$MEMORY" \
    "${guest[@]}" \
    -vga std \
    "${secret[@]}" \
    -vnc "0.0.0.0:0${auth:+,$auth}$vncaudio" \
    "${spicesecret[@]}" \
    -spice "port=5930,tls-port=5931,addr=0.0.0.0,x509-dir=/certs,$spiceauth$wan" \
    "${tablet[@]}" \
    -nographic \
    "${serial[@]}" \
    -monitor none
